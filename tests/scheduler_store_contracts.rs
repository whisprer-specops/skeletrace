// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

use std::time::Duration;

use chrono::{TimeDelta, Utc};
use skeletrace::{
    AdapterKind, EngineStore, EntityId, MetricDefinition, MetricId, MetricValueType, PollCadence,
    Quality, RetentionPolicy, Sample, SampleValue, ScheduleEntry, SourceDefinition, SourceHealth,
    SourceId, SourceKind, SourceSchedule, StoreError, ValidationError,
};

fn metric(metric_id: MetricId, source_id: SourceId) -> MetricDefinition {
    MetricDefinition {
        id: metric_id,
        name: "trend_score".into(),
        unit: "pts".into(),
        value_type: MetricValueType::Numeric,
        cadence: PollCadence::Fixed(Duration::from_secs(30)),
        interpolation: skeletrace::InterpolationMethod::Linear,
        retention: RetentionPolicy {
            hot_duration: Duration::from_secs(60),
            warm_duration: Duration::from_secs(120),
            store_on_change_only: true,
            change_threshold: Some(5.0),
            relative_change_threshold: Some(0.10),
            max_silent_gap: Duration::from_secs(45),
        },
        source_ids: vec![source_id],
        show_in_popup: true,
        popup_priority: 1,
        description: "trend score".into(),
    }
}

#[test]
fn schedule_entry_applies_exponential_backoff_and_recovers() {
    let now = Utc::now();
    let source_id = SourceId::new();
    let mut entry = ScheduleEntry::new(
        source_id,
        SourceSchedule::Fixed(Duration::from_secs(30)),
        now,
        100,
        Duration::ZERO,
    )
    .unwrap();

    assert!(!entry.is_due(now));
    assert!(entry.is_due(now + TimeDelta::seconds(31)));

    entry
        .mark_failure(Duration::from_secs(5), Duration::from_secs(30), now)
        .unwrap();
    let first_due = entry.next_due;
    entry
        .mark_failure(Duration::from_secs(5), Duration::from_secs(30), now)
        .unwrap();
    assert!(entry.next_due >= first_due);

    entry
        .mark_success(SourceSchedule::Fixed(Duration::from_secs(30)), now)
        .unwrap();
    assert_eq!(entry.consecutive_failures, 0);
}

#[test]
fn store_rejects_unknown_metrics_and_prunes_old_history() {
    let now = Utc::now();
    let source_id = SourceId::new();
    let metric_id = MetricId::new();
    let entity_id = EntityId::new();

    let mut store = EngineStore::new();
    store
        .register_source(SourceDefinition {
            id: source_id,
            name: "source".into(),
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
        })
        .unwrap();

    let unknown = Sample {
        entity_id,
        metric_id,
        ts_observed: now,
        ts_ingested: now,
        value: SampleValue::Numeric(42.0),
        quality: Quality::new(1.0).unwrap(),
        source_id,
    };
    assert!(matches!(
        store.ingest_sample(unknown.clone()),
        Err(StoreError::UnknownMetric(id)) if id == metric_id
    ));

    store.register_metric(metric(metric_id, source_id)).unwrap();
    store.ingest_sample(unknown).unwrap();
    store
        .ingest_sample(Sample {
            ts_observed: now + TimeDelta::seconds(10),
            ts_ingested: now + TimeDelta::seconds(10),
            value: SampleValue::Numeric(50.0),
            ..Sample {
                entity_id,
                metric_id,
                ts_observed: now,
                ts_ingested: now,
                value: SampleValue::Numeric(42.0),
                quality: Quality::new(1.0).unwrap(),
                source_id,
            }
        })
        .unwrap();

    assert_eq!(store.stats().retained_sample_count, 2);
    store.prune_retained_samples(now + TimeDelta::seconds(121));
    assert_eq!(store.stats().retained_sample_count, 0);
}

#[test]
fn non_automatic_schedules_do_not_count_as_automatic() {
    assert!(SourceSchedule::Fixed(Duration::from_secs(1)).is_automatic());
    assert!(!SourceSchedule::EventDriven.is_automatic());
    assert!(!SourceSchedule::Manual.is_automatic());
    assert!(matches!(
        SourceSchedule::Fixed(Duration::ZERO).validate(),
        Err(ValidationError::ZeroCapacity(_))
    ));
}
