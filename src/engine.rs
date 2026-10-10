// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Early engine slice: source scheduling, adapter dispatch, local storage, and hot cache.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::adapter::{AdapterError, ManualPushAdapter, SourceAdapter, SourcePull};
use crate::cache::{CacheBudget, CacheEntry, DetailTier, RingBuffer};
use crate::entity::{Boundary, Edge, Node};
use crate::governance::{
    AuditRecord, AuditTrail, ExportAuditRecord, FailureRecord, PolicyVerdict, SampleAuditRecord,
    SampleProvenance, SourceCapabilityProfile, SourcePolicy, TransformStep,
};
use crate::ingest::{ScheduleEntry, SourceDefinition, SourceHealth};
use crate::materialize::{
    SparseGeoMaterializer, SparseGeoViewMaterialization, TopologyMaterializer,
    TopologyViewMaterialization,
};
use crate::metric::{
    LatestValue, MetricDefinition, MetricRetentionReport, RetentionTuning, Sample,
};
use crate::observability::{EngineEvent, EventBuffer, EventBufferConfig, EventKind, EventLevel};
use crate::profiling::{
    cache_health, profile_sparse_geo, profile_topology, EngineHealthReport, PerfProbeConfig,
    PerfProbeReport, SourceHealthCount,
};
use crate::query::{
    AlertEvent, EntityClass, QueryFilter, QueryRequest, QueryResult, QueryResultEnvelope, QueryRow,
    QueryRowComparison, QuerySortKey, Watchlist, WatchlistEvaluation,
};
use crate::store::{EngineStore, StoreError, StoreStats};
use crate::types::{EntityId, MetricId, SourceId, Timestamp, ValidationError};
use crate::view::{DataCard, DataCardField, TimeRange, ViewJob};
use crate::warm_store::WarmStoreMaintenanceReport;
use crate::workload::{
    ReplayIngestReport, ReplayWorkloadRequest, ViewProfileTarget, WorkloadCheckpointReport,
    WorkloadRunReport,
};

fn compare_row_value(a: &QueryRow, b: &QueryRow, ascending: bool) -> std::cmp::Ordering {
    let ordering = match (&a.value, &b.value) {
        (crate::metric::SampleValue::Numeric(left), crate::metric::SampleValue::Numeric(right)) => {
            left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal)
        }
        (crate::metric::SampleValue::Code(left), crate::metric::SampleValue::Code(right)) => {
            left.cmp(right)
        }
        (crate::metric::SampleValue::Flag(left), crate::metric::SampleValue::Flag(right)) => {
            left.cmp(right)
        }
        _ => a.display_value.cmp(&b.display_value),
    };
    if ascending {
        ordering
    } else {
        ordering.reverse()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EngineError {
    Validation(String),
    UnknownSource(SourceId),
    MissingAdapter(SourceId),
    WrongAdapterKind(SourceId),
    Adapter(String),
    Store(String),
    Policy(String),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validation(msg) => write!(f, "engine validation error: {msg}"),
            Self::UnknownSource(id) => write!(f, "unknown source: {id}"),
            Self::MissingAdapter(id) => write!(f, "missing adapter for source: {id}"),
            Self::WrongAdapterKind(id) => write!(f, "wrong adapter kind for source: {id}"),
            Self::Adapter(msg) => write!(f, "adapter error: {msg}"),
            Self::Store(msg) => write!(f, "store error: {msg}"),
            Self::Policy(msg) => write!(f, "policy error: {msg}"),
        }
    }
}

impl std::error::Error for EngineError {}

impl From<ValidationError> for EngineError {
    fn from(value: ValidationError) -> Self {
        Self::Validation(value.to_string())
    }
}

impl From<AdapterError> for EngineError {
    fn from(value: AdapterError) -> Self {
        Self::Adapter(value.to_string())
    }
}

impl From<StoreError> for EngineError {
    fn from(value: StoreError) -> Self {
        Self::Store(value.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineConfig {
    pub cache_budget: CacheBudget,
    pub default_ring_capacity: usize,
    pub default_entity_ttl: Duration,
    pub journal_dir: Option<PathBuf>,
    pub warm_store_path: Option<PathBuf>,
}

impl EngineConfig {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.cache_budget.validate()?;
        if self.default_ring_capacity == 0 {
            return Err(ValidationError::ZeroCapacity(
                "engine.default_ring_capacity".into(),
            ));
        }
        if self.default_entity_ttl.is_zero() {
            return Err(ValidationError::ZeroCapacity(
                "engine.default_entity_ttl".into(),
            ));
        }
        Ok(())
    }
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            cache_budget: CacheBudget {
                max_active_entities: 1_024,
                max_total_ring_samples: 8_192,
                max_highres_entities: 64,
                max_approx_hot_bytes: 16 * 1024 * 1024,
                max_per_entity_hot_bytes: 256 * 1024,
            },
            default_ring_capacity: 32,
            default_entity_ttl: Duration::from_secs(600),
            journal_dir: None,
            warm_store_path: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct TickReport {
    pub due_sources: usize,
    pub polled_sources: usize,
    pub failed_sources: usize,
    pub raw_records_seen: usize,
    pub samples_seen: usize,
    pub samples_stored: usize,
}

pub struct SkeletraceEngine {
    config: EngineConfig,
    store: EngineStore,
    cache_entries: HashMap<EntityId, CacheEntry>,
    schedule: HashMap<SourceId, ScheduleEntry>,
    adapters: HashMap<SourceId, Box<dyn SourceAdapter>>,
    events: EventBuffer,
    source_policy: SourcePolicy,
    source_capabilities: HashMap<SourceId, SourceCapabilityProfile>,
    audit: AuditTrail,
    failures: Vec<FailureRecord>,
}

impl SkeletraceEngine {
    pub fn new(config: EngineConfig) -> Result<Self, EngineError> {
        config.validate()?;
        let store =
            EngineStore::with_backends(config.journal_dir.clone(), config.warm_store_path.clone())?;

        Ok(Self {
            config,
            store,
            cache_entries: HashMap::new(),
            schedule: HashMap::new(),
            adapters: HashMap::new(),
            events: EventBuffer::default(),
            source_policy: SourcePolicy::default(),
            source_capabilities: HashMap::new(),
            audit: AuditTrail::default(),
            failures: Vec::new(),
        })
    }

    #[must_use]
    pub fn store(&self) -> &EngineStore {
        &self.store
    }

    #[must_use]
    pub fn cache_entry(&self, entity_id: EntityId) -> Option<&CacheEntry> {
        self.cache_entries.get(&entity_id)
    }

    #[must_use]
    pub fn stats(&self) -> StoreStats {
        self.store.stats()
    }

    #[must_use]
    pub fn recent_events(&self, limit: usize) -> Vec<EngineEvent> {
        self.events.snapshot(limit)
    }

    #[must_use]
    pub const fn source_policy(&self) -> SourcePolicy {
        self.source_policy
    }

    pub fn set_source_policy(&mut self, policy: SourcePolicy) {
        self.source_policy = policy;
    }

    pub fn set_source_capability(
        &mut self,
        source_id: SourceId,
        capability: SourceCapabilityProfile,
    ) -> Result<(), EngineError> {
        capability.validate()?;
        if self.store.source(source_id).is_none() {
            return Err(EngineError::UnknownSource(source_id));
        }
        self.source_capabilities.insert(source_id, capability);
        Ok(())
    }

    #[must_use]
    pub fn source_capability(&self, source_id: SourceId) -> Option<SourceCapabilityProfile> {
        self.source_capabilities.get(&source_id).copied()
    }

    #[must_use]
    pub fn recent_audit_records(&self, limit: usize) -> Vec<AuditRecord> {
        self.audit.snapshot(limit)
    }

    #[must_use]
    pub fn recent_failures(&self, limit: usize) -> Vec<FailureRecord> {
        if limit == 0 {
            return Vec::new();
        }
        let take = self.failures.len().min(limit);
        self.failures[self.failures.len().saturating_sub(take)..].to_vec()
    }

    pub fn replay_ready_batches(
        &mut self,
        harness: &mut crate::replay::ReplayHarness,
        now: Timestamp,
    ) -> Result<ReplayIngestReport, EngineError> {
        let ready = harness.drain_all_ready(now);
        let mut ready_sources = HashSet::new();
        let mut report = ReplayIngestReport::default();

        for (source_id, pull) in ready {
            ready_sources.insert(source_id);
            report.pulls_processed += 1;
            match self.ingest_replay_pull(source_id, pull, now) {
                Ok(batch_report) => {
                    self.mark_source_success(source_id, now)?;
                    report.raw_records_seen += batch_report.raw_records_seen;
                    report.samples_seen += batch_report.samples_seen;
                    report.samples_stored += batch_report.samples_stored;
                }
                Err(err) => {
                    self.mark_source_failure_with_reason(
                        source_id,
                        now,
                        self.failure_from_error(source_id, now, &err),
                    )?;
                    return Err(err);
                }
            }
        }

        report.ready_sources = ready_sources.len();
        if report.pulls_processed > 0 {
            self.record_event(EngineEvent {
                at: now,
                level: EventLevel::Info,
                kind: EventKind::SourcePollSuccess,
                source_id: None,
                entity_id: None,
                metric_id: None,
                message: format!(
                    "replay ingested: sources={} pulls={} samples={} stored={}",
                    report.ready_sources,
                    report.pulls_processed,
                    report.samples_seen,
                    report.samples_stored
                ),
            });
        }
        Ok(report)
    }

    pub fn run_replay_workload(
        &mut self,
        harness: &mut crate::replay::ReplayHarness,
        request: &ReplayWorkloadRequest,
    ) -> Result<WorkloadRunReport, EngineError> {
        request.validate()?;
        let started_at = request.checkpoints[0];
        let finished_at = *request.checkpoints.last().unwrap_or(&started_at);
        let health_before = self.health_report(started_at);
        let audit_before = self.audit.len();
        let failure_before = self.failures.len();
        let event_before = self.events.len();

        let mut checkpoints = Vec::with_capacity(request.checkpoints.len());
        for at in &request.checkpoints {
            let replay = self.replay_ready_batches(harness, *at)?;
            let tick = self.tick(*at)?;
            checkpoints.push(WorkloadCheckpointReport {
                at: *at,
                replay,
                tick,
            });
        }

        let mut view_profiles = Vec::with_capacity(request.profile_views.len());
        for ViewProfileTarget { view, config } in &request.profile_views {
            view_profiles.push(self.profile_view_materialization(view, finished_at, *config)?);
        }

        let health_after = self.health_report(finished_at);
        Ok(WorkloadRunReport {
            label: request.label.clone(),
            started_at,
            finished_at,
            health_before,
            health_after,
            checkpoints,
            view_profiles,
            audit_delta: self.audit.len().saturating_sub(audit_before),
            failure_delta: self.failures.len().saturating_sub(failure_before),
            event_delta: self.events.len().saturating_sub(event_before),
        })
    }

    pub fn configure_event_buffer(&mut self, config: EventBufferConfig) -> Result<(), EngineError> {
        self.events.reconfigure(config)?;
        Ok(())
    }

    #[must_use]
    pub fn health_report(&self, now: Timestamp) -> EngineHealthReport {
        let mut healthy = 0usize;
        let mut degraded = 0usize;
        let mut unreachable = 0usize;
        let mut disabled = 0usize;
        let mut pending = 0usize;
        for source_id in self.store.source_ids() {
            if let Some(source) = self.store.source(source_id) {
                match source.health {
                    SourceHealth::Healthy => healthy += 1,
                    SourceHealth::Degraded => degraded += 1,
                    SourceHealth::Unreachable => unreachable += 1,
                    SourceHealth::Disabled => disabled += 1,
                    SourceHealth::Pending => pending += 1,
                }
            }
        }
        let source_health_counts = vec![
            SourceHealthCount {
                health: SourceHealth::Healthy,
                count: healthy,
            },
            SourceHealthCount {
                health: SourceHealth::Degraded,
                count: degraded,
            },
            SourceHealthCount {
                health: SourceHealth::Unreachable,
                count: unreachable,
            },
            SourceHealthCount {
                health: SourceHealth::Disabled,
                count: disabled,
            },
            SourceHealthCount {
                health: SourceHealth::Pending,
                count: pending,
            },
        ];

        let due_sources = self
            .schedule
            .values()
            .filter(|entry| entry.is_due(now))
            .count();

        EngineHealthReport {
            generated_at: now,
            store_stats: self.store.stats(),
            cache: cache_health(self.cache_entries.values()),
            source_health_counts,
            due_sources,
            event_counts: self.events.counts(),
        }
    }

    pub fn retune_metric_retention(
        &mut self,
        metric_id: MetricId,
        tuning: RetentionTuning,
        now: Timestamp,
    ) -> Result<MetricRetentionReport, EngineError> {
        let metric = self.store.retune_metric_retention(metric_id, tuning)?;
        let report = self.store.retention_report(metric_id)?;
        self.record_event(EngineEvent {
            at: now,
            level: EventLevel::Info,
            kind: EventKind::MetricRetentionTuned,
            source_id: None,
            entity_id: None,
            metric_id: Some(metric_id),
            message: format!(
                "retuned metric `{}` hot={:?} warm={:?} change_only={}",
                metric.name,
                metric.retention.hot_duration,
                metric.retention.warm_duration,
                metric.retention.store_on_change_only
            ),
        });
        Ok(report)
    }

    pub fn profile_view_materialization(
        &self,
        view_job: &ViewJob,
        now: Timestamp,
        config: PerfProbeConfig,
    ) -> Result<PerfProbeReport, EngineError> {
        config.validate()?;
        match view_job.kind {
            crate::view::ViewKind::Topology => {
                profile_topology(view_job, config.iterations, || {
                    self.materialize_topology(view_job, now)
                })
            }
            crate::view::ViewKind::SparseGeo => {
                profile_sparse_geo(view_job, config.iterations, || {
                    self.materialize_sparse_geo(view_job, now)
                })
            }
            other => Err(EngineError::Validation(format!(
                "profiling is only implemented for topology/sparse-geo views, not {:?}",
                other
            ))),
        }
    }

    pub fn profile_prune_cycle(
        &mut self,
        now: Timestamp,
        config: PerfProbeConfig,
    ) -> Result<PerfProbeReport, EngineError> {
        config.validate()?;
        let mut stats = crate::profiling::DurationStats::default();
        for _ in 0..config.iterations {
            let started = std::time::Instant::now();
            self.store.prune_all(now)?;
            self.evict_expired_cache(now);
            stats.record(started.elapsed());
        }
        Ok(PerfProbeReport {
            label: "prune-cycle".into(),
            iterations: config.iterations,
            stats,
        })
    }

    pub fn register_metric(&mut self, metric: MetricDefinition) -> Result<(), EngineError> {
        self.store.register_metric(metric)?;
        Ok(())
    }

    pub fn register_node(&mut self, node: Node) -> Result<(), EngineError> {
        self.store.upsert_node(node)?;
        Ok(())
    }

    pub fn register_edge(&mut self, edge: Edge) -> Result<(), EngineError> {
        self.store.upsert_edge(edge)?;
        Ok(())
    }

    pub fn register_boundary(&mut self, boundary: Boundary) -> Result<(), EngineError> {
        self.store.upsert_boundary(boundary)?;
        Ok(())
    }

    pub fn register_source(
        &mut self,
        source: SourceDefinition,
        adapter: Box<dyn SourceAdapter>,
        now: Timestamp,
    ) -> Result<(), EngineError> {
        let capability = SourceCapabilityProfile::recommended_for(&source);
        self.register_source_with_capability(source, adapter, capability, now)
    }

    pub fn register_source_with_capability(
        &mut self,
        source: SourceDefinition,
        adapter: Box<dyn SourceAdapter>,
        capability: SourceCapabilityProfile,
        now: Timestamp,
    ) -> Result<(), EngineError> {
        source.validate()?;
        capability.validate()?;
        if source.adapter != adapter.kind() {
            return Err(EngineError::Validation(format!(
                "source adapter mismatch: definition says {:?}, adapter is {:?}",
                source.adapter,
                adapter.kind()
            )));
        }

        match self.source_policy.evaluate_source(&source, &capability) {
            PolicyVerdict::Allow => {}
            PolicyVerdict::Deny { reason } => {
                let failure = FailureRecord::policy(source.id, now, reason.clone());
                self.record_failure(failure.clone());
                self.record_event(EngineEvent {
                    at: now,
                    level: EventLevel::Warn,
                    kind: EventKind::SourcePollFailure,
                    source_id: Some(source.id),
                    entity_id: None,
                    metric_id: None,
                    message: format!("source `{}` denied by policy: {}", source.name, reason),
                });
                return Err(EngineError::Policy(reason));
            }
        }

        self.store.register_source(source.clone())?;
        self.source_capabilities.insert(source.id, capability);
        self.adapters.insert(source.id, adapter);
        self.schedule.insert(
            source.id,
            ScheduleEntry::new(source.id, source.schedule, now, 128, Duration::ZERO)?,
        );
        self.record_event(EngineEvent {
            at: now,
            level: EventLevel::Info,
            kind: EventKind::SourceRegistered,
            source_id: Some(source.id),
            entity_id: None,
            metric_id: None,
            message: format!("registered source `{}`", source.name),
        });
        Ok(())
    }

    pub fn enqueue_manual_batch(
        &mut self,
        source_id: SourceId,
        batch: SourcePull,
    ) -> Result<(), EngineError> {
        let adapter = self
            .adapters
            .get_mut(&source_id)
            .ok_or(EngineError::MissingAdapter(source_id))?;
        let manual = adapter
            .as_any_mut()
            .downcast_mut::<ManualPushAdapter>()
            .ok_or(EngineError::WrongAdapterKind(source_id))?;
        manual.push_batch(batch);
        Ok(())
    }

    pub fn promote_for_view(
        &mut self,
        view_job: &ViewJob,
        now: Timestamp,
    ) -> Result<(), EngineError> {
        view_job.validate()?;
        let requested_tier = view_job
            .detail_override
            .unwrap_or(if view_job.requests_history() {
                DetailTier::Sampled
            } else {
                DetailTier::Active
            });

        let default_ring_capacity = self.config.default_ring_capacity;
        let requests_history = view_job.requests_history();

        for entity_id in &view_job.entities {
            let entry = self.ensure_cache_entry(*entity_id, requested_tier, now)?;
            entry.is_visible = true;
            if requests_history {
                entry.detail_tier = DetailTier::Sampled;
                for metric_id in &view_job.metrics {
                    ensure_ring_buffer(entry, *metric_id, default_ring_capacity)?;
                }
            }
        }

        Ok(())
    }

    pub fn tick(&mut self, now: Timestamp) -> Result<TickReport, EngineError> {
        let due_sources: Vec<SourceId> = self
            .schedule
            .iter()
            .filter_map(|(source_id, entry)| {
                self.store
                    .source(*source_id)
                    .filter(|source| source.schedule.is_automatic() && entry.is_due(now))
                    .map(|_| *source_id)
            })
            .collect();

        let mut report = TickReport {
            due_sources: due_sources.len(),
            ..TickReport::default()
        };

        for source_id in due_sources {
            report.polled_sources += 1;
            match self.poll_source(source_id, now) {
                Ok(batch_report) => {
                    self.mark_source_success(source_id, now)?;
                    report.raw_records_seen += batch_report.raw_records_seen;
                    report.samples_seen += batch_report.samples_seen;
                    report.samples_stored += batch_report.samples_stored;
                }
                Err(err) => {
                    self.mark_source_failure_with_reason(
                        source_id,
                        now,
                        self.failure_from_error(source_id, now, &err),
                    )?;
                    return Err(err);
                }
            }
        }

        self.store.prune_all(now)?;
        let evicted = self.evict_expired_cache(now);
        if report.due_sources > 0 || evicted > 0 {
            self.record_event(EngineEvent {
                at: now,
                level: EventLevel::Trace,
                kind: EventKind::WarmStorePruned,
                source_id: None,
                entity_id: None,
                metric_id: None,
                message: format!(
                    "tick complete: due={} polled={} evicted={}",
                    report.due_sources, report.polled_sources, evicted
                ),
            });
        }
        Ok(report)
    }

    pub fn poll_source_now(
        &mut self,
        source_id: SourceId,
        now: Timestamp,
    ) -> Result<TickReport, EngineError> {
        let mut report = match self.poll_source(source_id, now) {
            Ok(report) => report,
            Err(err) => {
                self.mark_source_failure_with_reason(
                    source_id,
                    now,
                    self.failure_from_error(source_id, now, &err),
                )?;
                return Err(err);
            }
        };
        // Manual/operator-driven polls should still count as a real source poll in the
        // returned report, even though they do not come from the automatic scheduler.
        report.polled_sources += 1;
        self.mark_source_success(source_id, now)?;
        self.store.prune_all(now)?;
        let evicted = self.evict_expired_cache(now);
        if evicted > 0 {
            self.record_event(EngineEvent {
                at: now,
                level: EventLevel::Trace,
                kind: EventKind::WarmStorePruned,
                source_id: Some(source_id),
                entity_id: None,
                metric_id: None,
                message: format!("manual poll pruning evicted {} cache entries", evicted),
            });
        }
        Ok(report)
    }

    pub fn materialize_topology(
        &self,
        view_job: &ViewJob,
        now: Timestamp,
    ) -> Result<TopologyViewMaterialization, EngineError> {
        Ok(TopologyMaterializer::build(&self.store, view_job, now)?)
    }

    pub fn materialize_sparse_geo(
        &self,
        view_job: &ViewJob,
        now: Timestamp,
    ) -> Result<SparseGeoViewMaterialization, EngineError> {
        Ok(SparseGeoMaterializer::build(&self.store, view_job, now)?)
    }

    fn entity_class_label_status(
        &self,
        entity_id: EntityId,
    ) -> Option<(EntityClass, String, crate::entity::EntityStatus)> {
        if let Some(node) = self.store.node(entity_id) {
            return Some((EntityClass::Node, node.label.clone(), node.status));
        }
        if let Some(edge) = self.store.edge(entity_id) {
            return Some((
                EntityClass::Edge,
                format!("edge:{}->{}", edge.source, edge.target),
                edge.status,
            ));
        }
        if let Some(boundary) = self.store.boundary(entity_id) {
            return Some((
                EntityClass::Boundary,
                boundary.label.clone(),
                boundary.status,
            ));
        }
        None
    }

    fn entity_tags(&self, entity_id: EntityId) -> Option<&[crate::types::Tag]> {
        if let Some(node) = self.store.node(entity_id) {
            return Some(&node.tags);
        }
        if let Some(edge) = self.store.edge(entity_id) {
            return Some(&edge.tags);
        }
        if let Some(boundary) = self.store.boundary(entity_id) {
            return Some(&boundary.tags);
        }
        None
    }

    fn display_value(value: &crate::metric::SampleValue) -> String {
        match value {
            crate::metric::SampleValue::Numeric(value) => format!("{value}"),
            crate::metric::SampleValue::Code(value) => value.clone(),
            crate::metric::SampleValue::Flag(value) => value.to_string(),
            crate::metric::SampleValue::Missing => "missing".into(),
        }
    }

    pub fn query_latest(
        &self,
        filter: &QueryFilter,
        now: Timestamp,
    ) -> Result<QueryResult, EngineError> {
        filter.validate()?;
        let mut rows = Vec::new();

        for ((entity_id, metric_id), latest) in self.store.latest_values_iter() {
            if !filter.entities.matches_id(entity_id)
                || !filter.matches_metric(metric_id)
                || !filter.matches_source(latest.source_id)
            {
                continue;
            }
            let Some((entity_class, entity_label, entity_status)) =
                self.entity_class_label_status(entity_id)
            else {
                continue;
            };
            if let Some(required_class) = filter.entities.class {
                if required_class != entity_class {
                    continue;
                }
            }
            if filter.only_hot && !entity_status.is_hot() {
                continue;
            }
            if !filter.matches_status(entity_status) {
                continue;
            }
            if !filter.entities.matches_label(&entity_label) {
                continue;
            }
            let Some(entity_tags) = self.entity_tags(entity_id) else {
                continue;
            };
            if !filter.matches_tags(entity_tags) {
                continue;
            }
            if let Some(predicate) = &filter.numeric_predicate {
                match &latest.value {
                    crate::metric::SampleValue::Numeric(value) if predicate.matches(*value) => {}
                    crate::metric::SampleValue::Numeric(_) => continue,
                    _ => continue,
                }
            }
            let Some(metric) = self.store.metric(metric_id) else {
                continue;
            };
            rows.push(QueryRow {
                entity_id,
                entity_label,
                entity_class,
                entity_status,
                metric_id,
                metric_name: metric.name.clone(),
                value: latest.value.clone(),
                display_value: Self::display_value(&latest.value),
                timestamp: latest.timestamp,
                quality: latest.quality,
                source_id: latest.source_id,
                comparison: None,
            });
        }

        rows.sort_by(|a, b| {
            b.timestamp
                .cmp(&a.timestamp)
                .then_with(|| a.entity_label.cmp(&b.entity_label))
                .then_with(|| a.metric_name.cmp(&b.metric_name))
        });
        if let Some(limit) = filter.limit {
            rows.truncate(limit);
        }
        Ok(QueryResult {
            generated_at: now,
            rows,
        })
    }

    pub fn query_request(
        &self,
        request: &QueryRequest,
        now: Timestamp,
    ) -> Result<QueryResultEnvelope, EngineError> {
        request.validate()?;
        let mut base_filter = request.filter.clone();
        let limit = base_filter.limit;
        base_filter.limit = None;
        let mut rows = self.query_latest(&base_filter, now)?.rows;

        if let Some(compare_range) = request.compare_range {
            for row in &mut rows {
                let history = self.store.samples_for_result(
                    row.entity_id,
                    row.metric_id,
                    compare_range,
                    now,
                )?;
                let Some(baseline) = history.last() else {
                    continue;
                };
                let numeric_delta = match (&row.value, &baseline.value) {
                    (
                        crate::metric::SampleValue::Numeric(current),
                        crate::metric::SampleValue::Numeric(previous),
                    ) => Some(*current - *previous),
                    _ => None,
                };
                row.comparison = Some(QueryRowComparison {
                    baseline_timestamp: baseline.ts_observed,
                    baseline_value: baseline.value.clone(),
                    baseline_display_value: Self::display_value(&baseline.value),
                    changed: baseline.value != row.value,
                    numeric_delta,
                });
            }
        }

        if let Some(sort) = request.sort {
            rows.sort_by(|a, b| {
                match sort {
                    QuerySortKey::TimestampDescending => b.timestamp.cmp(&a.timestamp),
                    QuerySortKey::TimestampAscending => a.timestamp.cmp(&b.timestamp),
                    QuerySortKey::EntityLabelAscending => a.entity_label.cmp(&b.entity_label),
                    QuerySortKey::EntityLabelDescending => b.entity_label.cmp(&a.entity_label),
                    QuerySortKey::MetricNameAscending => a.metric_name.cmp(&b.metric_name),
                    QuerySortKey::MetricNameDescending => b.metric_name.cmp(&a.metric_name),
                    QuerySortKey::ValueAscending => compare_row_value(a, b, true),
                    QuerySortKey::ValueDescending => compare_row_value(a, b, false),
                }
                .then_with(|| a.entity_label.cmp(&b.entity_label))
                .then_with(|| a.metric_name.cmp(&b.metric_name))
            });
        }

        if let Some(limit) = limit {
            rows.truncate(limit);
        }

        Ok(QueryResultEnvelope::from_rows(request, now, rows))
    }

    pub fn evaluate_watchlist(
        &self,
        watchlist: &Watchlist,
        now: Timestamp,
    ) -> Result<WatchlistEvaluation, EngineError> {
        watchlist.validate()?;
        let mut alerts = Vec::new();
        let mut missing_items = 0usize;
        for item in &watchlist.items {
            let Some(latest) = self.store.latest_value(item.entity_id, item.metric_id) else {
                missing_items += 1;
                continue;
            };
            if !item.rule.matches(&latest.value) {
                continue;
            }
            let Some((_, entity_label, _)) = self.entity_class_label_status(item.entity_id) else {
                missing_items += 1;
                continue;
            };
            let metric_name = self
                .store
                .metric(item.metric_id)
                .map(|metric| metric.name.clone())
                .unwrap_or_else(|| format!("metric:{}", item.metric_id));
            let display_value = match &latest.value {
                crate::metric::SampleValue::Numeric(value) => format!("{value}"),
                crate::metric::SampleValue::Code(value) => value.clone(),
                crate::metric::SampleValue::Flag(value) => value.to_string(),
                crate::metric::SampleValue::Missing => "missing".into(),
            };
            alerts.push(AlertEvent {
                watch_label: watchlist.label.clone(),
                item_label: item.label.clone(),
                entity_id: item.entity_id,
                entity_label,
                metric_id: item.metric_id,
                metric_name,
                display_value,
                triggered_at: latest.timestamp,
                source_id: latest.source_id,
            });
        }

        Ok(WatchlistEvaluation {
            evaluated_at: now,
            watchlist_label: watchlist.label.clone(),
            checked_items: watchlist.items.len(),
            missing_items,
            alerts,
        })
    }

    pub fn warm_store_maintenance_report(
        &self,
    ) -> Result<Option<WarmStoreMaintenanceReport>, EngineError> {
        Ok(self.store.warm_store_maintenance_report()?)
    }

    pub fn optimize_warm_store(
        &self,
        vacuum: bool,
    ) -> Result<Option<WarmStoreMaintenanceReport>, EngineError> {
        Ok(self.store.optimize_warm_store(vacuum)?)
    }

    pub fn hydrate_data_card(
        &self,
        entity_id: EntityId,
        metrics: &[MetricId],
        now: Timestamp,
        time_range: TimeRange,
    ) -> Result<DataCard, EngineError> {
        let label = self
            .store
            .entity_label(entity_id)
            .unwrap_or_else(|| entity_id.to_string());
        let mut fields = Vec::new();
        let mut history_available = false;

        for metric_id in metrics {
            if let Some(latest) = self.store.latest_value(entity_id, *metric_id) {
                let metric = self
                    .store
                    .metric(*metric_id)
                    .ok_or(EngineError::Store(format!("metric missing: {metric_id}")))?;
                let display_value = sample_value_to_string(&latest.value);
                fields.push(DataCardField {
                    metric_name: metric.name.clone(),
                    display_value,
                    unit: metric.unit.clone(),
                    timestamp: latest.timestamp,
                });

                if !self
                    .store
                    .samples_for_result(entity_id, *metric_id, time_range, now)?
                    .is_empty()
                {
                    history_available = true;
                }
            }
        }

        Ok(DataCard {
            entity_id,
            label,
            kind_label: "entity".into(),
            summary_fields: fields,
            history_available,
        })
    }

    fn poll_source(
        &mut self,
        source_id: SourceId,
        now: Timestamp,
    ) -> Result<TickReport, EngineError> {
        let source = self
            .store
            .source(source_id)
            .cloned()
            .ok_or(EngineError::UnknownSource(source_id))?;
        let adapter = self
            .adapters
            .get_mut(&source_id)
            .ok_or(EngineError::MissingAdapter(source_id))?;
        let adapter_kind = adapter.kind();
        let batch = match adapter.pull(&source, now) {
            Ok(batch) => batch,
            Err(err) => {
                return Err(EngineError::from(err));
            }
        };

        self.ingest_source_pull(source, adapter_kind, batch, now, false)
    }

    fn ingest_replay_pull(
        &mut self,
        source_id: SourceId,
        batch: SourcePull,
        now: Timestamp,
    ) -> Result<TickReport, EngineError> {
        let source = self
            .store
            .source(source_id)
            .cloned()
            .ok_or(EngineError::UnknownSource(source_id))?;
        self.ingest_source_pull(source.clone(), source.adapter, batch, now, true)
    }

    fn ingest_source_pull(
        &mut self,
        source: SourceDefinition,
        adapter_kind: crate::ingest::AdapterKind,
        batch: SourcePull,
        now: Timestamp,
        replay_injected: bool,
    ) -> Result<TickReport, EngineError> {
        let metric_map = self.metric_map();
        batch.validate_against(&metric_map)?;

        let mut report = TickReport {
            raw_records_seen: batch.raw_count(),
            samples_seen: batch.sample_count(),
            ..TickReport::default()
        };
        let topology_count = batch.topology_count();

        for node in &batch.nodes {
            self.store.upsert_node(node.clone())?;
        }
        for boundary in &batch.boundaries {
            self.store.upsert_boundary(boundary.clone())?;
        }
        for edge in &batch.edges {
            self.store.upsert_edge(edge.clone())?;
        }

        self.store.ingest_raw_records_batch(&batch.raw_records)?;
        let sample_outcomes = self.store.ingest_samples_batch(&batch.samples)?;
        for (sample, stored) in batch.samples.into_iter().zip(sample_outcomes) {
            if stored.stored_history {
                report.samples_stored += 1;
            }
            self.update_cache_from_sample(&sample, now)?;

            let mut transform = Vec::with_capacity(4);
            transform.push(if replay_injected {
                TransformStep::ReplayInject
            } else {
                TransformStep::AdapterPull
            });
            transform.push(TransformStep::Normalize);
            transform.push(TransformStep::LatestStateUpdate);
            if stored.stored_history {
                transform.push(TransformStep::HistoryStore);
            }

            let provenance = SampleProvenance {
                source_id: source.id,
                source_name: source.name.clone(),
                adapter_kind,
                endpoint: source.endpoint.clone(),
                auth_ref: source.auth_ref.clone(),
                retrieved_at: now,
                source_timestamp: Some(sample.ts_observed),
                transform,
            };
            self.audit.push(AuditRecord::Sample(SampleAuditRecord {
                at: now,
                entity_id: sample.entity_id,
                metric_id: sample.metric_id,
                stored_history: stored.stored_history,
                provenance,
            }));
        }

        self.record_event(EngineEvent {
            at: now,
            level: EventLevel::Info,
            kind: EventKind::SourcePollSuccess,
            source_id: Some(source.id),
            entity_id: None,
            metric_id: None,
            message: format!(
                "source ingest succeeded: raw={} samples={} stored={} topology={} replay={}",
                report.raw_records_seen,
                report.samples_seen,
                report.samples_stored,
                topology_count,
                replay_injected
            ),
        });

        Ok(report)
    }

    fn metric_map(&self) -> HashMap<MetricId, MetricDefinition> {
        self.store
            .metric_ids()
            .into_iter()
            .filter_map(|id| self.store.metric(id).cloned().map(|metric| (id, metric)))
            .collect()
    }

    fn update_cache_from_sample(
        &mut self,
        sample: &Sample,
        now: Timestamp,
    ) -> Result<(), EngineError> {
        let hot_duration = self
            .store
            .metric(sample.metric_id)
            .ok_or_else(|| EngineError::Store(format!("unknown metric {}", sample.metric_id)))?
            .retention
            .hot_duration;
        let latest = LatestValue::from_sample(sample);

        let entry = self.ensure_cache_entry(sample.entity_id, DetailTier::Active, now)?;
        entry.ttl = hot_duration;
        entry.touch(now);
        entry.upsert_latest(latest);

        if let Some(buffer) = entry
            .ring_buffers
            .iter_mut()
            .find(|buffer| buffer.metric_id == sample.metric_id)
        {
            buffer.push(sample.clone());
        }
        Ok(())
    }

    fn ensure_cache_entry(
        &mut self,
        entity_id: EntityId,
        detail_tier: DetailTier,
        now: Timestamp,
    ) -> Result<&mut CacheEntry, EngineError> {
        if !self.cache_entries.contains_key(&entity_id)
            && self.cache_entries.len() >= self.config.cache_budget.max_active_entities
        {
            self.evict_one_for_capacity();
        }

        let ttl = self.config.default_entity_ttl;
        let entry = self
            .cache_entries
            .entry(entity_id)
            .or_insert(CacheEntry::new(entity_id, detail_tier, ttl, now)?);
        if entry.detail_tier < detail_tier {
            entry.detail_tier = detail_tier;
        }
        Ok(entry)
    }

    fn evict_expired_cache(&mut self, now: Timestamp) -> usize {
        let before = self.cache_entries.len();
        self.cache_entries.retain(|_, entry| {
            if matches!(entry.eviction_policy, crate::cache::EvictionPolicy::Pinned) {
                return true;
            }
            let age = now.signed_duration_since(entry.last_accessed);
            let Ok(age) = age.to_std() else {
                return true;
            };
            age <= entry.ttl
        });
        before.saturating_sub(self.cache_entries.len())
    }

    fn evict_one_for_capacity(&mut self) {
        if let Some(candidate) = self
            .cache_entries
            .iter()
            .filter(|(_, entry)| !entry.is_selected && !entry.is_visible)
            .min_by_key(|(_, entry)| (entry.priority, entry.last_accessed))
            .map(|(entity_id, _)| *entity_id)
        {
            self.cache_entries.remove(&candidate);
            self.record_event(EngineEvent {
                at: chrono::Utc::now(),
                level: EventLevel::Warn,
                kind: EventKind::CacheEvicted,
                source_id: None,
                entity_id: Some(candidate),
                metric_id: None,
                message: "evicted cache entry to satisfy active-entity budget".into(),
            });
        }
    }

    fn mark_source_success(
        &mut self,
        source_id: SourceId,
        now: Timestamp,
    ) -> Result<(), EngineError> {
        let schedule = {
            let source = self
                .store
                .source_mut(source_id)
                .ok_or(EngineError::UnknownSource(source_id))?;
            source.health = SourceHealth::Healthy;
            source.last_polled = Some(now);
            source.last_error = None;
            source.schedule
        };

        let entry = self
            .schedule
            .get_mut(&source_id)
            .ok_or(EngineError::UnknownSource(source_id))?;
        entry.mark_success(schedule, now)?;
        self.record_event(EngineEvent {
            at: now,
            level: EventLevel::Trace,
            kind: EventKind::SourcePollSuccess,
            source_id: Some(source_id),
            entity_id: None,
            metric_id: None,
            message: "source marked healthy".into(),
        });
        Ok(())
    }

    fn mark_source_failure_with_reason(
        &mut self,
        source_id: SourceId,
        now: Timestamp,
        failure: FailureRecord,
    ) -> Result<(), EngineError> {
        let (backoff, max_backoff) = {
            let source = self
                .store
                .source_mut(source_id)
                .ok_or(EngineError::UnknownSource(source_id))?;
            source.health = SourceHealth::Degraded;
            source.last_error = Some(now);
            (source.backoff, source.max_backoff)
        };

        let entry = self
            .schedule
            .get_mut(&source_id)
            .ok_or(EngineError::UnknownSource(source_id))?;
        entry.mark_failure(backoff, max_backoff, now)?;
        let detail = failure.detail.clone();
        self.record_failure(failure);
        self.record_event(EngineEvent {
            at: now,
            level: EventLevel::Error,
            kind: EventKind::SourcePollFailure,
            source_id: Some(source_id),
            entity_id: None,
            metric_id: None,
            message: format!("source failure triggered backoff {:?}: {}", backoff, detail),
        });
        Ok(())
    }

    pub fn record_export_audit(
        &mut self,
        result: &crate::export::SnapshotExportResult,
        job: &crate::export::SnapshotExportJob,
        now: Timestamp,
    ) {
        self.audit.push(AuditRecord::Export(ExportAuditRecord {
            at: now,
            snapshot_id: result.manifest.id,
            format: result.manifest.format,
            view_kind: job.view.kind,
            output_path: result.output_path.to_string_lossy().to_string(),
            entity_count: result.manifest.entity_count,
            metric_count: result.manifest.metric_count,
            size_bytes: result.manifest.size_bytes,
        }));
    }

    fn record_failure(&mut self, failure: FailureRecord) {
        const MAX_FAILURES: usize = 512;
        if self.failures.len() >= MAX_FAILURES {
            self.failures.remove(0);
        }
        self.failures.push(failure);
    }

    fn failure_from_error(
        &self,
        source_id: SourceId,
        now: Timestamp,
        error: &EngineError,
    ) -> FailureRecord {
        match error {
            EngineError::Adapter(msg) => {
                let lower = msg.to_ascii_lowercase();
                let adapter_error = if lower.contains("parse") {
                    AdapterError::Parse(msg.clone())
                } else if lower.contains("unsupported") {
                    AdapterError::Unsupported(msg.clone())
                } else if lower.contains("validation") {
                    AdapterError::Validation(msg.clone())
                } else {
                    AdapterError::Io(msg.clone())
                };
                FailureRecord::from_adapter_error(source_id, now, &adapter_error)
            }
            EngineError::Policy(msg) => FailureRecord::policy(source_id, now, msg.clone()),
            EngineError::Store(msg) | EngineError::Validation(msg) => {
                FailureRecord::storage(Some(source_id), now, msg.clone())
            }
            EngineError::UnknownSource(id)
            | EngineError::MissingAdapter(id)
            | EngineError::WrongAdapterKind(id) => {
                FailureRecord::storage(Some(*id), now, error.to_string())
            }
        }
    }

    fn record_event(&mut self, event: EngineEvent) {
        self.events.push(event);
    }
}

fn ensure_ring_buffer(
    entry: &mut CacheEntry,
    metric_id: MetricId,
    capacity: usize,
) -> Result<(), EngineError> {
    if entry
        .ring_buffers
        .iter()
        .any(|buffer| buffer.metric_id == metric_id)
    {
        return Ok(());
    }
    entry
        .ring_buffers
        .push(RingBuffer::new(entry.entity_id, metric_id, capacity)?);
    Ok(())
}

fn sample_value_to_string(value: &crate::metric::SampleValue) -> String {
    match value {
        crate::metric::SampleValue::Numeric(v) => format!("{v}"),
        crate::metric::SampleValue::Code(v) => v.clone(),
        crate::metric::SampleValue::Flag(v) => v.to_string(),
        crate::metric::SampleValue::Missing => "missing".into(),
    }
}
