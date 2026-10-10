// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

use std::any::Any;
use std::time::Duration;

use chrono::Utc;
use skeletrace::{
    AdapterError, AdapterKind, AuditRecord, CacheBudget, Confidence, EngineConfig, EngineError,
    EntityId, EntityStatus, ExportFormat, FlowId, GeoCoord, InterpolationMethod, ManualPushAdapter,
    MetricDefinition, MetricId, MetricValueType, Node, NodeKind, PollCadence, Priority, Quality,
    ReplayBatch, ReplayHarness, RetentionPolicy, Sample, SampleValue, SkeletraceEngine,
    SnapshotExportJob, SnapshotExporter, SnapshotRequest, SourceAdapter, SourceCapabilityProfile,
    SourceCostModel, SourceDefinition, SourceHealth, SourceId, SourceKind, SourcePolicy,
    SourcePull, SourceSchedule, Tag, TimeRange, TransportSupport, ViewJob, ViewJobId, ViewKind,
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

fn node(entity_id: EntityId, now: chrono::DateTime<chrono::Utc>) -> Node {
    Node {
        id: entity_id,
        kind: NodeKind::Identity,
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
) -> Sample {
    Sample {
        entity_id,
        metric_id,
        ts_observed: now,
        ts_ingested: now,
        value: SampleValue::Numeric(42.0),
        quality: Quality::new(1.0).unwrap(),
        source_id,
    }
}

#[derive(Default)]
struct DummyTorAdapter;

impl SourceAdapter for DummyTorAdapter {
    fn kind(&self) -> AdapterKind {
        AdapterKind::TorHttpPoller
    }

    fn pull(
        &mut self,
        _source: &SourceDefinition,
        _now: chrono::DateTime<chrono::Utc>,
    ) -> Result<SourcePull, AdapterError> {
        Ok(SourcePull::default())
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[derive(Default)]
struct FailingParseAdapter;

impl SourceAdapter for FailingParseAdapter {
    fn kind(&self) -> AdapterKind {
        AdapterKind::Manual
    }

    fn pull(
        &mut self,
        _source: &SourceDefinition,
        _now: chrono::DateTime<chrono::Utc>,
    ) -> Result<SourcePull, AdapterError> {
        Err(AdapterError::Parse("fixture parse exploded".into()))
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[test]
fn restrictive_policy_denies_tor_sources_before_registration() {
    let now = Utc::now();
    let source_id = SourceId::new();
    let mut engine = SkeletraceEngine::new(EngineConfig::default()).unwrap();
    engine.set_source_policy(SourcePolicy {
        allow_tor_transport: false,
        ..SourcePolicy::default()
    });

    let source = SourceDefinition {
        id: source_id,
        name: "tor-source".into(),
        kind: SourceKind::Api,
        adapter: AdapterKind::TorHttpPoller,
        schedule: SourceSchedule::Fixed(Duration::from_secs(60)),
        endpoint: "https://example.invalid/feed".into(),
        auth_ref: None,
        health: SourceHealth::Pending,
        last_polled: None,
        last_error: None,
        backoff: Duration::from_secs(5),
        max_backoff: Duration::from_secs(30),
        tags: vec![],
    };

    let capability = SourceCapabilityProfile {
        transport_support: TransportSupport::TorOnly,
        cost_model: SourceCostModel::Free,
        ..SourceCapabilityProfile::default()
    };

    let err = engine
        .register_source_with_capability(source, Box::new(DummyTorAdapter), capability, now)
        .unwrap_err();
    assert!(matches!(err, EngineError::Policy(_)));
    let failures = engine.recent_failures(1);
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].code, "policy.denied");
}

#[test]
fn successful_poll_records_sample_audit_and_export_audit() {
    let now = Utc::now();
    let source_id = SourceId::new();
    let metric_id = MetricId::new();
    let entity_id = EntityId::new();

    let mut engine = SkeletraceEngine::new(EngineConfig {
        cache_budget: CacheBudget {
            max_active_entities: 64,
            max_total_ring_samples: 256,
            max_highres_entities: 16,
            max_approx_hot_bytes: 2 * 1024 * 1024,
            max_per_entity_hot_bytes: 64 * 1024,
        },
        default_ring_capacity: 16,
        default_entity_ttl: Duration::from_secs(300),
        journal_dir: None,
        warm_store_path: None,
    })
    .unwrap();

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
    engine
        .enqueue_manual_batch(
            source_id,
            SourcePull {
                raw_records: vec![],
                samples: vec![sample(entity_id, metric_id, source_id, now)],
                touched_entities: vec![entity_id],
                nodes: vec![],
                edges: vec![],
                boundaries: vec![],
            },
        )
        .unwrap();
    engine.poll_source_now(source_id, now).unwrap();

    let mut audit = engine.recent_audit_records(8);
    assert!(matches!(audit.last(), Some(AuditRecord::Sample(_))));
    let sample_audit = match audit.pop().unwrap() {
        AuditRecord::Sample(record) => record,
        other => panic!("unexpected audit record: {other:?}"),
    };
    assert_eq!(sample_audit.provenance.source_id, source_id);
    assert_eq!(sample_audit.provenance.adapter_kind, AdapterKind::Manual);
    assert!(sample_audit.stored_history);

    let out_dir = std::env::temp_dir().join(format!("skeletrace-stage8-{}", FlowId::new()));
    let exporter = SnapshotExporter::new(&out_dir, None).unwrap();
    let job = SnapshotExportJob {
        request: SnapshotRequest {
            entities: vec![entity_id],
            metrics: vec![metric_id],
            time_range: TimeRange::LatestOnly,
            format: ExportFormat::NativeJson,
            notes: Some("stage8 export".into()),
        },
        view: ViewJob {
            id: ViewJobId::new(),
            kind: ViewKind::DataCard,
            entities: vec![entity_id],
            metrics: vec![metric_id],
            time_range: TimeRange::LatestOnly,
            detail_override: None,
            viewport: None,
        },
        output_stem: Some("audit-check".into()),
    };
    let result = exporter.export(&engine, &job, now).unwrap();
    engine.record_export_audit(&result, &job, now);

    let audit = engine.recent_audit_records(8);
    assert!(audit
        .iter()
        .any(|entry| matches!(entry, AuditRecord::Export(_))));
}

#[test]
fn adapter_failures_are_classified_for_operator_inspection() {
    let now = Utc::now();
    let source_id = SourceId::new();
    let mut engine = SkeletraceEngine::new(EngineConfig::default()).unwrap();
    engine
        .register_source(manual_source(source_id), Box::new(FailingParseAdapter), now)
        .unwrap();

    let err = engine.poll_source_now(source_id, now).unwrap_err();
    assert!(matches!(err, EngineError::Adapter(_)));

    let failures = engine.recent_failures(4);
    assert!(!failures.is_empty());
    assert_eq!(failures.last().unwrap().code, "adapter.parse");
}

#[test]
fn replay_harness_only_releases_batches_that_are_due() {
    let now = Utc::now();
    let source_a = SourceId::new();
    let source_b = SourceId::new();
    let entity_id = EntityId::new();
    let metric_id = MetricId::new();

    let mut harness = ReplayHarness::new();
    harness.push_batch(ReplayBatch {
        source_id: source_a,
        due_at: now,
        pull: SourcePull {
            raw_records: vec![],
            samples: vec![sample(entity_id, metric_id, source_a, now)],
            touched_entities: vec![entity_id],
            nodes: vec![],
            edges: vec![],
            boundaries: vec![],
        },
    });
    harness.push_batch(ReplayBatch {
        source_id: source_a,
        due_at: now + chrono::TimeDelta::seconds(30),
        pull: SourcePull::default(),
    });
    harness.push_batch(ReplayBatch {
        source_id: source_b,
        due_at: now,
        pull: SourcePull::default(),
    });

    let ready = harness.drain_all_ready(now);
    assert_eq!(ready.len(), 2);
    assert_eq!(harness.pending_for(source_a), 1);
    assert_eq!(harness.pending_for(source_b), 0);
}
