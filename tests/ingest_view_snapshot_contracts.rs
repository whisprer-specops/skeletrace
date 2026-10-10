// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

use std::time::Duration;

use chrono::{TimeDelta, Utc};
use skeletrace::{
    AdapterKind, CacheBudget, DetailTier, EntityId, ExportFormat, GeoBBox, MetricId, SnapshotId,
    SnapshotManifest, SnapshotRequest, SourceDefinition, SourceHealth, SourceId, SourceKind,
    SourceSchedule, TimeRange, ValidationError, ViewJob, ViewJobId, ViewKind,
};

#[test]
fn source_schedule_rejects_zero_interval() {
    assert!(matches!(
        SourceSchedule::Fixed(Duration::ZERO).validate(),
        Err(ValidationError::ZeroCapacity(_))
    ));
}

#[test]
fn manual_sources_may_omit_endpoint() {
    let source = SourceDefinition {
        id: SourceId::new(),
        name: "manual analyst input".into(),
        kind: SourceKind::Manual,
        adapter: AdapterKind::Manual,
        schedule: SourceSchedule::Manual,
        endpoint: String::new(),
        auth_ref: None,
        health: SourceHealth::Pending,
        last_polled: None,
        last_error: None,
        backoff: Duration::from_secs(5),
        max_backoff: Duration::from_secs(30),
        tags: vec![],
    };

    assert!(source.validate().is_ok());
}

#[test]
fn source_definition_rejects_inverted_backoff_window() {
    let source = SourceDefinition {
        id: SourceId::new(),
        name: "feed".into(),
        kind: SourceKind::Api,
        adapter: AdapterKind::HttpPoller,
        schedule: SourceSchedule::Fixed(Duration::from_secs(30)),
        endpoint: "https://example.invalid/feed".into(),
        auth_ref: None,
        health: SourceHealth::Healthy,
        last_polled: None,
        last_error: None,
        backoff: Duration::from_secs(60),
        max_backoff: Duration::from_secs(10),
        tags: vec![],
    };

    assert!(matches!(
        source.validate(),
        Err(ValidationError::InvalidWindow { .. })
    ));
}

#[test]
fn time_range_contains_and_history_flags_behave() {
    let now = Utc::now();
    let start = now - TimeDelta::minutes(5);
    let end = now + TimeDelta::minutes(5);

    assert!(TimeRange::LatestOnly.contains(now, now));
    assert!(!TimeRange::LatestOnly.requests_history());

    let since = TimeRange::Since(start);
    assert!(since.contains(now, now));
    assert!(since.requests_history());

    let window = TimeRange::Window { start, end };
    assert!(window.contains(now, now));
    assert!(window.requests_history());
}

#[test]
fn sparse_geo_view_requires_viewport() {
    let job = ViewJob {
        id: ViewJobId::new(),
        kind: ViewKind::SparseGeo,
        entities: vec![EntityId::new()],
        metrics: vec![MetricId::new()],
        time_range: TimeRange::LatestOnly,
        detail_override: Some(DetailTier::Skeleton),
        viewport: None,
    };

    assert!(matches!(
        job.validate(),
        Err(ValidationError::InvalidState(_))
    ));

    let valid_job = ViewJob {
        viewport: Some(GeoBBox::new(50.0, -5.0, 60.0, 2.0).unwrap()),
        ..job
    };
    assert!(valid_job.validate().is_ok());
}

#[test]
fn compare_and_timeline_views_imply_history_requests() {
    let job = ViewJob {
        id: ViewJobId::new(),
        kind: ViewKind::Compare,
        entities: vec![EntityId::new(), EntityId::new()],
        metrics: vec![MetricId::new()],
        time_range: TimeRange::LatestOnly,
        detail_override: None,
        viewport: None,
    };

    assert!(job.requests_history());
}

#[test]
fn snapshot_export_extensions_and_manifest_validation_hold() {
    assert_eq!(ExportFormat::GeoJson.file_extension(), "geojson");
    assert_eq!(ExportFormat::NativeJson.file_extension(), "json");
    assert_eq!(ExportFormat::Csv.file_extension(), "csv");

    let request = SnapshotRequest {
        entities: vec![EntityId::new()],
        metrics: vec![MetricId::new()],
        time_range: TimeRange::Window {
            start: Utc::now() - TimeDelta::minutes(10),
            end: Utc::now(),
        },
        format: ExportFormat::NativeJson,
        notes: Some("test snapshot".into()),
    };
    assert!(request.validate().is_ok());

    let manifest = SnapshotManifest {
        id: SnapshotId::new(),
        created_at: Utc::now(),
        entity_count: 3,
        metric_count: 7,
        sample_count: 42,
        time_range: request.time_range,
        format: request.format,
        size_bytes: 2048,
        notes: request.notes,
        storage_path: String::new(),
    };

    assert!(matches!(
        manifest.validate(),
        Err(ValidationError::EmptyField(_))
    ));
}

#[test]
fn cache_budget_rejects_zero_limits() {
    let budget = CacheBudget {
        max_active_entities: 1,
        max_total_ring_samples: 1,
        max_highres_entities: 1,
        max_approx_hot_bytes: 1,
        max_per_entity_hot_bytes: 0,
    };

    assert!(matches!(
        budget.validate(),
        Err(ValidationError::ZeroCapacity(_))
    ));
}
