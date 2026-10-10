// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Graph entity types — the sparse truth model.
//!
//! The core runtime distinguishes:
//! - nodes: identities, endpoints, topics, clusters, anchors
//! - edges: relations / routes / inferred connections
//! - flows: time-bound movement through relations
//! - boundaries: seams between otherwise dissimilar zones

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::spatial::{GeoBBox, GeoCoord};
use crate::types::{Confidence, EntityId, FlowId, Priority, Tag, Timestamp, ValidationError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeKind {
    Endpoint,
    Hop,
    Relay,
    Observer,
    Anchor,
    Exchange,
    Identity,
    Account,
    Topic,
    Cluster,
    Chokepoint,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EntityStatus {
    Active,
    Stale,
    Alerting,
    Dormant,
    Evicted,
}

impl EntityStatus {
    #[must_use]
    pub const fn is_hot(self) -> bool {
        matches!(self, Self::Active | Self::Alerting | Self::Stale)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: EntityId,
    pub kind: NodeKind,
    pub label: String,
    /// Geographic position, if known or inferred.
    pub position: Option<GeoCoord>,
    /// How confident we are in the position fix.
    pub position_confidence: Confidence,
    pub status: EntityStatus,
    pub priority: Priority,
    pub tags: Vec<Tag>,
    pub first_seen: Timestamp,
    pub last_seen: Timestamp,
}

impl Node {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.label.trim().is_empty() {
            return Err(ValidationError::EmptyField("node.label".into()));
        }
        if self.last_seen < self.first_seen {
            return Err(ValidationError::InvalidWindow {
                start_field: "first_seen".into(),
                end_field: "last_seen".into(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EdgeKind {
    Route,
    Link,
    Inference,
    Association,
    Membership,
    Reference,
    BoundaryCrossing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EdgeDirection {
    Directed,
    Undirected,
    Bidirectional,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GeometryMode {
    Straight,
    Geodesic,
    RaisedArc,
    ThroughGlobe,
    Abstract,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub id: EntityId,
    pub kind: EdgeKind,
    pub direction: EdgeDirection,
    pub source: EntityId,
    pub target: EntityId,
    pub geometry_mode: GeometryMode,
    pub waypoints: Vec<GeoCoord>,
    pub confidence: Confidence,
    pub status: EntityStatus,
    pub priority: Priority,
    pub tags: Vec<Tag>,
    pub first_seen: Timestamp,
    pub last_seen: Timestamp,
}

impl Edge {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.source == self.target {
            return Err(ValidationError::InvalidReference(
                "edge source and target must differ".into(),
            ));
        }
        if self.last_seen < self.first_seen {
            return Err(ValidationError::InvalidWindow {
                start_field: "first_seen".into(),
                end_field: "last_seen".into(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FlowKind {
    Traffic,
    Financial,
    Logistics,
    Signal,
    Information,
    Narrative,
    Social,
    Generic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Flow {
    pub id: FlowId,
    pub kind: FlowKind,
    pub edge_path: Vec<EntityId>,
    pub label: Option<String>,
    pub window_start: Timestamp,
    pub window_end: Option<Timestamp>,
    pub ttl: Duration,
    pub status: EntityStatus,
    pub priority: Priority,
    pub tags: Vec<Tag>,
}

impl Flow {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.edge_path.is_empty() {
            return Err(ValidationError::EmptyField("flow.edge_path".into()));
        }
        if self.ttl.is_zero() {
            return Err(ValidationError::ZeroCapacity("flow.ttl".into()));
        }
        if let Some(end) = self.window_end {
            if end < self.window_start {
                return Err(ValidationError::InvalidWindow {
                    start_field: "window_start".into(),
                    end_field: "window_end".into(),
                });
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BoundaryKind {
    Platform,
    Paywall,
    NetworkZone,
    AccessControl,
    TrustZone,
    Language,
    Legal,
    Visibility,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Boundary {
    pub id: EntityId,
    pub kind: BoundaryKind,
    pub label: String,
    /// Optional rough spatial extent where geography is meaningful.
    pub extent: Option<GeoBBox>,
    /// IDs of entities currently associated with or constrained by this boundary.
    pub related_entities: Vec<EntityId>,
    pub confidence: Confidence,
    pub status: EntityStatus,
    pub priority: Priority,
    pub tags: Vec<Tag>,
    pub first_seen: Timestamp,
    pub last_seen: Timestamp,
}

impl Boundary {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.label.trim().is_empty() {
            return Err(ValidationError::EmptyField("boundary.label".into()));
        }
        if self.last_seen < self.first_seen {
            return Err(ValidationError::InvalidWindow {
                start_field: "first_seen".into(),
                end_field: "last_seen".into(),
            });
        }
        if let Some(extent) = self.extent {
            extent.validate()?;
        }
        Ok(())
    }
}
