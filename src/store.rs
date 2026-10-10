// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Tiny local truth store for the early engine.
//!
//! This stays deliberately simple and low-overhead:
//! - registries for entities, metrics, and sources
//! - latest-value index
//! - recent significant samples in memory
//! - optional append-only NDJSON journals on disk
//! - optional SQLite-backed warm history

use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::entity::{Boundary, Edge, Node};
use crate::ingest::{RawIngestRecord, SourceDefinition};
use crate::metric::{
    LatestValue, MetricDefinition, MetricRetentionReport, RetentionTuning, Sample,
};
use crate::types::{EntityId, MetricId, SourceId, Timestamp, ValidationError};
use crate::view::TimeRange;
use crate::warm_store::{SqliteWarmStore, WarmStoreError, WarmStoreMaintenanceReport};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoreError {
    Validation(String),
    UnknownMetric(MetricId),
    UnknownSource(SourceId),
    Io(String),
    Serde(String),
    WarmStore(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validation(msg) => write!(f, "store validation error: {msg}"),
            Self::UnknownMetric(id) => write!(f, "unknown metric: {id}"),
            Self::UnknownSource(id) => write!(f, "unknown source: {id}"),
            Self::Io(msg) => write!(f, "store I/O error: {msg}"),
            Self::Serde(msg) => write!(f, "store serialization error: {msg}"),
            Self::WarmStore(msg) => write!(f, "warm-store error: {msg}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<ValidationError> for StoreError {
    fn from(value: ValidationError) -> Self {
        Self::Validation(value.to_string())
    }
}

impl From<WarmStoreError> for StoreError {
    fn from(value: WarmStoreError) -> Self {
        Self::WarmStore(value.to_string())
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct StoreStats {
    pub node_count: usize,
    pub edge_count: usize,
    pub boundary_count: usize,
    pub metric_count: usize,
    pub source_count: usize,
    pub latest_value_count: usize,
    pub retained_sample_count: usize,
    pub warm_sample_count: usize,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct BatchIngestReport {
    pub raw_records_seen: usize,
    pub raw_records_written: usize,
    pub samples_seen: usize,
    pub samples_stored: usize,
    pub latest_updates: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SampleIngestOutcome {
    pub updated_latest: bool,
    pub stored_history: bool,
}

#[derive(Debug)]
pub struct EngineStore {
    nodes: HashMap<EntityId, Node>,
    edges: HashMap<EntityId, Edge>,
    boundaries: HashMap<EntityId, Boundary>,
    metrics: HashMap<MetricId, MetricDefinition>,
    sources: HashMap<SourceId, SourceDefinition>,
    latest_values: HashMap<(EntityId, MetricId), LatestValue>,
    retained_samples: HashMap<(EntityId, MetricId), Vec<Sample>>,
    journal_dir: Option<PathBuf>,
    warm_store: Option<SqliteWarmStore>,
}

impl Default for EngineStore {
    fn default() -> Self {
        Self::new()
    }
}

impl EngineStore {
    #[must_use]
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: HashMap::new(),
            boundaries: HashMap::new(),
            metrics: HashMap::new(),
            sources: HashMap::new(),
            latest_values: HashMap::new(),
            retained_samples: HashMap::new(),
            journal_dir: None,
            warm_store: None,
        }
    }

    pub fn with_journal_dir(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        fs::create_dir_all(&path).map_err(|err| StoreError::Io(err.to_string()))?;
        let mut store = Self::new();
        store.journal_dir = Some(path);
        Ok(store)
    }

    pub fn with_sqlite_warm_store(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let mut store = Self::new();
        store.warm_store = Some(SqliteWarmStore::open(path)?);
        Ok(store)
    }

    pub fn with_backends(
        journal_dir: Option<PathBuf>,
        warm_store_path: Option<PathBuf>,
    ) -> Result<Self, StoreError> {
        let mut store = Self::new();
        if let Some(path) = journal_dir {
            fs::create_dir_all(&path).map_err(|err| StoreError::Io(err.to_string()))?;
            store.journal_dir = Some(path);
        }
        if let Some(path) = warm_store_path {
            store.warm_store = Some(SqliteWarmStore::open(path)?);
        }
        Ok(store)
    }

    pub fn upsert_node(&mut self, node: Node) -> Result<(), StoreError> {
        node.validate()?;
        self.nodes.insert(node.id, node);
        Ok(())
    }

    pub fn upsert_edge(&mut self, edge: Edge) -> Result<(), StoreError> {
        edge.validate()?;
        self.edges.insert(edge.id, edge);
        Ok(())
    }

    pub fn upsert_boundary(&mut self, boundary: Boundary) -> Result<(), StoreError> {
        boundary.validate()?;
        self.boundaries.insert(boundary.id, boundary);
        Ok(())
    }

    pub fn register_metric(&mut self, metric: MetricDefinition) -> Result<(), StoreError> {
        metric.validate()?;
        self.metrics.insert(metric.id, metric);
        Ok(())
    }

    pub fn register_source(&mut self, source: SourceDefinition) -> Result<(), StoreError> {
        source.validate()?;
        self.sources.insert(source.id, source);
        Ok(())
    }

    #[must_use]
    pub fn metric(&self, metric_id: MetricId) -> Option<&MetricDefinition> {
        self.metrics.get(&metric_id)
    }

    pub fn retune_metric_retention(
        &mut self,
        metric_id: MetricId,
        tuning: RetentionTuning,
    ) -> Result<MetricDefinition, StoreError> {
        let metric = self
            .metrics
            .get_mut(&metric_id)
            .ok_or(StoreError::UnknownMetric(metric_id))?;
        metric.retention = tuning.apply_to(metric.retention, metric.value_type)?;
        metric.validate()?;
        Ok(metric.clone())
    }

    pub fn retention_report(
        &self,
        metric_id: MetricId,
    ) -> Result<MetricRetentionReport, StoreError> {
        let metric = self
            .metrics
            .get(&metric_id)
            .ok_or(StoreError::UnknownMetric(metric_id))?;
        let hot_sample_count = self
            .retained_samples
            .iter()
            .filter(|((_, id), _)| *id == metric_id)
            .map(|(_, samples)| samples.len())
            .sum();
        let warm_sample_count = self
            .warm_store
            .as_ref()
            .map(|store| store.sample_count_for_metric(metric_id))
            .transpose()?
            .unwrap_or(0);

        Ok(MetricRetentionReport {
            metric_id,
            metric_name: metric.name.clone(),
            hot_sample_count,
            warm_sample_count,
            hot_duration: metric.retention.hot_duration,
            warm_duration: metric.retention.warm_duration,
            store_on_change_only: metric.retention.store_on_change_only,
            change_threshold: metric.retention.change_threshold,
            relative_change_threshold: metric.retention.relative_change_threshold,
        })
    }

    #[must_use]
    pub fn node(&self, entity_id: EntityId) -> Option<&Node> {
        self.nodes.get(&entity_id)
    }

    #[must_use]
    pub fn edge(&self, entity_id: EntityId) -> Option<&Edge> {
        self.edges.get(&entity_id)
    }

    #[must_use]
    pub fn boundary(&self, entity_id: EntityId) -> Option<&Boundary> {
        self.boundaries.get(&entity_id)
    }

    #[must_use]
    pub fn nodes_cloned(&self) -> Vec<Node> {
        self.nodes.values().cloned().collect()
    }

    #[must_use]
    pub fn edges_cloned(&self) -> Vec<Edge> {
        self.edges.values().cloned().collect()
    }

    pub(crate) fn edges_iter(&self) -> impl Iterator<Item = &Edge> {
        self.edges.values()
    }

    #[must_use]
    pub fn boundaries_cloned(&self) -> Vec<Boundary> {
        self.boundaries.values().cloned().collect()
    }

    #[must_use]
    pub fn source(&self, source_id: SourceId) -> Option<&SourceDefinition> {
        self.sources.get(&source_id)
    }

    pub fn source_mut(&mut self, source_id: SourceId) -> Option<&mut SourceDefinition> {
        self.sources.get_mut(&source_id)
    }

    #[must_use]
    pub fn latest_value(&self, entity_id: EntityId, metric_id: MetricId) -> Option<&LatestValue> {
        self.latest_values.get(&(entity_id, metric_id))
    }

    pub(crate) fn latest_values_iter(
        &self,
    ) -> impl Iterator<Item = ((EntityId, MetricId), &LatestValue)> {
        self.latest_values.iter().map(|(key, value)| (*key, value))
    }

    #[must_use]
    pub fn latest_values_cloned(&self) -> Vec<((EntityId, MetricId), LatestValue)> {
        self.latest_values
            .iter()
            .map(|(key, value)| (*key, value.clone()))
            .collect()
    }

    #[must_use]
    pub fn samples_for(
        &self,
        entity_id: EntityId,
        metric_id: MetricId,
        range: TimeRange,
        now: Timestamp,
    ) -> Vec<Sample> {
        self.samples_for_result(entity_id, metric_id, range, now)
            .unwrap_or_default()
    }

    pub fn samples_for_result(
        &self,
        entity_id: EntityId,
        metric_id: MetricId,
        range: TimeRange,
        now: Timestamp,
    ) -> Result<Vec<Sample>, StoreError> {
        let mut out = self
            .retained_samples
            .get(&(entity_id, metric_id))
            .map(|samples| {
                samples
                    .iter()
                    .filter(|sample| range.contains(sample.ts_observed, now))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        if let Some(warm_store) = &self.warm_store {
            let mut seen: HashSet<(Timestamp, SourceId)> = out
                .iter()
                .map(|sample| (sample.ts_observed, sample.source_id))
                .collect();
            for sample in warm_store.query_samples(entity_id, metric_id, range, now)? {
                let key = (sample.ts_observed, sample.source_id);
                if seen.insert(key) {
                    out.push(sample);
                }
            }
            out.sort_by_key(|sample| sample.ts_observed);
        }

        Ok(out)
    }

    pub fn ingest_raw_record(&mut self, record: &RawIngestRecord) -> Result<(), StoreError> {
        if !self.sources.contains_key(&record.source_id) {
            return Err(StoreError::UnknownSource(record.source_id));
        }
        self.append_ndjson("raw-records.ndjson", record)
    }

    pub fn ingest_raw_records_batch(
        &mut self,
        records: &[RawIngestRecord],
    ) -> Result<usize, StoreError> {
        for record in records {
            self.ingest_raw_record(record)?;
        }
        Ok(records.len())
    }

    pub fn ingest_samples_batch(
        &mut self,
        samples: &[Sample],
    ) -> Result<Vec<SampleIngestOutcome>, StoreError> {
        let mut outcomes = Vec::with_capacity(samples.len());
        for sample in samples {
            outcomes.push(self.ingest_sample(sample.clone())?);
        }
        Ok(outcomes)
    }

    pub fn ingest_batch(
        &mut self,
        raw_records: &[RawIngestRecord],
        samples: &[Sample],
    ) -> Result<BatchIngestReport, StoreError> {
        let raw_records_written = self.ingest_raw_records_batch(raw_records)?;
        let outcomes = self.ingest_samples_batch(samples)?;
        Ok(BatchIngestReport {
            raw_records_seen: raw_records.len(),
            raw_records_written,
            samples_seen: samples.len(),
            samples_stored: outcomes
                .iter()
                .filter(|outcome| outcome.stored_history)
                .count(),
            latest_updates: outcomes
                .iter()
                .filter(|outcome| outcome.updated_latest)
                .count(),
        })
    }

    pub fn ingest_sample(&mut self, sample: Sample) -> Result<SampleIngestOutcome, StoreError> {
        let metric = self
            .metrics
            .get(&sample.metric_id)
            .ok_or(StoreError::UnknownMetric(sample.metric_id))?;
        sample.validate(metric)?;

        let key = (sample.entity_id, sample.metric_id);
        let previous = self.latest_values.get(&key);
        let should_store = metric.retention.should_store(previous, &sample);

        self.latest_values
            .insert(key, LatestValue::from_sample(&sample));

        if should_store {
            self.retained_samples
                .entry(key)
                .or_default()
                .push(sample.clone());
            self.append_ndjson("samples.ndjson", &sample)?;
            if let Some(warm_store) = &self.warm_store {
                warm_store.insert_sample(&sample)?;
            }
        }

        Ok(SampleIngestOutcome {
            updated_latest: true,
            stored_history: should_store,
        })
    }

    pub fn prune_retained_samples(&mut self, now: Timestamp) {
        let _ = self.prune_all(now);
    }

    pub fn prune_all(&mut self, now: Timestamp) -> Result<(), StoreError> {
        let metrics = &self.metrics;
        self.retained_samples.retain(|(_, metric_id), samples| {
            let Some(metric) = metrics.get(metric_id) else {
                samples.clear();
                return false;
            };

            let cutoff = now
                - chrono::TimeDelta::from_std(metric.retention.hot_duration)
                    .unwrap_or_else(|_| chrono::TimeDelta::days(36_500));
            samples.retain(|sample| sample.ts_observed > cutoff);
            !samples.is_empty()
        });

        if let Some(warm_store) = &self.warm_store {
            warm_store.prune_for_metrics(&self.metrics, now)?;
        }

        Ok(())
    }

    #[must_use]
    pub fn entity_label(&self, entity_id: EntityId) -> Option<String> {
        if let Some(node) = self.nodes.get(&entity_id) {
            return Some(node.label.clone());
        }
        if let Some(boundary) = self.boundaries.get(&entity_id) {
            return Some(boundary.label.clone());
        }
        if self.edges.contains_key(&entity_id) {
            return Some(format!("edge:{entity_id}"));
        }
        None
    }

    #[must_use]
    pub fn stats(&self) -> StoreStats {
        let warm_sample_count = self
            .warm_store
            .as_ref()
            .and_then(|store| store.sample_count().ok())
            .unwrap_or(0);

        StoreStats {
            node_count: self.nodes.len(),
            edge_count: self.edges.len(),
            boundary_count: self.boundaries.len(),
            metric_count: self.metrics.len(),
            source_count: self.sources.len(),
            latest_value_count: self.latest_values.len(),
            retained_sample_count: self.retained_samples.values().map(Vec::len).sum(),
            warm_sample_count,
        }
    }

    #[must_use]
    pub fn metric_ids(&self) -> Vec<MetricId> {
        self.metrics.keys().copied().collect()
    }

    #[must_use]
    pub fn source_ids(&self) -> Vec<SourceId> {
        self.sources.keys().copied().collect()
    }

    #[must_use]
    pub fn warm_store(&self) -> Option<&SqliteWarmStore> {
        self.warm_store.as_ref()
    }

    pub fn warm_store_maintenance_report(
        &self,
    ) -> Result<Option<WarmStoreMaintenanceReport>, StoreError> {
        self.warm_store
            .as_ref()
            .map(SqliteWarmStore::maintenance_report)
            .transpose()
            .map_err(StoreError::from)
    }

    pub fn optimize_warm_store(
        &self,
        vacuum: bool,
    ) -> Result<Option<WarmStoreMaintenanceReport>, StoreError> {
        self.warm_store
            .as_ref()
            .map(|store| store.optimize(vacuum))
            .transpose()
            .map_err(StoreError::from)
    }

    fn append_ndjson<T: Serialize>(&self, file_name: &str, value: &T) -> Result<(), StoreError> {
        let Some(dir) = &self.journal_dir else {
            return Ok(());
        };

        let path = dir.join(file_name);
        append_line_json(&path, value)
    }
}

fn append_line_json<T: Serialize>(path: &Path, value: &T) -> Result<(), StoreError> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|err| StoreError::Io(err.to_string()))?;

    serde_json::to_writer(&mut file, value).map_err(|err| StoreError::Serde(err.to_string()))?;
    file.write_all(b"\n")
        .map_err(|err| StoreError::Io(err.to_string()))?;
    file.flush()
        .map_err(|err| StoreError::Io(err.to_string()))?;
    Ok(())
}
