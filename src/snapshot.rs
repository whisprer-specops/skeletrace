// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Snapshot types — selective investigation artifacts.

use serde::{Deserialize, Serialize};

use crate::types::{EntityId, MetricId, SnapshotId, Timestamp, ValidationError};
use crate::view::TimeRange;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ExportFormat {
    GeoJson,
    NativeJson,
    Csv,
}

impl ExportFormat {
    #[must_use]
    pub const fn file_extension(self) -> &'static str {
        match self {
            Self::GeoJson => "geojson",
            Self::NativeJson => "json",
            Self::Csv => "csv",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotRequest {
    pub entities: Vec<EntityId>,
    pub metrics: Vec<MetricId>,
    pub time_range: TimeRange,
    pub format: ExportFormat,
    pub notes: Option<String>,
}

impl SnapshotRequest {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.time_range.validate()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotManifest {
    pub id: SnapshotId,
    pub created_at: Timestamp,
    pub entity_count: usize,
    pub metric_count: usize,
    pub sample_count: usize,
    pub time_range: TimeRange,
    pub format: ExportFormat,
    pub size_bytes: u64,
    pub notes: Option<String>,
    pub storage_path: String,
}

impl SnapshotManifest {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.storage_path.trim().is_empty() {
            return Err(ValidationError::EmptyField("snapshot.storage_path".into()));
        }
        self.time_range.validate()
    }
}
