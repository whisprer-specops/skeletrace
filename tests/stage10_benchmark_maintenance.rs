// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

use std::time::Duration;

use chrono::Utc;
use skeletrace::{
    AdapterKind, AdapterProfile, Confidence, EngineConfig, EngineProfile, EngineStore, EntityId,
    EntityStatus, FlowId, GeoCoord, InterpolationMethod, MetricDefinition, MetricId,
    MetricValueType, OperatorApi, OperatorRequest, OperatorResponse, PollCadence, Priority,
    Quality, RawIngestRecord, ReplayBatch, ReplayBenchmarkRequest, ReplayWorkloadRequest,
    RetentionPolicy, Sample, SampleValue, SnapshotExporter, SourceDefinition, SourceHealth,
    SourceId, SourceKind, SourceProfile, SourcePull, SourceSchedule, Tag, TimeRange, ViewJob,
    ViewJobId, ViewKind,
};

fn manual_source(source_id: SourceId) -> SourceDefinition {
    SourceDefinition {
        id: source_id,
        name: format!("manual-{source_id}"),
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
        tags: vec![Tag::new("env", "test").unwrap()],
    }
}

fn numeric_metric(metric_id: MetricId, source_id: SourceId, name: &str) -> MetricDefinition {
    MetricDefinition {
        id: metric_id,
        name: name.into(),
        unit: "units".into(),
        value_type: MetricValueType::Numeric,
        cadence: PollCadence::Manual,
        interpolation: InterpolationMethod::StepForward,
        retention: RetentionPolicy {
            hot_duration: Duration::from_secs(300),
            warm_duration: Duration::from_secs(1800),
            store_on_change_only: false,
            change_threshold: None,
            relative_change_threshold: None,
            max_silent_gap: Duration::from_secs(300),
        },
        source_ids: vec![source_id],
        show_in_popup: true,
        popup_priority: 1,
        description: name.into(),
    }
}

fn node(entity_id: EntityId, now: chrono::DateTime<chrono::Utc>) -> skeletrace::Node {
    skeletrace::Node {
        id: entity_id,
        kind: skeletrace::NodeKind::Identity,
        label: "alpha".into(),
        position: Some(GeoCoord::new(51.5, -0.1, None).unwrap()),
        position_confidence: Confidence::new(0.95).unwrap(),
        status: EntityStatus::Active,
        priority: Priority::HIGH,
        tags: vec![Tag::new("role", "source").unwrap()],
        first_seen: now,
        last_seen: now,
    }
}

fn sample(
    entity_id: EntityId,
    metric_id: MetricId,
    source_id: SourceId,
    now: chrono::DateTime<chrono::Utc>,
    value: f64,
) -> Sample {
    Sample {
        entity_id,
        metric_id,
        ts_observed: now,
        ts_ingested: now,
        value: SampleValue::Numeric(value),
        quality: Quality::new(1.0).unwrap(),
        source_id,
    }
}

fn profile_with_manual_source(
    name: &str,
    config: EngineConfig,
    source_id: SourceId,
    metric_id: MetricId,
    entity_id: EntityId,
    now: chrono::DateTime<chrono::Utc>,
) -> EngineProfile {
    EngineProfile {
        name: name.into(),
        config,
        metrics: vec![numeric_metric(metric_id, source_id, "volume")],
        nodes: vec![node(entity_id, now)],
        edges: vec![],
        boundaries: vec![],
        sources: vec![SourceProfile {
            definition: manual_source(source_id),
            adapter_profile: AdapterProfile::ManualPush,
        }],
    }
}

#[test]
fn benchmark_replay_workload_uses_fresh_profile_iterations() {
    let now = Utc::now();
    let source_id = SourceId::new();
    let metric_id = MetricId::new();
    let entity_id = EntityId::new();

    let temp_dir = std::env::temp_dir().join(format!("skeletrace-stage10-bench-{}", FlowId::new()));
    std::fs::create_dir_all(&temp_dir).unwrap();
    let profile = profile_with_manual_source(
        "bench-profile",
        EngineConfig::default(),
        source_id,
        metric_id,
        entity_id,
        now,
    );

    let fixture = skeletrace::WorkloadFixture {
        label: "bench-fixture".into(),
        batches: vec![ReplayBatch {
            source_id,
            due_at: now,
            pull: SourcePull {
                raw_records: vec![],
                samples: vec![sample(entity_id, metric_id, source_id, now, 7.0)],
                touched_entities: vec![entity_id],
                nodes: vec![],
                edges: vec![],
                boundaries: vec![],
            },
        }],
    };

    let exporter =
        SnapshotExporter::new(temp_dir.join("exports"), None::<std::path::PathBuf>).unwrap();
    let mut api = OperatorApi::from_profile(&profile, exporter, now).unwrap();

    let request = ReplayBenchmarkRequest {
        label: "bench-run".into(),
        iterations: 3,
        workload: ReplayWorkloadRequest {
            label: "bench-workload".into(),
            checkpoints: vec![now],
            profile_views: vec![skeletrace::ViewProfileTarget {
                view: ViewJob {
                    id: ViewJobId::new(),
                    kind: ViewKind::Topology,
                    entities: vec![entity_id],
                    metrics: vec![metric_id],
                    time_range: TimeRange::LatestOnly,
                    detail_override: None,
                    viewport: None,
                },
                config: skeletrace::PerfProbeConfig { iterations: 1 },
            }],
        },
    };

    let response = api
        .execute(OperatorRequest::BenchmarkReplayWorkload {
            fixture,
            request,
            now,
        })
        .unwrap();

    let report = match response {
        OperatorResponse::ReplayBenchmark(report) => report,
        other => panic!("unexpected response: {other:?}"),
    };

    assert_eq!(report.iterations, 3);
    assert_eq!(report.stats.runs, 3);
    assert_eq!(report.total_checkpoints, 3);
    assert_eq!(report.total_samples_seen, 3);
    assert_eq!(report.total_samples_stored, 3);
    assert_eq!(report.source_summaries.len(), 1);
    assert_eq!(report.source_summaries[0].scheduled_pulls, 1);
    assert_eq!(report.source_summaries[0].samples, 1);
    assert!(report.last_run.is_some());
}

#[test]
fn warm_store_report_and_optimize_are_operator_visible() {
    let now = Utc::now();
    let source_id = SourceId::new();
    let metric_id = MetricId::new();
    let entity_id = EntityId::new();

    let temp_dir = std::env::temp_dir().join(format!("skeletrace-stage10-warm-{}", FlowId::new()));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let config = EngineConfig {
        warm_store_path: Some(temp_dir.join("warm.sqlite")),
        ..EngineConfig::default()
    };

    let profile =
        profile_with_manual_source("warm-profile", config, source_id, metric_id, entity_id, now);

    let exporter =
        SnapshotExporter::new(temp_dir.join("exports"), None::<std::path::PathBuf>).unwrap();
    let mut api = OperatorApi::from_profile(&profile, exporter, now).unwrap();

    api.engine_mut()
        .enqueue_manual_batch(
            source_id,
            SourcePull {
                raw_records: vec![],
                samples: vec![sample(entity_id, metric_id, source_id, now, 11.0)],
                touched_entities: vec![entity_id],
                nodes: vec![],
                edges: vec![],
                boundaries: vec![],
            },
        )
        .unwrap();
    api.engine_mut().poll_source_now(source_id, now).unwrap();

    let report = match api.execute(OperatorRequest::WarmStoreReport).unwrap() {
        OperatorResponse::WarmStoreReport(Some(report)) => report,
        other => panic!("unexpected response: {other:?}"),
    };
    assert!(report.sample_count >= 1);
    assert!(report.page_count >= 1);
    assert!(report.page_size >= 1);
    assert!(report.index_count >= 1);

    let optimized = match api
        .execute(OperatorRequest::OptimizeWarmStore { vacuum: false })
        .unwrap()
    {
        OperatorResponse::WarmStoreReport(Some(report)) => report,
        other => panic!("unexpected response: {other:?}"),
    };
    assert!(optimized.optimize_ran);
    assert!(!optimized.vacuum_ran);
    assert!(optimized.sample_count >= 1);
}

#[test]
fn store_batch_ingest_reports_counts() {
    let now = Utc::now();
    let source_id = SourceId::new();
    let metric_id = MetricId::new();
    let entity_id = EntityId::new();

    let mut store = EngineStore::new();
    store.register_source(manual_source(source_id)).unwrap();
    store
        .register_metric(numeric_metric(metric_id, source_id, "volume"))
        .unwrap();

    let raw = RawIngestRecord {
        source_id,
        source_timestamp: Some(now),
        ingested_at: now,
        payload: serde_json::json!({"kind": "test"}),
    };
    let report = store
        .ingest_batch(&[raw], &[sample(entity_id, metric_id, source_id, now, 3.0)])
        .unwrap();

    assert_eq!(report.raw_records_seen, 1);
    assert_eq!(report.raw_records_written, 1);
    assert_eq!(report.samples_seen, 1);
    assert_eq!(report.samples_stored, 1);
    assert_eq!(report.latest_updates, 1);
    assert_eq!(store.stats().latest_value_count, 1);
}
