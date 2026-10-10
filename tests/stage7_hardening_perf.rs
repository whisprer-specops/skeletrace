// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

use std::time::Duration;

use chrono::Utc;
use skeletrace::{
    AdapterKind, CacheBudget, Confidence, EngineConfig, EntityId, EntityStatus, EventBufferConfig,
    EventKind, GeoCoord, InterpolationMethod, MetricDefinition, MetricId, MetricValueType, Node,
    NodeKind, PerfProbeConfig, PollCadence, Priority, Quality, RetentionPolicy, RetentionTuning,
    Sample, SampleValue, SkeletraceEngine, SourceDefinition, SourceHealth, SourceId, SourceKind,
    SourcePull, SourceSchedule, Tag, TimeRange, ViewJob, ViewJobId, ViewKind,
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

#[test]
fn engine_health_report_and_event_log_track_runtime_activity() {
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
        .configure_event_buffer(EventBufferConfig {
            max_events: 16,
            include_trace: true,
        })
        .unwrap();
    engine
        .register_metric(numeric_metric(metric_id, source_id, "volume"))
        .unwrap();
    engine.register_node(node(entity_id, now)).unwrap();
    engine
        .register_source(
            manual_source(source_id),
            Box::new(skeletrace::ManualPushAdapter::new()),
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

    let report = engine.health_report(now);
    assert_eq!(report.store_stats.node_count, 1);
    assert_eq!(report.store_stats.latest_value_count, 1);
    assert_eq!(report.cache.active_entities, 1);
    assert!(report.event_counts.total >= 2);
    assert!(report
        .source_health_counts
        .iter()
        .any(|entry| entry.health == SourceHealth::Healthy && entry.count == 1));

    let events = engine.recent_events(16);
    assert!(events
        .iter()
        .any(|event| event.kind == EventKind::SourceRegistered));
    assert!(events
        .iter()
        .any(|event| event.kind == EventKind::SourcePollSuccess));
}

#[test]
fn retention_tuning_updates_policy_and_reports_counts() {
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
            Box::new(skeletrace::ManualPushAdapter::new()),
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

    let report = engine
        .retune_metric_retention(
            metric_id,
            RetentionTuning {
                hot_duration: Some(Duration::from_secs(60)),
                warm_duration: Some(Duration::from_secs(600)),
                store_on_change_only: Some(true),
                change_threshold: Some(Some(10.0)),
                relative_change_threshold: Some(Some(0.25)),
                max_silent_gap: Some(Duration::from_secs(120)),
            },
            now,
        )
        .unwrap();

    assert_eq!(report.metric_id, metric_id);
    assert_eq!(report.hot_duration, Duration::from_secs(60));
    assert_eq!(report.warm_duration, Duration::from_secs(600));
    assert!(report.store_on_change_only);
    assert_eq!(report.hot_sample_count, 1);
}

#[test]
fn profile_materialization_and_prune_cycle_return_perf_reports() {
    let now = Utc::now();
    let source_id = SourceId::new();
    let metric_id = MetricId::new();
    let entity_id = EntityId::new();

    let mut engine = SkeletraceEngine::new(EngineConfig::default()).unwrap();
    engine
        .register_metric(numeric_metric(metric_id, source_id, "volume"))
        .unwrap();
    engine.register_node(node(entity_id, now)).unwrap();

    let view = ViewJob {
        id: ViewJobId::new(),
        kind: ViewKind::Topology,
        entities: vec![entity_id],
        metrics: vec![metric_id],
        time_range: TimeRange::LatestOnly,
        detail_override: None,
        viewport: None,
    };

    let perf = engine
        .profile_view_materialization(&view, now, PerfProbeConfig { iterations: 3 })
        .unwrap();
    assert_eq!(perf.iterations, 3);
    assert_eq!(perf.stats.runs, 3);
    assert!(perf.label.starts_with("topology:"));

    let prune_perf = engine
        .profile_prune_cycle(now, PerfProbeConfig { iterations: 2 })
        .unwrap();
    assert_eq!(prune_perf.stats.runs, 2);
}
