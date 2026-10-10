// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! View and render contracts.
//!
//! A `ViewJob` asks the engine for a slice of truth. Nothing renders unless
//! a view requests it. Views may influence hot-cache priority, but they do
//! not define the underlying data model.

use serde::{Deserialize, Serialize};

use crate::cache::DetailTier;
use crate::spatial::GeoBBox;
use crate::types::{EntityId, MetricId, Timestamp, ValidationError, ViewJobId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ViewKind {
    Topology,
    SparseGeo,
    Timeline,
    DataCard,
    Compare,
    SnapshotExport,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum TimeRange {
    LatestOnly,
    Since(Timestamp),
    Window { start: Timestamp, end: Timestamp },
}

impl TimeRange {
    pub fn validate(self) -> Result<(), ValidationError> {
        if let Self::Window { start, end } = self {
            if end < start {
                return Err(ValidationError::InvalidWindow {
                    start_field: "time_range.start".into(),
                    end_field: "time_range.end".into(),
                });
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn contains(self, timestamp: Timestamp, now: Timestamp) -> bool {
        match self {
            Self::LatestOnly => true,
            Self::Since(start) => timestamp >= start && timestamp <= now,
            Self::Window { start, end } => timestamp >= start && timestamp <= end,
        }
    }

    #[must_use]
    pub fn requests_history(self) -> bool {
        !matches!(self, Self::LatestOnly)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewJob {
    pub id: ViewJobId,
    pub kind: ViewKind,
    pub entities: Vec<EntityId>,
    pub metrics: Vec<MetricId>,
    pub time_range: TimeRange,
    pub detail_override: Option<DetailTier>,
    pub viewport: Option<GeoBBox>,
}

impl ViewJob {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.time_range.validate()?;
        if matches!(self.kind, ViewKind::SparseGeo) && self.viewport.is_none() {
            return Err(ValidationError::InvalidState(
                "sparse geo views must carry a viewport".into(),
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn requests_history(&self) -> bool {
        self.time_range.requests_history()
            || matches!(self.kind, ViewKind::Timeline | ViewKind::Compare)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RenderRule {
    SkipInvisible,
    MinimalLowPriority,
    DetailSelected,
    PromoteAlerting,
    FadeStale,
    PinForCompare,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataCard {
    pub entity_id: EntityId,
    pub label: String,
    pub kind_label: String,
    pub summary_fields: Vec<DataCardField>,
    pub history_available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataCardField {
    pub metric_name: String,
    pub display_value: String,
    pub unit: String,
    pub timestamp: Timestamp,
}
