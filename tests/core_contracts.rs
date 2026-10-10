// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

use chrono::Utc;
use skeletrace::{
    CacheEntry, Confidence, DetailTier, EntityId, GeoBBox, GeoCoord, LatestValue, MetricDefinition,
    MetricId, MetricValueType, PollCadence, Quality, RetentionPolicy, RingBuffer, Sample,
    SampleValue, SourceId, ValidationError, WGS84,
};
use std::time::Duration;

#[test]
fn confidence_rejects_out_of_range_values() {
    assert!(matches!(
        Confidence::new(2.0),
        Err(ValidationError::OutOfRange { .. })
    ));
    assert!(Confidence::new(0.75).is_ok());
}

#[test]
fn geo_coord_roundtrips_through_ecef() {
    let coord = GeoCoord::new(51.5074, -0.1278, Some(15.0)).unwrap();
    let ecef = coord.to_ecef(WGS84);
    let roundtrip = GeoCoord::from_ecef(ecef, WGS84).unwrap();

    assert!((coord.lat - roundtrip.lat).abs() < 1e-6);
    assert!((coord.lon - roundtrip.lon).abs() < 1e-6);
}

#[test]
fn bbox_contains_expected_point() {
    let bbox = GeoBBox::new(50.0, -1.0, 52.0, 1.0).unwrap();
    let coord = GeoCoord::new(51.5, 0.0, None).unwrap();
    assert!(bbox.contains(coord));
}

#[test]
fn ring_buffer_wraps_and_preserves_logical_order() {
    let entity_id = EntityId::new();
    let metric_id = MetricId::new();
    let source_id = SourceId::new();
    let now = Utc::now();

    let mut rb = RingBuffer::new(entity_id, metric_id, 2).unwrap();
    for value in [1.0, 2.0, 3.0] {
        rb.push(Sample {
            entity_id,
            metric_id,
            ts_observed: now,
            ts_ingested: now,
            value: SampleValue::Numeric(value),
            quality: Quality::new(1.0).unwrap(),
            source_id,
        });
    }

    let ordered = rb.ordered_samples();
    assert_eq!(ordered.len(), 2);
    assert_eq!(ordered[0].value, SampleValue::Numeric(2.0));
    assert_eq!(ordered[1].value, SampleValue::Numeric(3.0));
}

#[test]
fn retention_policy_uses_thresholds_and_heartbeat() {
    let metric_id = MetricId::new();
    let source_id = SourceId::new();
    let now = Utc::now();

    let def = MetricDefinition {
        id: metric_id,
        name: "volume_bytes".into(),
        unit: "B".into(),
        value_type: MetricValueType::Numeric,
        cadence: PollCadence::Fixed(Duration::from_secs(30)),
        interpolation: skeletrace::InterpolationMethod::StepForward,
        retention: RetentionPolicy {
            hot_duration: Duration::from_secs(300),
            warm_duration: Duration::from_secs(3600),
            store_on_change_only: true,
            change_threshold: Some(10_000.0),
            relative_change_threshold: Some(0.10),
            max_silent_gap: Duration::from_secs(120),
        },
        source_ids: vec![source_id],
        show_in_popup: true,
        popup_priority: 1,
        description: "Byte volume".into(),
    };

    let previous = LatestValue {
        metric_id,
        value: SampleValue::Numeric(100_000.0),
        timestamp: now,
        quality: Quality::new(1.0).unwrap(),
        source_id,
    };

    let insignificant = Sample {
        entity_id: EntityId::new(),
        metric_id,
        ts_observed: now + chrono::TimeDelta::seconds(30),
        ts_ingested: now + chrono::TimeDelta::seconds(30),
        value: SampleValue::Numeric(101_000.0),
        quality: Quality::new(1.0).unwrap(),
        source_id,
    };

    let significant = Sample {
        entity_id: EntityId::new(),
        metric_id,
        ts_observed: now + chrono::TimeDelta::seconds(30),
        ts_ingested: now + chrono::TimeDelta::seconds(30),
        value: SampleValue::Numeric(120_000.0),
        quality: Quality::new(1.0).unwrap(),
        source_id,
    };

    assert!(!def.retention.should_store(Some(&previous), &insignificant));
    assert!(def.retention.should_store(Some(&previous), &significant));
}

#[test]
fn cache_entry_upserts_latest_values() {
    let entity_id = EntityId::new();
    let metric_id = MetricId::new();
    let source_id = SourceId::new();
    let now = Utc::now();

    let mut entry =
        CacheEntry::new(entity_id, DetailTier::Active, Duration::from_secs(60), now).unwrap();

    entry.upsert_latest(LatestValue {
        metric_id,
        value: SampleValue::Flag(true),
        timestamp: now,
        quality: Quality::new(1.0).unwrap(),
        source_id,
    });
    entry.upsert_latest(LatestValue {
        metric_id,
        value: SampleValue::Flag(false),
        timestamp: now,
        quality: Quality::new(1.0).unwrap(),
        source_id,
    });

    assert_eq!(entry.latest_by_metric.len(), 1);
    assert_eq!(
        entry.latest(metric_id).unwrap().value,
        SampleValue::Flag(false)
    );
}
