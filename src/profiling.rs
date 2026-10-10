// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Runtime health and low-overhead profiling helpers.

use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::cache::CacheEntry;
use crate::engine::EngineError;
use crate::ingest::SourceHealth;
use crate::materialize::{SparseGeoViewMaterialization, TopologyViewMaterialization};
use crate::observability::EventCounts;
use crate::store::StoreStats;
use crate::types::{Timestamp, ValidationError};
use crate::view::{ViewJob, ViewKind};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PerfProbeConfig {
    pub iterations: u32,
}

impl PerfProbeConfig {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.iterations == 0 {
            return Err(ValidationError::ZeroCapacity(
                "perf_probe.iterations".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct DurationStats {
    pub runs: u32,
    pub total_nanos: u128,
    pub min_nanos: u128,
    pub max_nanos: u128,
}

impl DurationStats {
    pub fn record(&mut self, elapsed: std::time::Duration) {
        let nanos = elapsed.as_nanos();
        if self.runs == 0 || nanos < self.min_nanos {
            self.min_nanos = nanos;
        }
        if nanos > self.max_nanos {
            self.max_nanos = nanos;
        }
        self.total_nanos += nanos;
        self.runs = self.runs.saturating_add(1);
    }

    #[must_use]
    pub fn mean_nanos(&self) -> u128 {
        if self.runs == 0 {
            0
        } else {
            self.total_nanos / u128::from(self.runs)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerfProbeReport {
    pub label: String,
    pub iterations: u32,
    pub stats: DurationStats,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct CacheHealthReport {
    pub active_entities: usize,
    pub visible_entities: usize,
    pub selected_entities: usize,
    pub alerting_entities: usize,
    pub total_ring_buffers: usize,
    pub total_ring_samples: usize,
    pub approx_hot_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceHealthCount {
    pub health: SourceHealth,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineHealthReport {
    pub generated_at: Timestamp,
    pub store_stats: StoreStats,
    pub cache: CacheHealthReport,
    pub source_health_counts: Vec<SourceHealthCount>,
    pub due_sources: usize,
    pub event_counts: EventCounts,
}

pub fn cache_health<'a>(entries: impl Iterator<Item = &'a CacheEntry>) -> CacheHealthReport {
    let mut report = CacheHealthReport::default();
    for entry in entries {
        report.active_entities += 1;
        if entry.is_visible {
            report.visible_entities += 1;
        }
        if entry.is_selected {
            report.selected_entities += 1;
        }
        if entry.is_alerting {
            report.alerting_entities += 1;
        }
        report.total_ring_buffers += entry.ring_buffers.len();
        report.total_ring_samples += entry.ring_buffers.iter().map(|b| b.len()).sum::<usize>();
        report.approx_hot_bytes += entry.approx_hot_bytes();
    }
    report
}

pub fn profile_topology<F>(
    view: &ViewJob,
    iterations: u32,
    mut materialize: F,
) -> Result<PerfProbeReport, EngineError>
where
    F: FnMut() -> Result<TopologyViewMaterialization, EngineError>,
{
    view.validate()?;
    let mut stats = DurationStats::default();
    for _ in 0..iterations {
        let started = Instant::now();
        let mat = materialize()?;
        std::hint::black_box(mat);
        stats.record(started.elapsed());
    }
    Ok(PerfProbeReport {
        label: format!("topology:{}", view.id),
        iterations,
        stats,
    })
}

pub fn profile_sparse_geo<F>(
    view: &ViewJob,
    iterations: u32,
    mut materialize: F,
) -> Result<PerfProbeReport, EngineError>
where
    F: FnMut() -> Result<SparseGeoViewMaterialization, EngineError>,
{
    view.validate()?;
    let mut stats = DurationStats::default();
    for _ in 0..iterations {
        let started = Instant::now();
        let mat = materialize()?;
        std::hint::black_box(mat);
        stats.record(started.elapsed());
    }
    Ok(PerfProbeReport {
        label: format!("sparse-geo:{}", view.id),
        iterations,
        stats,
    })
}

pub fn profile_view_kind(view: &ViewJob) -> Result<ViewKind, ValidationError> {
    view.validate()?;
    Ok(view.kind)
}
