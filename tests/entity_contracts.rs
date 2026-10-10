// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

use std::time::Duration;

use chrono::{TimeDelta, Utc};
use skeletrace::{
    Boundary, BoundaryKind, Confidence, Edge, EdgeDirection, EdgeKind, EntityId, EntityStatus,
    Flow, FlowId, FlowKind, GeoBBox, GeometryMode, Node, NodeKind, Priority, Tag, ValidationError,
};

#[test]
fn entity_status_hotness_matches_runtime_expectation() {
    assert!(EntityStatus::Active.is_hot());
    assert!(EntityStatus::Alerting.is_hot());
    assert!(EntityStatus::Stale.is_hot());
    assert!(!EntityStatus::Dormant.is_hot());
    assert!(!EntityStatus::Evicted.is_hot());
}

#[test]
fn node_validation_rejects_blank_labels() {
    let now = Utc::now();
    let node = Node {
        id: EntityId::new(),
        kind: NodeKind::Identity,
        label: "   ".into(),
        position: None,
        position_confidence: Confidence::new(0.8).unwrap(),
        status: EntityStatus::Active,
        priority: Priority::HIGH,
        tags: vec![Tag::new("role", "source").unwrap()],
        first_seen: now,
        last_seen: now,
    };

    assert!(matches!(
        node.validate(),
        Err(ValidationError::EmptyField(_))
    ));
}

#[test]
fn edge_validation_rejects_self_reference() {
    let now = Utc::now();
    let same = EntityId::new();
    let edge = Edge {
        id: EntityId::new(),
        kind: EdgeKind::Link,
        direction: EdgeDirection::Directed,
        source: same,
        target: same,
        geometry_mode: GeometryMode::Straight,
        waypoints: vec![],
        confidence: Confidence::new(0.9).unwrap(),
        status: EntityStatus::Active,
        priority: Priority::NORMAL,
        tags: vec![],
        first_seen: now,
        last_seen: now,
    };

    assert!(matches!(
        edge.validate(),
        Err(ValidationError::InvalidReference(_))
    ));
}

#[test]
fn flow_validation_rejects_empty_paths_and_zero_ttl() {
    let now = Utc::now();
    let flow = Flow {
        id: FlowId::new(),
        kind: FlowKind::Information,
        edge_path: vec![],
        label: Some("empty path".into()),
        window_start: now,
        window_end: None,
        ttl: Duration::ZERO,
        status: EntityStatus::Active,
        priority: Priority::NORMAL,
        tags: vec![],
    };

    assert!(matches!(
        flow.validate(),
        Err(ValidationError::EmptyField(_))
    ));

    let flow = Flow {
        edge_path: vec![EntityId::new()],
        ttl: Duration::ZERO,
        ..flow
    };

    assert!(matches!(
        flow.validate(),
        Err(ValidationError::ZeroCapacity(_))
    ));
}

#[test]
fn flow_validation_rejects_inverted_window() {
    let now = Utc::now();
    let flow = Flow {
        id: FlowId::new(),
        kind: FlowKind::Signal,
        edge_path: vec![EntityId::new(), EntityId::new()],
        label: Some("window".into()),
        window_start: now,
        window_end: Some(now - TimeDelta::seconds(5)),
        ttl: Duration::from_secs(60),
        status: EntityStatus::Active,
        priority: Priority::CRITICAL,
        tags: vec![],
    };

    assert!(matches!(
        flow.validate(),
        Err(ValidationError::InvalidWindow { .. })
    ));
}

#[test]
fn boundary_validation_checks_extent_and_label() {
    let now = Utc::now();
    let invalid_extent = GeoBBox {
        min_lat: 10.0,
        min_lon: 5.0,
        max_lat: 0.0,
        max_lon: 15.0,
    };

    let boundary = Boundary {
        id: EntityId::new(),
        kind: BoundaryKind::Paywall,
        label: "Public/private seam".into(),
        extent: Some(invalid_extent),
        related_entities: vec![EntityId::new()],
        confidence: Confidence::new(0.65).unwrap(),
        status: EntityStatus::Stale,
        priority: Priority::HIGH,
        tags: vec![],
        first_seen: now,
        last_seen: now,
    };

    assert!(matches!(
        boundary.validate(),
        Err(ValidationError::InvalidWindow { .. }) | Err(ValidationError::OutOfRange { .. })
    ));

    let valid_boundary = Boundary {
        label: "Core choke zone".into(),
        extent: Some(GeoBBox::new(-5.0, -10.0, 5.0, 10.0).unwrap()),
        ..boundary
    };

    assert!(valid_boundary.validate().is_ok());
}
