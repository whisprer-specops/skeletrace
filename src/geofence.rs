// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Geofences: which nodes of a view fall inside an area, and the area a set of nodes covers.
//!
//! Fences come as GeoJSON polygons (RFC 7946), the format GIS tools and geojson.io save: longitude first,
//! then latitude; an outer ring, then any holes. A node inside a hole is outside the fence, and a node on a
//! hole's edge is on the fence's edge. Several polygons in one fence (a `MultiPolygon`) count as one area.
//!
//! Edges are taken as straight lines on a latitude/longitude map, which suits areas up to a few hundred
//! kilometres across. Rings that cross the ±180° meridian, or run too close to a pole, aren't supported: a ring
//! spanning more than 180° of longitude is refused. Areas are in square kilometres on the Earth (a sphere of
//! the mean radius), from `skeletrace-algo`'s equal-area formula.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use skeletrace_algo::geo::{convex_hull, polygon_area_m2, Fence, Placement};

use crate::materialize::TopologyViewMaterialization;
use crate::spatial::GeoCoord;
use crate::types::EntityId;

/// One polygon of a fence: an outer ring and any holes, as corners in order. Rings may repeat their first
/// corner at the end, as GeoJSON does, or not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FencePolygon {
    pub outer: Vec<GeoCoord>,
    #[serde(default)]
    pub holes: Vec<Vec<GeoCoord>>,
}

/// A named area made of one or more polygons.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeoFence {
    pub name: String,
    pub polygons: Vec<FencePolygon>,
}

/// Why fences couldn't be read or used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FenceError {
    /// The GeoJSON isn't shaped as RFC 7946 describes.
    NotGeoJson(String),
    /// The GeoJSON holds no polygons.
    NoPolygons,
    /// A ring can't be used as given.
    BadRing { fence: String, why: String },
}

impl fmt::Display for FenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotGeoJson(why) => write!(f, "not usable GeoJSON: {why}"),
            Self::NoPolygons => write!(f, "the GeoJSON has no polygons to use as fences"),
            Self::BadRing { fence, why } => write!(f, "fence {fence:?}: {why}"),
        }
    }
}

impl std::error::Error for FenceError {}

/// A node of the view, with its position when it has a usable one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeoNode {
    pub entity_id: EntityId,
    pub label: String,
    pub kind_label: String,
    pub position: Option<GeoCoord>,
}

/// One fence's result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FenceResult {
    pub name: String,
    /// The fence's area in square kilometres, holes taken out.
    pub area_km2: f64,
    /// Nodes strictly inside.
    pub inside: Vec<GeoNode>,
    /// Nodes exactly on an edge, as the numbers are given.
    pub on_edge: Vec<GeoNode>,
}

/// The result of [`fence_check`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FenceCheck {
    /// One result per fence, in the order given. A node can be in several.
    pub fences: Vec<FenceResult>,
    /// Nodes with a position that are in no fence and on no edge.
    pub outside_all: Vec<GeoNode>,
    /// Nodes without a usable position, which can't be placed.
    pub no_position: Vec<GeoNode>,
}

/// The result of [`cover_area`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoverArea {
    /// How many nodes had a usable position.
    pub positioned: usize,
    /// The nodes at the corners of the outline, anticlockwise on a map. Fewer than three when the nodes all
    /// sit on one spot or one line.
    pub outline: Vec<GeoNode>,
    /// The area inside the outline in square kilometres; 0 with fewer than three corners.
    pub area_km2: f64,
    /// The outline as a GeoJSON `Feature` with a `Polygon`, ready to load on a map; `None` with fewer than
    /// three corners.
    pub geojson: Option<Value>,
    /// Set when the nodes span more than 180° of longitude, where a flat outline means little.
    pub warning: Option<String>,
    /// Nodes without a usable position, left out.
    pub no_position: Vec<GeoNode>,
}

fn not_geojson(why: impl Into<String>) -> FenceError {
    FenceError::NotGeoJson(why.into())
}

/// Reads fences from GeoJSON: a `Polygon` or `MultiPolygon`, a `Feature` holding one, or a
/// `FeatureCollection` of them. Each feature is one fence, named by its `name` property when it has one, and
/// "fence 1", "fence 2" and so on otherwise. Features that aren't polygons are skipped.
pub fn fences_from_geojson(value: &Value) -> Result<Vec<GeoFence>, FenceError> {
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| not_geojson("missing \"type\""))?;
    let mut fences: Vec<GeoFence> = Vec::new();
    match kind {
        "FeatureCollection" => {
            let features = value
                .get("features")
                .and_then(Value::as_array)
                .ok_or_else(|| not_geojson("a FeatureCollection needs a \"features\" array"))?;
            for feature in features {
                if let Some(fence) = fence_from_feature(feature, fences.len() + 1)? {
                    fences.push(fence);
                }
            }
        }
        "Feature" => fences.extend(fence_from_feature(value, 1)?),
        "Polygon" | "MultiPolygon" => {
            let name = "fence 1".to_string();
            if let Some(polygons) = polygons_from_geometry(value, &name)? {
                fences.push(GeoFence { name, polygons });
            }
        }
        other => return Err(not_geojson(format!("type {other:?} can't hold polygons"))),
    }
    fences.retain(|fence| !fence.polygons.is_empty());
    if fences.is_empty() {
        return Err(FenceError::NoPolygons);
    }
    Ok(fences)
}

fn fence_from_feature(feature: &Value, number: usize) -> Result<Option<GeoFence>, FenceError> {
    if feature.get("type").and_then(Value::as_str) != Some("Feature") {
        return Err(not_geojson(
            "every member of \"features\" must be a Feature",
        ));
    }
    let name = feature
        .get("properties")
        .and_then(|p| p.get("name"))
        .and_then(Value::as_str)
        .map_or_else(|| format!("fence {number}"), str::to_string);
    let Some(geometry) = feature.get("geometry").filter(|g| !g.is_null()) else {
        return Ok(None);
    };
    Ok(polygons_from_geometry(geometry, &name)?.map(|polygons| GeoFence { name, polygons }))
}

/// The polygons of a `Polygon` or `MultiPolygon` geometry; `None` for any other geometry.
fn polygons_from_geometry(
    geometry: &Value,
    name: &str,
) -> Result<Option<Vec<FencePolygon>>, FenceError> {
    let coordinates = geometry.get("coordinates");
    let polygon_list: Vec<&Value> = match geometry.get("type").and_then(Value::as_str) {
        Some("Polygon") => {
            vec![coordinates.ok_or_else(|| not_geojson("a Polygon needs coordinates"))?]
        }
        Some("MultiPolygon") => coordinates
            .and_then(Value::as_array)
            .ok_or_else(|| not_geojson("a MultiPolygon needs an array of polygons"))?
            .iter()
            .collect(),
        _ => return Ok(None),
    };
    let mut polygons = Vec::with_capacity(polygon_list.len());
    for rings in polygon_list {
        let rings = rings
            .as_array()
            .filter(|r| !r.is_empty())
            .ok_or_else(|| not_geojson("a polygon needs at least one ring"))?;
        let mut parsed = Vec::with_capacity(rings.len());
        for ring in rings {
            parsed.push(ring_from_json(ring, name)?);
        }
        let outer = parsed.remove(0);
        polygons.push(FencePolygon {
            outer,
            holes: parsed,
        });
    }
    Ok(Some(polygons))
}

fn ring_from_json(ring: &Value, name: &str) -> Result<Vec<GeoCoord>, FenceError> {
    let bad = |why: String| FenceError::BadRing {
        fence: name.to_string(),
        why,
    };
    let positions = ring
        .as_array()
        .ok_or_else(|| bad("a ring must be an array of positions".into()))?;
    let mut corners = Vec::with_capacity(positions.len());
    for position in positions {
        let numbers = position.as_array().filter(|p| p.len() >= 2);
        let (Some(lon), Some(lat)) = (
            numbers.and_then(|p| p[0].as_f64()),
            numbers.and_then(|p| p[1].as_f64()),
        ) else {
            return Err(bad(format!(
                "{position} isn't a [longitude, latitude] position"
            )));
        };
        let corner = GeoCoord::new(lat, lon, None).map_err(|err| bad(err.to_string()))?;
        corners.push(corner);
    }
    Ok(corners)
}

/// A ring ready for placing points: the algorithm crate's fence over (latitude, longitude) corners.
struct Ring {
    fence: Fence,
    corners: Vec<(f64, f64)>,
}

fn compile_ring(ring: &[GeoCoord], name: &str) -> Result<Ring, FenceError> {
    let bad = |why: &str| FenceError::BadRing {
        fence: name.to_string(),
        why: why.to_string(),
    };
    let mut corners: Vec<(f64, f64)> = ring.iter().map(|c| (c.lat, c.lon)).collect();
    if corners.len() > 1 && corners.first() == corners.last() {
        corners.pop();
    }
    for c in &corners {
        // A range check also rules out NaN and the infinities.
        if !((-90.0..=90.0).contains(&c.0) && (-180.0..=180.0).contains(&c.1)) {
            return Err(bad("a corner is outside latitude ±90° or longitude ±180°"));
        }
    }
    if corners.len() < 3 {
        return Err(bad("a ring needs at least three different corners"));
    }
    let (west, east) = corners
        .iter()
        .fold((f64::MAX, f64::MIN), |(w, e), c| (w.min(c.1), e.max(c.1)));
    if east - west > 180.0 {
        return Err(bad(
            "a ring spans more than 180° of longitude; split fences that cross the ±180° meridian",
        ));
    }
    let fence = Fence::new(&corners).ok_or_else(|| bad("a ring can't be used as a fence"))?;
    Ok(Ring { fence, corners })
}

/// A fence ready for placing points: (outer ring, holes) for each polygon.
struct CompiledFence {
    polygons: Vec<(Ring, Vec<Ring>)>,
    area_km2: f64,
}

fn compile_fence(fence: &GeoFence) -> Result<CompiledFence, FenceError> {
    if fence.polygons.is_empty() {
        return Err(FenceError::BadRing {
            fence: fence.name.clone(),
            why: "the fence has no polygons".into(),
        });
    }
    let mut polygons = Vec::with_capacity(fence.polygons.len());
    let mut area_m2 = 0.0;
    for polygon in &fence.polygons {
        let outer = compile_ring(&polygon.outer, &fence.name)?;
        let mut holes = Vec::with_capacity(polygon.holes.len());
        area_m2 += polygon_area_m2(&outer.corners).unwrap_or(0.0);
        for hole in &polygon.holes {
            let hole = compile_ring(hole, &fence.name)?;
            area_m2 -= polygon_area_m2(&hole.corners).unwrap_or(0.0);
            holes.push(hole);
        }
        polygons.push((outer, holes));
    }
    Ok(CompiledFence {
        polygons,
        area_km2: area_m2.max(0.0) / 1e6,
    })
}

/// Inside, on the edge of, or outside a whole fence, holes and all.
fn place(fence: &CompiledFence, at: (f64, f64)) -> Placement {
    let mut on_edge = false;
    for (outer, holes) in &fence.polygons {
        match outer.fence.locate(at) {
            Placement::Outside => {}
            Placement::OnEdge => on_edge = true,
            Placement::Inside => {
                let (mut in_hole, mut on_hole_edge) = (false, false);
                for hole in holes {
                    match hole.fence.locate(at) {
                        Placement::Inside => in_hole = true,
                        Placement::OnEdge => on_hole_edge = true,
                        Placement::Outside => {}
                    }
                }
                // Inside any one polygon is inside the fence, whatever the others say.
                if on_hole_edge {
                    on_edge = true;
                } else if !in_hole {
                    return Placement::Inside;
                }
            }
        }
    }
    if on_edge {
        Placement::OnEdge
    } else {
        Placement::Outside
    }
}

/// The view's nodes, each with its position if that position is usable.
fn view_nodes(view: &TopologyViewMaterialization) -> Vec<GeoNode> {
    view.nodes
        .iter()
        .map(|node| GeoNode {
            entity_id: node.entity_id,
            label: node.label.clone(),
            kind_label: node.kind_label.clone(),
            position: node
                .position
                .filter(|p| (-90.0..=90.0).contains(&p.lat) && (-180.0..=180.0).contains(&p.lon)),
        })
        .collect()
}

/// Which of the view's nodes are inside, on the edge of, or outside each fence. O(nodes × corners).
pub fn fence_check(
    view: &TopologyViewMaterialization,
    fences: &[GeoFence],
) -> Result<FenceCheck, FenceError> {
    if fences.is_empty() {
        return Err(FenceError::NoPolygons);
    }
    let compiled: Vec<CompiledFence> =
        fences.iter().map(compile_fence).collect::<Result<_, _>>()?;
    let mut results: Vec<FenceResult> = fences
        .iter()
        .zip(&compiled)
        .map(|(fence, c)| FenceResult {
            name: fence.name.clone(),
            area_km2: c.area_km2,
            inside: Vec::new(),
            on_edge: Vec::new(),
        })
        .collect();
    let (mut outside_all, mut no_position) = (Vec::new(), Vec::new());
    for node in view_nodes(view) {
        let Some(position) = node.position else {
            no_position.push(node);
            continue;
        };
        let mut anywhere = false;
        for (fence, result) in compiled.iter().zip(results.iter_mut()) {
            match place(fence, (position.lat, position.lon)) {
                Placement::Inside => {
                    result.inside.push(node.clone());
                    anywhere = true;
                }
                Placement::OnEdge => {
                    result.on_edge.push(node.clone());
                    anywhere = true;
                }
                Placement::Outside => {}
            }
        }
        if !anywhere {
            outside_all.push(node);
        }
    }
    Ok(FenceCheck {
        fences: results,
        outside_all,
        no_position,
    })
}

/// The tightest outline around every positioned node of the view (the convex hull), and its area.
/// O(N log N).
pub fn cover_area(view: &TopologyViewMaterialization) -> CoverArea {
    let (mut placed, mut no_position) = (Vec::new(), Vec::new());
    for node in view_nodes(view) {
        if node.position.is_some() {
            placed.push(node);
        } else {
            no_position.push(node);
        }
    }
    // (longitude, latitude), so the outline runs anticlockwise on a map, as GeoJSON expects.
    let points: Vec<(f64, f64)> = placed
        .iter()
        .filter_map(|n| n.position.map(|p| (p.lon, p.lat)))
        .collect();
    let hull = convex_hull(&points).unwrap_or_default();
    let outline: Vec<GeoNode> = hull.iter().map(|&i| placed[i].clone()).collect();
    let (mut area_km2, mut geojson) = (0.0, None);
    if outline.len() >= 3 {
        let corners: Vec<(f64, f64)> = hull.iter().map(|&i| (points[i].1, points[i].0)).collect();
        area_km2 = polygon_area_m2(&corners).unwrap_or(0.0) / 1e6;
        let mut ring: Vec<[f64; 2]> = hull.iter().map(|&i| [points[i].0, points[i].1]).collect();
        ring.push(ring[0]);
        geojson = Some(serde_json::json!({
            "type": "Feature",
            "properties": {
                "name": "cover area",
                "nodes": placed.len(),
                "area_km2": area_km2,
            },
            "geometry": { "type": "Polygon", "coordinates": [ring] },
        }));
    }
    let warning = {
        let (west, east) = points
            .iter()
            .fold((f64::MAX, f64::MIN), |(w, e), p| (w.min(p.0), e.max(p.0)));
        (!points.is_empty() && east - west > 180.0).then(|| {
            "the nodes span more than 180° of longitude, so a flat outline means little".to_string()
        })
    };
    CoverArea {
        positioned: placed.len(),
        outline,
        area_km2,
        geojson,
        warning,
        no_position,
    }
}
