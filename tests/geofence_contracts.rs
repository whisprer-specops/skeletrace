// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Contract tests for geofences and covered areas: GeoJSON reading, placement, areas, the operator API and
//! the command line.

use std::process::Command;

use chrono::Utc;
use serde_json::json;
use skeletrace::{
    cover_area, fence_check, fences_from_geojson, CliCommand, Confidence, EngineConfig, EntityId,
    EntityStatus, FenceError, GeoCoord, Node, NodeKind, OperatorApi, OperatorRequest,
    OperatorResponse, Priority, SkeletraceEngine, SnapshotExporter, TimeRange, TopologyNodeView,
    TopologyViewMaterialization, ViewJob, ViewJobId, ViewKind,
};

/// A view of nodes at (latitude, longitude) positions; `None` for a node without one.
fn view(positions: &[Option<(f64, f64)>]) -> TopologyViewMaterialization {
    TopologyViewMaterialization {
        view_id: ViewJobId::new(),
        nodes: positions
            .iter()
            .enumerate()
            .map(|(i, p)| TopologyNodeView {
                entity_id: EntityId::new(),
                label: format!("n{i}"),
                kind_label: "Relay".into(),
                position: p.map(|(lat, lon)| GeoCoord::new(lat, lon, None).unwrap()),
                metrics: Vec::new(),
            })
            .collect(),
        edges: Vec::new(),
        boundaries: Vec::new(),
    }
}

/// A closed GeoJSON ring from (longitude, latitude) corners.
fn ring(corners: &[(f64, f64)]) -> serde_json::Value {
    let mut positions: Vec<[f64; 2]> = corners.iter().map(|&(lon, lat)| [lon, lat]).collect();
    positions.push(positions[0]);
    json!(positions)
}

fn labels(nodes: &[skeletrace::GeoNode]) -> Vec<&str> {
    nodes.iter().map(|n| n.label.as_str()).collect()
}

#[test]
fn geojson_shapes_are_read() {
    let square = ring(&[
        (-1.35, 50.70),
        (-0.95, 50.70),
        (-0.95, 50.85),
        (-1.35, 50.85),
    ]);
    let polygon = json!({ "type": "Polygon", "coordinates": [square] });
    let fences = fences_from_geojson(&polygon).unwrap();
    assert_eq!(fences.len(), 1);
    assert_eq!(fences[0].name, "fence 1");
    assert_eq!(fences[0].polygons[0].outer.len(), 5);
    assert_eq!(fences[0].polygons[0].outer[0].lat, 50.70);
    assert_eq!(fences[0].polygons[0].outer[0].lon, -1.35);
    let collection = json!({
        "type": "FeatureCollection",
        "features": [
            { "type": "Feature", "properties": { "name": "Solent" }, "geometry": polygon },
            { "type": "Feature", "properties": {}, "geometry": { "type": "Point", "coordinates": [0.0, 51.0] } },
            { "type": "Feature", "properties": null, "geometry": {
                "type": "MultiPolygon", "coordinates": [[square], [square]] } },
        ]
    });
    let fences = fences_from_geojson(&collection).unwrap();
    let names: Vec<&str> = fences.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["Solent", "fence 2"]);
    assert_eq!(fences[1].polygons.len(), 2);

    for bad in [
        json!({}),
        json!({ "type": "LineString", "coordinates": [[0, 0], [1, 1]] }),
        json!({ "type": "FeatureCollection" }),
        json!({ "type": "FeatureCollection", "features": [{ "type": "Nope" }] }),
    ] {
        assert!(
            matches!(fences_from_geojson(&bad), Err(FenceError::NotGeoJson(_))),
            "{bad}"
        );
    }
    let only_points = json!({ "type": "FeatureCollection", "features": [
        { "type": "Feature", "properties": {}, "geometry": { "type": "Point", "coordinates": [0, 0] } } ] });
    assert_eq!(
        fences_from_geojson(&only_points),
        Err(FenceError::NoPolygons)
    );
    for bad_ring in [
        json!([["a", "b"], [1, 1], [1, 0], ["a", "b"]]),
        json!([[0, 95], [1, 1], [1, 0], [0, 95]]),
        json!([[0, 0], [200, 1], [1, 0], [0, 0]]),
    ] {
        let polygon = json!({ "type": "Polygon", "coordinates": [bad_ring] });
        assert!(
            matches!(
                fences_from_geojson(&polygon),
                Err(FenceError::BadRing { .. })
            ),
            "{polygon}"
        );
    }
}

#[test]
fn holes_edges_and_nodes_without_positions() {
    // A 4° square with a 2° hole in the middle, both centred on (0, 0).
    let outer = ring(&[(-2.0, -2.0), (2.0, -2.0), (2.0, 2.0), (-2.0, 2.0)]);
    let hole = ring(&[(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]);
    let fences =
        fences_from_geojson(&json!({ "type": "Polygon", "coordinates": [outer, hole] })).unwrap();
    let v = view(&[
        Some((1.5, 0.0)), // n0: between the rings
        Some((0.0, 0.0)), // n1: in the hole
        Some((2.0, 0.0)), // n2: on the outer edge
        Some((1.0, 0.5)), // n3: on the hole's edge
        Some((3.0, 0.0)), // n4: outside
        None,             // n5: no position
    ]);
    let check = fence_check(&v, &fences).unwrap();
    let fence = &check.fences[0];
    assert_eq!(labels(&fence.inside), ["n0"]);
    assert_eq!(labels(&fence.on_edge), ["n2", "n3"]);
    assert_eq!(labels(&check.outside_all), ["n1", "n4"]);
    assert_eq!(labels(&check.no_position), ["n5"]);
    // The hole is taken out of the area: about 4² − 2² = 12 square degrees near the equator.
    let degree_km = 6_371.008_8 * std::f64::consts::PI / 180.0;
    let rough = 12.0 * degree_km * degree_km;
    assert!(
        (fence.area_km2 - rough).abs() / rough < 0.002,
        "{}",
        fence.area_km2
    );
}

#[test]
fn a_node_can_sit_in_several_fences_and_multipolygons_are_one_area() {
    let west = ring(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]);
    let east = ring(&[(5.0, 0.0), (6.0, 0.0), (6.0, 1.0), (5.0, 1.0)]);
    let wide = ring(&[(0.0, 0.0), (6.0, 0.0), (6.0, 1.0), (0.0, 1.0)]);
    let collection = json!({ "type": "FeatureCollection", "features": [
        { "type": "Feature", "properties": { "name": "islands" },
          "geometry": { "type": "MultiPolygon", "coordinates": [[west], [east]] } },
        { "type": "Feature", "properties": { "name": "strip" },
          "geometry": { "type": "Polygon", "coordinates": [wide] } },
    ]});
    let fences = fences_from_geojson(&collection).unwrap();
    let v = view(&[Some((0.5, 5.5)), Some((0.5, 3.0))]);
    let check = fence_check(&v, &fences).unwrap();
    assert_eq!(labels(&check.fences[0].inside), ["n0"]);
    assert_eq!(labels(&check.fences[1].inside), ["n0", "n1"]);
    assert!(check.outside_all.is_empty());
    let one_square = check.fences[0].area_km2 / 2.0;
    assert!((check.fences[1].area_km2 / one_square - 6.0).abs() < 1e-9);
}

/// Winding number of `outline` around `p` (Sunday's method), for an outline that doesn't cross itself.
fn winding(outline: &[(f64, f64)], p: (f64, f64)) -> i32 {
    let side = |a: (f64, f64), b: (f64, f64), p: (f64, f64)| {
        (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0)
    };
    let mut w = 0;
    for i in 0..outline.len() {
        let (a, b) = (outline[i], outline[(i + 1) % outline.len()]);
        if a.1 <= p.1 {
            if b.1 > p.1 && side(a, b, p) > 0.0 {
                w += 1;
            }
        } else if b.1 <= p.1 && side(a, b, p) < 0.0 {
            w -= 1;
        }
    }
    w
}

#[test]
fn random_outlines_match_winding_numbers() {
    let mut state: u64 = 51;
    let mut unit = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 11) as f64 / (1u64 << 53) as f64
    };
    for _ in 0..200 {
        // A star-shaped outline round (51 N, 1 W): corners at increasing angles never cross.
        let corners = 3 + (unit() * 10.0) as usize;
        let mut angles: Vec<f64> = (0..corners)
            .map(|_| unit() * std::f64::consts::TAU)
            .collect();
        angles.sort_by(f64::total_cmp);
        let outline: Vec<(f64, f64)> = angles
            .iter()
            .map(|a| {
                let r = 0.05 + 0.45 * unit();
                (-1.0 + r * a.cos(), 51.0 + r * a.sin())
            })
            .collect();
        let fences =
            fences_from_geojson(&json!({ "type": "Polygon", "coordinates": [ring(&outline)] }))
                .unwrap();
        let points: Vec<(f64, f64)> = (0..40)
            .map(|_| (-1.6 + 1.2 * unit(), 50.4 + 1.2 * unit()))
            .collect();
        let v = view(
            &points
                .iter()
                .map(|&(lon, lat)| Some((lat, lon)))
                .collect::<Vec<_>>(),
        );
        let check = fence_check(&v, &fences).unwrap();
        for (i, &p) in points.iter().enumerate() {
            let label = format!("n{i}");
            let inside = labels(&check.fences[0].inside).contains(&label.as_str());
            let on_edge = labels(&check.fences[0].on_edge).contains(&label.as_str());
            if !on_edge {
                assert_eq!(inside, winding(&outline, p) != 0, "{outline:?} {p:?}");
            }
        }
    }
}

#[test]
fn areas_match_the_closed_form_for_boxes() {
    let r = 6_371_008.8_f64;
    let box_at_equator = ring(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]);
    let fences =
        fences_from_geojson(&json!({ "type": "Polygon", "coordinates": [box_at_equator] }))
            .unwrap();
    let check = fence_check(&view(&[]), &fences).unwrap();
    let want = r * r * 1f64.to_radians() * 1f64.to_radians().sin() / 1e6;
    assert!((check.fences[0].area_km2 - want).abs() / want < 1e-9);
    assert!((12_300.0..12_400.0).contains(&want));
}

#[test]
fn covered_area_outlines_the_nodes() {
    // A 1° square of nodes, one in the middle, one with no position.
    let v = view(&[
        Some((50.0, -2.0)),
        Some((50.0, -1.0)),
        Some((51.0, -1.0)),
        Some((51.0, -2.0)),
        Some((50.5, -1.5)),
        None,
    ]);
    let cover = cover_area(&v);
    assert_eq!(cover.positioned, 5);
    // Anticlockwise on a map, starting from the westernmost, then southernmost.
    assert_eq!(labels(&cover.outline), ["n0", "n1", "n2", "n3"]);
    assert_eq!(labels(&cover.no_position), ["n5"]);
    let r = 6_371.008_8_f64;
    let want = r * r * 1f64.to_radians() * (51f64.to_radians().sin() - 50f64.to_radians().sin());
    assert!((cover.area_km2 - want).abs() / want < 1e-9);
    let feature = cover.geojson.unwrap();
    let ring = feature["geometry"]["coordinates"][0].as_array().unwrap();
    assert_eq!(ring.len(), 5);
    assert_eq!(ring[0], ring[4]);
    assert_eq!(ring[0], json!([-2.0, 50.0]));
    assert!(cover.warning.is_none());
    // In a line: no area and no polygon.
    let line = cover_area(&view(&[
        Some((50.0, -3.0)),
        Some((50.1, -3.0)),
        Some((50.2, -3.0)),
    ]));
    assert_eq!(
        (line.outline.len(), line.area_km2, line.geojson.is_none()),
        (2, 0.0, true)
    );
    // Nodes either side of the world.
    let wide = cover_area(&view(&[
        Some((0.0, -170.0)),
        Some((1.0, 170.0)),
        Some((2.0, 0.0)),
    ]));
    assert!(wide.warning.is_some());
}

fn positioned_node(label: &str, lat: f64, lon: f64) -> Node {
    let now = Utc::now();
    Node {
        id: EntityId::new(),
        kind: NodeKind::Relay,
        label: label.into(),
        position: Some(GeoCoord::new(lat, lon, None).unwrap()),
        position_confidence: Confidence::new(1.0).unwrap(),
        status: EntityStatus::Active,
        priority: Priority::HIGH,
        tags: vec![],
        first_seen: now,
        last_seen: now,
    }
}

#[test]
fn the_operator_api_checks_fences_and_covered_area() {
    let mut engine = SkeletraceEngine::new(EngineConfig::default()).unwrap();
    let nodes = [
        positioned_node("MV Lantern", 50.78, -1.10),
        positioned_node("Dredger 7", 50.7815, -1.095),
        positioned_node("Fishing 22", 50.60, -1.50),
    ];
    for n in &nodes {
        engine.register_node(n.clone()).unwrap();
    }
    let dir = std::env::temp_dir().join(format!("skeletrace-fence-{}", uuid::Uuid::new_v4()));
    let exporter = SnapshotExporter::new(dir.join("exports"), None::<std::path::PathBuf>).unwrap();
    let mut api = OperatorApi::new(engine, exporter);
    let view = ViewJob {
        id: ViewJobId::new(),
        kind: ViewKind::Topology,
        entities: nodes.iter().map(|n| n.id).collect(),
        metrics: vec![],
        time_range: TimeRange::LatestOnly,
        detail_override: None,
        viewport: None,
    };
    let solent = ring(&[
        (-1.35, 50.70),
        (-0.95, 50.70),
        (-0.95, 50.85),
        (-1.35, 50.85),
    ]);
    let fences = fences_from_geojson(
        &json!({ "type": "Feature", "properties": { "name": "Solent" },
        "geometry": { "type": "Polygon", "coordinates": [solent] } }),
    )
    .unwrap();
    match api
        .execute(OperatorRequest::FenceCheck {
            view: view.clone(),
            fences,
            now: Utc::now(),
        })
        .unwrap()
    {
        OperatorResponse::FenceCheck(check) => {
            assert_eq!(check.fences[0].name, "Solent");
            let mut inside = labels(&check.fences[0].inside);
            inside.sort_unstable();
            assert_eq!(inside, ["Dredger 7", "MV Lantern"]);
            assert_eq!(labels(&check.outside_all), ["Fishing 22"]);
        }
        other => panic!("expected a fence check, got {other:?}"),
    }
    match api
        .execute(OperatorRequest::CoverArea {
            view,
            now: Utc::now(),
        })
        .unwrap()
    {
        OperatorResponse::CoverArea(cover) => {
            assert_eq!(cover.positioned, 3);
            assert_eq!(cover.outline.len(), 3);
            assert!(cover.area_km2 > 0.0);
        }
        other => panic!("expected a covered area, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_commands_need_the_right_number_of_arguments() {
    assert_eq!(
        CliCommand::parse_from(["skeletrace", "fence-check", "p.json", "v.json", "f.geojson"])
            .unwrap(),
        CliCommand::FenceCheck {
            profile_path: "p.json".into(),
            view_path: "v.json".into(),
            fences_path: "f.geojson".into(),
        }
    );
    assert_eq!(
        CliCommand::parse_from(["skeletrace", "cover-area", "p.json", "v.json"]).unwrap(),
        CliCommand::CoverArea {
            profile_path: "p.json".into(),
            view_path: "v.json".into(),
        }
    );
    for args in [
        vec!["fence-check", "p.json", "v.json"],
        vec!["fence-check", "p.json", "v.json", "f.geojson", "extra"],
        vec!["cover-area", "p.json"],
        vec!["cover-area", "p.json", "v.json", "extra"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_skeletrace"))
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "args: {args:?}");
        assert!(output.stdout.is_empty());
    }
}
