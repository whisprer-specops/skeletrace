// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::time::Duration;

use chrono::{TimeDelta, Utc};
use skeletrace::{
    AdapterKind, DataCard, DetailTier, EngineConfig, EntityId, EntityStatus, FileSampleRecord,
    ManualPushAdapter, MetricDefinition, MetricId, MetricValueType, NdjsonSampleFileAdapter, Node,
    NodeKind, PollCadence, Priority, Quality, RawIngestRecord, RetentionPolicy, Sample,
    SampleValue, SkeletraceEngine, SourceDefinition, SourceHealth, SourceId, SourceKind,
    SourcePull, SourceSchedule, Tag, TimeRange, ViewJob, ViewJobId, ViewKind,
};

fn numeric_metric(metric_id: MetricId, source_id: SourceId) -> MetricDefinition {
    MetricDefinition {
        id: metric_id,
        name: "volume_bytes".into(),
        unit: "B".into(),
        value_type: MetricValueType::Numeric,
        cadence: PollCadence::Fixed(Duration::from_secs(30)),
        interpolation: skeletrace::InterpolationMethod::StepForward,
        retention: RetentionPolicy {
            hot_duration: Duration::from_secs(300),
            warm_duration: Duration::from_secs(3_600),
            store_on_change_only: true,
            change_threshold: Some(1.0),
            relative_change_threshold: Some(0.01),
            max_silent_gap: Duration::from_secs(60),
        },
        source_ids: vec![source_id],
        show_in_popup: true,
        popup_priority: 1,
        description: "byte volume".into(),
    }
}

fn node(entity_id: EntityId) -> Node {
    let now = Utc::now();
    Node {
        id: entity_id,
        kind: NodeKind::Identity,
        label: "alpha-node".into(),
        position: None,
        position_confidence: skeletrace::Confidence::new(0.9).unwrap(),
        status: EntityStatus::Active,
        priority: Priority::HIGH,
        tags: vec![Tag::new("kind", "identity").unwrap()],
        first_seen: now,
        last_seen: now,
    }
}

#[test]
fn manual_batches_can_be_enqueued_polled_and_hydrated() {
    let now = Utc::now();
    let source_id = SourceId::new();
    let metric_id = MetricId::new();
    let entity_id = EntityId::new();

    let mut engine = SkeletraceEngine::new(EngineConfig::default()).unwrap();
    engine.register_node(node(entity_id)).unwrap();
    engine
        .register_metric(numeric_metric(metric_id, source_id))
        .unwrap();

    let source = SourceDefinition {
        id: source_id,
        name: "manual-source".into(),
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
    engine
        .register_source(source, Box::new(ManualPushAdapter::new()), now)
        .unwrap();

    engine
        .promote_for_view(
            &ViewJob {
                id: ViewJobId::new(),
                kind: ViewKind::Timeline,
                entities: vec![entity_id],
                metrics: vec![metric_id],
                time_range: TimeRange::Since(now - TimeDelta::minutes(1)),
                detail_override: Some(DetailTier::Sampled),
                viewport: None,
            },
            now,
        )
        .unwrap();

    engine
        .enqueue_manual_batch(
            source_id,
            SourcePull {
                raw_records: vec![RawIngestRecord {
                    source_id,
                    source_timestamp: Some(now),
                    ingested_at: now,
                    payload: serde_json::json!({"kind": "manual", "value": 12345}),
                }],
                samples: vec![Sample {
                    entity_id,
                    metric_id,
                    ts_observed: now,
                    ts_ingested: now,
                    value: SampleValue::Numeric(12_345.0),
                    quality: Quality::new(1.0).unwrap(),
                    source_id,
                }],
                touched_entities: vec![entity_id],
                nodes: vec![],
                edges: vec![],
                boundaries: vec![],
            },
        )
        .unwrap();

    let report = engine.poll_source_now(source_id, now).unwrap();
    assert_eq!(report.samples_seen, 1);
    assert_eq!(report.samples_stored, 1);

    let card: DataCard = engine
        .hydrate_data_card(
            entity_id,
            &[metric_id],
            now,
            TimeRange::Since(now - TimeDelta::minutes(5)),
        )
        .unwrap();
    assert_eq!(card.label, "alpha-node");
    assert_eq!(card.summary_fields.len(), 1);
    assert_eq!(card.summary_fields[0].metric_name, "volume_bytes");
    assert!(card.history_available);

    let cache = engine.cache_entry(entity_id).unwrap();
    assert_eq!(cache.ring_buffers.len(), 1);
    assert_eq!(cache.ring_buffers[0].len(), 1);
}

#[test]
fn ndjson_adapter_tails_new_lines_without_replaying_old_ones() {
    let base = Utc::now();
    let source_id = SourceId::new();
    let metric_id = MetricId::new();
    let entity_id = EntityId::new();

    let mut engine = SkeletraceEngine::new(EngineConfig::default()).unwrap();
    engine.register_node(node(entity_id)).unwrap();
    engine
        .register_metric(numeric_metric(metric_id, source_id))
        .unwrap();

    let path = std::env::temp_dir().join(format!("skeletrace-stage3-{}.ndjson", SourceId::new()));
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&path)
        .unwrap();
    writeln!(
        file,
        "{}",
        serde_json::to_string(&FileSampleRecord {
            entity_id,
            metric_id,
            ts_observed: base,
            ts_ingested: Some(base),
            value: SampleValue::Numeric(10.0),
            quality: Quality::new(1.0).unwrap(),
            raw_payload: None,
        })
        .unwrap()
    )
    .unwrap();
    drop(file);

    let source = SourceDefinition {
        id: source_id,
        name: "ndjson-source".into(),
        kind: SourceKind::File,
        adapter: AdapterKind::FileImport,
        schedule: SourceSchedule::Fixed(Duration::from_secs(30)),
        endpoint: path.to_string_lossy().into_owned(),
        auth_ref: None,
        health: SourceHealth::Pending,
        last_polled: None,
        last_error: None,
        backoff: Duration::from_secs(5),
        max_backoff: Duration::from_secs(30),
        tags: vec![],
    };
    engine
        .register_source(source, Box::new(NdjsonSampleFileAdapter::new()), base)
        .unwrap();

    let first = engine.tick(base + TimeDelta::seconds(31)).unwrap();
    assert_eq!(first.polled_sources, 1);
    assert_eq!(first.samples_seen, 1);
    assert_eq!(engine.stats().retained_sample_count, 1);

    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(
        file,
        "{}",
        serde_json::to_string(&FileSampleRecord {
            entity_id,
            metric_id,
            ts_observed: base + TimeDelta::seconds(60),
            ts_ingested: Some(base + TimeDelta::seconds(60)),
            value: SampleValue::Numeric(25.0),
            quality: Quality::new(1.0).unwrap(),
            raw_payload: None,
        })
        .unwrap()
    )
    .unwrap();
    drop(file);

    let second = engine.tick(base + TimeDelta::seconds(62)).unwrap();
    assert_eq!(second.polled_sources, 1);
    assert_eq!(second.samples_seen, 1);
    assert_eq!(engine.stats().retained_sample_count, 2);

    let _ = fs::remove_file(path);
}
