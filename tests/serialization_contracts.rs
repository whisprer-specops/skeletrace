// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

use std::time::Duration;

use chrono::{TimeDelta, Utc};
use skeletrace::{
    Boundary, BoundaryKind, Confidence, EntityId, EntityStatus, ExportFormat, Flow, FlowId,
    FlowKind, GeoBBox, MetricId, Priority, SnapshotRequest, SourceId, Tag, TimeRange, ViewJob,
    ViewJobId, ViewKind,
};

#[test]
fn boundary_roundtrips_through_json() {
    let now = Utc::now();
    let boundary = Boundary {
        id: EntityId::new(),
        kind: BoundaryKind::NetworkZone,
        label: "public-web / tor seam".into(),
        extent: Some(GeoBBox::new(40.0, -10.0, 60.0, 10.0).unwrap()),
        related_entities: vec![EntityId::new(), EntityId::new()],
        confidence: Confidence::new(0.72).unwrap(),
        status: EntityStatus::Active,
        priority: Priority::HIGH,
        tags: vec![Tag::new("seam", "network").unwrap()],
        first_seen: now,
        last_seen: now,
    };

    let encoded = serde_json::to_string(&boundary).unwrap();
    let decoded: Boundary = serde_json::from_str(&encoded).unwrap();

    assert_eq!(decoded.label, boundary.label);
    assert_eq!(decoded.kind, boundary.kind);
    assert_eq!(decoded.related_entities, boundary.related_entities);
}

#[test]
fn flow_roundtrips_through_json() {
    let now = Utc::now();
    let flow = Flow {
        id: FlowId::new(),
        kind: FlowKind::Narrative,
        edge_path: vec![EntityId::new(), EntityId::new()],
        label: Some("topic spread".into()),
        window_start: now,
        window_end: Some(now + TimeDelta::seconds(45)),
        ttl: Duration::from_secs(300),
        status: EntityStatus::Active,
        priority: Priority::NORMAL,
        tags: vec![Tag::new("topic", "campaign").unwrap()],
    };

    let encoded = serde_json::to_vec(&flow).unwrap();
    let decoded: Flow = serde_json::from_slice(&encoded).unwrap();

    assert_eq!(decoded.kind, flow.kind);
    assert_eq!(decoded.edge_path, flow.edge_path);
    assert_eq!(decoded.label, flow.label);
}

#[test]
fn view_job_and_snapshot_request_serialize_cleanly() {
    let start = Utc::now() - TimeDelta::minutes(2);
    let end = Utc::now();

    let view_job = ViewJob {
        id: ViewJobId::new(),
        kind: ViewKind::Compare,
        entities: vec![EntityId::new(), EntityId::new()],
        metrics: vec![MetricId::new()],
        time_range: TimeRange::Window { start, end },
        detail_override: None,
        viewport: None,
    };

    let request = SnapshotRequest {
        entities: view_job.entities.clone(),
        metrics: view_job.metrics.clone(),
        time_range: view_job.time_range,
        format: ExportFormat::GeoJson,
        notes: Some("operator initiated export".into()),
    };

    let view_json = serde_json::to_value(&view_job).unwrap();
    let request_json = serde_json::to_value(&request).unwrap();

    assert_eq!(view_json["kind"], "Compare");
    assert_eq!(request_json["format"], "GeoJson");
    assert_eq!(request_json["notes"], "operator initiated export");
}

#[test]
fn id_types_roundtrip_as_uuid_strings() {
    let source_id = SourceId::new();
    let json = serde_json::to_string(&source_id).unwrap();
    let decoded: SourceId = serde_json::from_str(&json).unwrap();

    assert_eq!(decoded, source_id);
    assert!(json.contains('-'));
}
