// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! View materializers for topology and sparse geo outputs.
//!
//! These stay intentionally lightweight and derive display-ready slices from
//! the store without turning the runtime truth into a giant map model.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::entity::{Edge, GeometryMode};
use crate::spatial::{GeoBBox, GeoCoord};
use crate::store::EngineStore;
use crate::types::{EntityId, MetricId, Timestamp, ValidationError, ViewJobId};
use crate::view::ViewJob;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterializedMetricValue {
    pub metric_id: MetricId,
    pub metric_name: String,
    pub display_value: String,
    pub unit: String,
    pub timestamp: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyNodeView {
    pub entity_id: EntityId,
    pub label: String,
    pub kind_label: String,
    pub position: Option<GeoCoord>,
    pub metrics: Vec<MaterializedMetricValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyEdgeView {
    pub entity_id: EntityId,
    pub source: EntityId,
    pub target: EntityId,
    pub kind_label: String,
    pub geometry_mode: GeometryMode,
    pub metrics: Vec<MaterializedMetricValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyBoundaryView {
    pub entity_id: EntityId,
    pub label: String,
    pub kind_label: String,
    pub extent: Option<GeoBBox>,
    pub metrics: Vec<MaterializedMetricValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyViewMaterialization {
    pub view_id: ViewJobId,
    pub nodes: Vec<TopologyNodeView>,
    pub edges: Vec<TopologyEdgeView>,
    pub boundaries: Vec<TopologyBoundaryView>,
}

pub struct TopologyMaterializer;

impl TopologyMaterializer {
    pub fn build(
        store: &EngineStore,
        view: &ViewJob,
        _now: Timestamp,
    ) -> Result<TopologyViewMaterialization, ValidationError> {
        view.validate()?;

        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        let mut boundaries = Vec::new();

        for entity_id in &view.entities {
            if let Some(node) = store.node(*entity_id) {
                nodes.push(TopologyNodeView {
                    entity_id: node.id,
                    label: node.label.clone(),
                    kind_label: format!("{:?}", node.kind),
                    position: node.position,
                    metrics: collect_metrics(store, node.id, &view.metrics),
                });
            }
            if let Some(edge) = store.edge(*entity_id) {
                edges.push(TopologyEdgeView {
                    entity_id: edge.id,
                    source: edge.source,
                    target: edge.target,
                    kind_label: format!("{:?}", edge.kind),
                    geometry_mode: edge.geometry_mode,
                    metrics: collect_metrics(store, edge.id, &view.metrics),
                });
            }
            if let Some(boundary) = store.boundary(*entity_id) {
                boundaries.push(TopologyBoundaryView {
                    entity_id: boundary.id,
                    label: boundary.label.clone(),
                    kind_label: format!("{:?}", boundary.kind),
                    extent: boundary.extent,
                    metrics: collect_metrics(store, boundary.id, &view.metrics),
                });
            }
        }

        let selected: HashSet<EntityId> = view.entities.iter().copied().collect();
        let mut included_edges: HashSet<EntityId> =
            edges.iter().map(|edge| edge.entity_id).collect();
        for edge in store.edges_iter() {
            if selected.contains(&edge.source)
                && selected.contains(&edge.target)
                && included_edges.insert(edge.id)
            {
                edges.push(TopologyEdgeView {
                    entity_id: edge.id,
                    source: edge.source,
                    target: edge.target,
                    kind_label: format!("{:?}", edge.kind),
                    geometry_mode: edge.geometry_mode,
                    metrics: collect_metrics(store, edge.id, &view.metrics),
                });
            }
        }

        Ok(TopologyViewMaterialization {
            view_id: view.id,
            nodes,
            edges,
            boundaries,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SparseGeoGeometry {
    Point(GeoCoord),
    LineString(Vec<GeoCoord>),
    BBox(GeoBBox),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SparseGeoFeature {
    pub entity_id: EntityId,
    pub label: String,
    pub kind_label: String,
    pub geometry: SparseGeoGeometry,
    pub metrics: Vec<MaterializedMetricValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SparseGeoViewMaterialization {
    pub view_id: ViewJobId,
    pub viewport: GeoBBox,
    pub features: Vec<SparseGeoFeature>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SparseGeoFeatureCollection {
    pub viewport: GeoBBox,
    pub feature_count: usize,
    pub geojson: Value,
}

impl SparseGeoViewMaterialization {
    #[must_use]
    pub fn to_feature_collection(&self) -> SparseGeoFeatureCollection {
        let features: Vec<Value> = self
            .features
            .iter()
            .map(|feature| {
                json!({
                    "type": "Feature",
                    "id": feature.entity_id.to_string(),
                    "properties": build_properties(feature),
                    "geometry": geometry_to_geojson(&feature.geometry),
                })
            })
            .collect();

        SparseGeoFeatureCollection {
            viewport: self.viewport,
            feature_count: features.len(),
            geojson: json!({
                "type": "FeatureCollection",
                "features": features,
            }),
        }
    }
}

pub struct SparseGeoMaterializer;

impl SparseGeoMaterializer {
    pub fn build(
        store: &EngineStore,
        view: &ViewJob,
        _now: Timestamp,
    ) -> Result<SparseGeoViewMaterialization, ValidationError> {
        view.validate()?;
        let viewport = view.viewport.ok_or_else(|| {
            ValidationError::InvalidState("sparse geo materialization requires viewport".into())
        })?;

        let mut features = Vec::new();

        for entity_id in &view.entities {
            if let Some(node) = store.node(*entity_id) {
                if let Some(position) = node.position.filter(|coord| viewport.contains(*coord)) {
                    features.push(SparseGeoFeature {
                        entity_id: node.id,
                        label: node.label.clone(),
                        kind_label: format!("{:?}", node.kind),
                        geometry: SparseGeoGeometry::Point(position),
                        metrics: collect_metrics(store, node.id, &view.metrics),
                    });
                }
            }
            if let Some(boundary) = store.boundary(*entity_id) {
                if let Some(extent) = boundary.extent.filter(|extent| extent.intersects(viewport)) {
                    features.push(SparseGeoFeature {
                        entity_id: boundary.id,
                        label: boundary.label.clone(),
                        kind_label: format!("{:?}", boundary.kind),
                        geometry: SparseGeoGeometry::BBox(extent),
                        metrics: collect_metrics(store, boundary.id, &view.metrics),
                    });
                }
            }
            if let Some(edge) = store.edge(*entity_id) {
                if let Some(line) = edge_coords_in_view(store, edge, viewport) {
                    features.push(SparseGeoFeature {
                        entity_id: edge.id,
                        label: format!("{:?}", edge.kind),
                        kind_label: format!("{:?}", edge.kind),
                        geometry: SparseGeoGeometry::LineString(line),
                        metrics: collect_metrics(store, edge.id, &view.metrics),
                    });
                }
            }
        }

        Ok(SparseGeoViewMaterialization {
            view_id: view.id,
            viewport,
            features,
        })
    }
}

fn collect_metrics(
    store: &EngineStore,
    entity_id: EntityId,
    metric_ids: &[MetricId],
) -> Vec<MaterializedMetricValue> {
    metric_ids
        .iter()
        .filter_map(|metric_id| {
            let latest = store.latest_value(entity_id, *metric_id)?;
            let definition = store.metric(*metric_id)?;
            Some(MaterializedMetricValue {
                metric_id: *metric_id,
                metric_name: definition.name.clone(),
                display_value: sample_value_to_string(&latest.value),
                unit: definition.unit.clone(),
                timestamp: latest.timestamp,
            })
        })
        .collect()
}

fn edge_coords_in_view(
    store: &EngineStore,
    edge: &Edge,
    viewport: GeoBBox,
) -> Option<Vec<GeoCoord>> {
    let mut coords = Vec::new();
    if let Some(position) = store.node(edge.source).and_then(|node| node.position) {
        coords.push(position);
    }
    coords.extend(edge.waypoints.iter().copied());
    if let Some(position) = store.node(edge.target).and_then(|node| node.position) {
        coords.push(position);
    }

    if coords.len() < 2 {
        return None;
    }
    if coords.iter().any(|coord| viewport.contains(*coord)) {
        Some(coords)
    } else {
        None
    }
}

fn build_properties(feature: &SparseGeoFeature) -> Value {
    let mut properties = Map::new();
    properties.insert("label".into(), Value::String(feature.label.clone()));
    properties.insert("kind".into(), Value::String(feature.kind_label.clone()));

    let mut metrics = Map::new();
    for metric in &feature.metrics {
        metrics.insert(
            metric.metric_name.clone(),
            json!({
                "metric_id": metric.metric_id.to_string(),
                "value": metric.display_value,
                "unit": metric.unit,
                "timestamp": metric.timestamp.to_rfc3339(),
            }),
        );
    }
    properties.insert("metrics".into(), Value::Object(metrics));
    Value::Object(properties)
}

fn geometry_to_geojson(geometry: &SparseGeoGeometry) -> Value {
    match geometry {
        SparseGeoGeometry::Point(coord) => json!({
            "type": "Point",
            "coordinates": [coord.lon, coord.lat],
        }),
        SparseGeoGeometry::LineString(coords) => json!({
            "type": "LineString",
            "coordinates": coords.iter().map(|coord| vec![coord.lon, coord.lat]).collect::<Vec<_>>(),
        }),
        SparseGeoGeometry::BBox(bbox) => json!({
            "type": "Polygon",
            "coordinates": [[
                [bbox.min_lon, bbox.min_lat],
                [bbox.max_lon, bbox.min_lat],
                [bbox.max_lon, bbox.max_lat],
                [bbox.min_lon, bbox.max_lat],
                [bbox.min_lon, bbox.min_lat]
            ]],
        }),
    }
}

fn sample_value_to_string(value: &crate::metric::SampleValue) -> String {
    match value {
        crate::metric::SampleValue::Numeric(v) => format!("{v}"),
        crate::metric::SampleValue::Code(v) => v.clone(),
        crate::metric::SampleValue::Flag(v) => v.to_string(),
        crate::metric::SampleValue::Missing => "missing".into(),
    }
}
