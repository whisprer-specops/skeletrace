// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

use std::time::Duration;

use chrono::{TimeDelta, Utc};
use skeletrace::{
    AdapterKind, AuditRecord, Confidence, EngineConfig, EntityId, EntityStatus, FlowId, GeoCoord,
    InterpolationMethod, ManualPushAdapter, MetricDefinition, MetricId, MetricValueType,
    OperatorApi, OperatorRequest, OperatorResponse, PollCadence, Priority, Quality, ReplayBatch,
    ReplayWorkloadRequest, RetentionPolicy, Sample, SampleValue, SkeletraceEngine,
    SnapshotExporter, SourceDefinition, SourceHealth, SourceId, SourceKind, SourcePull,
    SourceSchedule, Tag, TimeRange, TransformStep, ViewJob, ViewJobId, ViewKind, ViewProfileTarget,
    WorkloadFixture,
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

#[test]
fn replay_ready_batches_ingests_fixture_and_records_replay_provenance() {
    let now = Utc::now();
    let source_id = SourceId::new();
    let metric_id = MetricId::new();
    let entity_id = EntityId::new();

    let mut engine = SkeletraceEngine::new(EngineConfig::default()).unwrap();
    engine
        .register_metric(numeric_metric(metric_id, source_id, "volume"))
        .unwrap();
    engine.register_node(node(entity_id, now)).unwrap();
    engine
        .register_source(
            manual_source(source_id),
            Box::new(ManualPushAdapter::new()),
            now,
        )
        .unwrap();

    let fixture = WorkloadFixture {
        label: "replay-one".into(),
        batches: vec![ReplayBatch {
            source_id,
            due_at: now,
            pull: SourcePull {
                raw_records: vec![],
                samples: vec![sample(entity_id, metric_id, source_id, now, 42.0)],
                touched_entities: vec![entity_id],
                nodes: vec![],
                edges: vec![],
                boundaries: vec![],
            },
        }],
    };

    let mut harness = fixture.into_harness();
    let report = engine.replay_ready_batches(&mut harness, now).unwrap();
    assert_eq!(report.ready_sources, 1);
    assert_eq!(report.pulls_processed, 1);
    assert_eq!(report.samples_seen, 1);
    assert_eq!(report.samples_stored, 1);
    assert!(harness.is_empty());

    let audit = engine.recent_audit_records(8);
    let sample_audit = audit
        .into_iter()
        .find_map(|record| match record {
            AuditRecord::Sample(record) => Some(record),
            _ => None,
        })
        .unwrap();
    assert!(sample_audit
        .provenance
        .transform
        .contains(&TransformStep::ReplayInject));
}

#[test]
fn replay_workload_report_tracks_checkpoints_and_view_profiles() {
    let now = Utc::now();
    let t1 = now;
    let t2 = now + TimeDelta::seconds(30);
    let source_id = SourceId::new();
    let metric_id = MetricId::new();
    let entity_id = EntityId::new();

    let mut engine = SkeletraceEngine::new(EngineConfig::default()).unwrap();
    engine
        .register_metric(numeric_metric(metric_id, source_id, "volume"))
        .unwrap();
    engine.register_node(node(entity_id, now)).unwrap();
    engine
        .register_source(
            manual_source(source_id),
            Box::new(ManualPushAdapter::new()),
            now,
        )
        .unwrap();

    let fixture = WorkloadFixture {
        label: "two-step".into(),
        batches: vec![
            ReplayBatch {
                source_id,
                due_at: t1,
                pull: SourcePull {
                    raw_records: vec![],
                    samples: vec![sample(entity_id, metric_id, source_id, t1, 10.0)],
                    touched_entities: vec![entity_id],
                    nodes: vec![],
                    edges: vec![],
                    boundaries: vec![],
                },
            },
            ReplayBatch {
                source_id,
                due_at: t2,
                pull: SourcePull {
                    raw_records: vec![],
                    samples: vec![sample(entity_id, metric_id, source_id, t2, 20.0)],
                    touched_entities: vec![entity_id],
                    nodes: vec![],
                    edges: vec![],
                    boundaries: vec![],
                },
            },
        ],
    };
    let mut harness = fixture.into_harness();

    let request = ReplayWorkloadRequest {
        label: "workload-a".into(),
        checkpoints: vec![t1, t2],
        profile_views: vec![ViewProfileTarget {
            view: ViewJob {
                id: ViewJobId::new(),
                kind: ViewKind::Topology,
                entities: vec![entity_id],
                metrics: vec![metric_id],
                time_range: TimeRange::LatestOnly,
                detail_override: None,
                viewport: None,
            },
            config: skeletrace::PerfProbeConfig { iterations: 2 },
        }],
    };

    let report = engine.run_replay_workload(&mut harness, &request).unwrap();
    assert_eq!(report.checkpoints.len(), 2);
    assert_eq!(report.checkpoints[0].replay.samples_seen, 1);
    assert_eq!(report.checkpoints[1].replay.samples_seen, 1);
    assert_eq!(report.view_profiles.len(), 1);
    assert_eq!(report.view_profiles[0].stats.runs, 2);
    assert!(report.audit_delta >= 2);
    assert_eq!(report.failure_delta, 0);
    assert!(report.health_after.store_stats.latest_value_count >= 1);
}

#[test]
fn workload_fixture_round_trips_and_operator_runs_workload() {
    let now = Utc::now();
    let source_id = SourceId::new();
    let metric_id = MetricId::new();
    let entity_id = EntityId::new();

    let mut engine = SkeletraceEngine::new(EngineConfig::default()).unwrap();
    engine
        .register_metric(numeric_metric(metric_id, source_id, "volume"))
        .unwrap();
    engine.register_node(node(entity_id, now)).unwrap();
    engine
        .register_source(
            manual_source(source_id),
            Box::new(ManualPushAdapter::new()),
            now,
        )
        .unwrap();

    let fixture = WorkloadFixture {
        label: "json-fixture".into(),
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

    let temp_dir = std::env::temp_dir().join(format!("skeletrace-stage9-{}", FlowId::new()));
    std::fs::create_dir_all(&temp_dir).unwrap();
    let fixture_path = temp_dir.join("fixture.json");
    fixture.save_json(&fixture_path).unwrap();
    let loaded = WorkloadFixture::load_json(&fixture_path).unwrap();
    assert_eq!(loaded.label, fixture.label);
    assert_eq!(loaded.batches.len(), 1);

    let exporter =
        SnapshotExporter::new(temp_dir.join("exports"), Option::<std::path::PathBuf>::None)
            .unwrap();
    let mut api = OperatorApi::new(engine, exporter);

    let response = api
        .execute(OperatorRequest::RunReplayWorkload {
            fixture: loaded,
            request: ReplayWorkloadRequest {
                label: "operator-run".into(),
                checkpoints: vec![now],
                profile_views: vec![],
            },
        })
        .unwrap();

    match response {
        OperatorResponse::Workload(report) => {
            assert_eq!(report.checkpoints.len(), 1);
            assert_eq!(report.checkpoints[0].replay.samples_stored, 1);
        }
        other => panic!("unexpected operator response: {other:?}"),
    }

    let _ = std::fs::remove_dir_all(temp_dir);
}
