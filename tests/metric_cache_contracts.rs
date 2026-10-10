// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

use std::time::Duration;

use chrono::{TimeDelta, Utc};
use skeletrace::{
    CacheEntry, DetailTier, EntityId, InterpolationMethod, LatestValue, MetricDefinition, MetricId,
    MetricValueType, PollCadence, Quality, RetentionPolicy, RingBuffer, Sample, SampleValue,
    SourceId,
};

fn sample(
    entity_id: EntityId,
    metric_id: MetricId,
    source_id: SourceId,
    ts: chrono::DateTime<chrono::Utc>,
    value: SampleValue,
) -> Sample {
    Sample {
        entity_id,
        metric_id,
        ts_observed: ts,
        ts_ingested: ts,
        value,
        quality: Quality::new(0.95).unwrap(),
        source_id,
    }
}

#[test]
fn ring_buffer_never_exceeds_capacity_after_many_pushes() {
    let entity_id = EntityId::new();
    let metric_id = MetricId::new();
    let source_id = SourceId::new();
    let base = Utc::now();

    let mut rb = RingBuffer::new(entity_id, metric_id, 4).unwrap();
    for idx in 0..32 {
        rb.push(sample(
            entity_id,
            metric_id,
            source_id,
            base + TimeDelta::seconds(idx),
            SampleValue::Numeric(idx as f64),
        ));
    }

    assert_eq!(rb.len(), 4);
    let ordered = rb.ordered_samples();
    assert_eq!(ordered.first().unwrap().value, SampleValue::Numeric(28.0));
    assert_eq!(ordered.last().unwrap().value, SampleValue::Numeric(31.0));
}

#[test]
fn latest_tracks_empty_partial_full_and_wrapped_buffers() {
    let entity_id = EntityId::new();
    let metric_id = MetricId::new();
    let source_id = SourceId::new();
    let now = Utc::now();
    for capacity in [1, 2, 4] {
        let mut buffer = RingBuffer::new(entity_id, metric_id, capacity).unwrap();
        assert!(buffer.latest().is_none());
        for value in 0..20 {
            buffer.push(sample(
                entity_id,
                metric_id,
                source_id,
                now,
                SampleValue::Numeric(f64::from(value)),
            ));
            assert_eq!(
                buffer.latest().unwrap().value,
                SampleValue::Numeric(f64::from(value))
            );
        }
    }
}

#[test]
fn latest_value_tracks_sample_and_reports_size() {
    let entity_id = EntityId::new();
    let metric_id = MetricId::new();
    let source_id = SourceId::new();
    let now = Utc::now();

    let sample = sample(
        entity_id,
        metric_id,
        source_id,
        now,
        SampleValue::Code("alpha".into()),
    );
    let latest = LatestValue::from_sample(&sample);

    assert_eq!(latest.metric_id, metric_id);
    assert_eq!(latest.timestamp, now);
    assert!(latest.approx_bytes() >= "alpha".len());
}

#[test]
fn cache_entry_hot_bytes_grow_when_latest_and_buffers_are_added() {
    let entity_id = EntityId::new();
    let metric_id = MetricId::new();
    let source_id = SourceId::new();
    let now = Utc::now();

    let mut entry =
        CacheEntry::new(entity_id, DetailTier::Sampled, Duration::from_secs(90), now).unwrap();
    let initial = entry.approx_hot_bytes();

    entry.upsert_latest(LatestValue {
        metric_id,
        value: SampleValue::Code("short-payload".into()),
        timestamp: now,
        quality: Quality::new(1.0).unwrap(),
        source_id,
    });

    let mut ring = RingBuffer::new(entity_id, metric_id, 2).unwrap();
    ring.push(sample(
        entity_id,
        metric_id,
        source_id,
        now,
        SampleValue::Numeric(10.0),
    ));
    ring.push(sample(
        entity_id,
        metric_id,
        source_id,
        now + TimeDelta::seconds(30),
        SampleValue::Numeric(20.0),
    ));
    entry.ring_buffers.push(ring);

    assert!(entry.approx_hot_bytes() > initial);
}

#[test]
fn retention_policy_heartbeat_stores_after_silent_gap_even_without_value_change() {
    let metric_id = MetricId::new();
    let source_id = SourceId::new();
    let now = Utc::now();

    let retention = RetentionPolicy {
        hot_duration: Duration::from_secs(300),
        warm_duration: Duration::from_secs(600),
        store_on_change_only: true,
        change_threshold: Some(100.0),
        relative_change_threshold: Some(0.50),
        max_silent_gap: Duration::from_secs(60),
    };

    let previous = LatestValue {
        metric_id,
        value: SampleValue::Numeric(1000.0),
        timestamp: now,
        quality: Quality::new(1.0).unwrap(),
        source_id,
    };

    let unchanged_late = sample(
        EntityId::new(),
        metric_id,
        source_id,
        now + TimeDelta::seconds(120),
        SampleValue::Numeric(1000.0),
    );

    assert!(retention.should_store(Some(&previous), &unchanged_late));
}

#[test]
fn sample_validation_rejects_metric_value_mismatches() {
    let metric_id = MetricId::new();
    let source_id = SourceId::new();
    let definition = MetricDefinition {
        id: metric_id,
        name: "bool_flag".into(),
        unit: String::new(),
        value_type: MetricValueType::Flag,
        cadence: PollCadence::EventDriven,
        interpolation: InterpolationMethod::None,
        retention: RetentionPolicy {
            hot_duration: Duration::from_secs(60),
            warm_duration: Duration::from_secs(300),
            store_on_change_only: false,
            change_threshold: None,
            relative_change_threshold: None,
            max_silent_gap: Duration::from_secs(60),
        },
        source_ids: vec![source_id],
        show_in_popup: true,
        popup_priority: 0,
        description: "feature flag".into(),
    };

    let bad = sample(
        EntityId::new(),
        metric_id,
        source_id,
        Utc::now(),
        SampleValue::Numeric(1.0),
    );

    assert!(bad.validate(&definition).is_err());
}
