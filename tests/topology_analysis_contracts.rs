// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Chokepoint and bridge detection on hand-built topology views.

use skeletrace::{
    analyze_topology, EntityId, GeometryMode, TopologyEdgeView, TopologyNodeView,
    TopologyViewMaterialization, ViewJobId,
};

fn view(nodes: usize, pairs: &[(usize, usize)]) -> (Vec<EntityId>, TopologyViewMaterialization) {
    let ids: Vec<EntityId> = (0..nodes).map(|_| EntityId::new()).collect();
    let view = TopologyViewMaterialization {
        view_id: ViewJobId::new(),
        nodes: ids
            .iter()
            .enumerate()
            .map(|(i, id)| TopologyNodeView {
                entity_id: *id,
                label: format!("n{i}"),
                kind_label: "Relay".into(),
                position: None,
                metrics: Vec::new(),
            })
            .collect(),
        edges: pairs
            .iter()
            .map(|(a, b)| TopologyEdgeView {
                entity_id: EntityId::new(),
                source: ids[*a],
                target: ids[*b],
                kind_label: "Link".into(),
                geometry_mode: GeometryMode::Abstract,
                metrics: Vec::new(),
            })
            .collect(),
        boundaries: Vec::new(),
    };
    (ids, view)
}

#[test]
fn a_path_has_inner_chokepoints_and_all_bridges() {
    let (ids, v) = view(4, &[(0, 1), (1, 2), (2, 3)]);
    let a = analyze_topology(&v);
    assert_eq!(a.components.len(), 1);
    let mut choke: Vec<EntityId> = a.chokepoints.iter().map(|c| c.entity_id).collect();
    choke.sort();
    let mut expected = vec![ids[1], ids[2]];
    expected.sort();
    assert_eq!(choke, expected);
    assert!(a
        .chokepoints
        .iter()
        .all(|c| c.splits_into == 2 && c.degree == 2));
    assert_eq!(a.bridges.len(), 3);
}

#[test]
fn a_triangle_has_no_weak_points() {
    let (_, v) = view(3, &[(0, 1), (1, 2), (2, 0)]);
    let a = analyze_topology(&v);
    assert!(a.chokepoints.is_empty());
    assert!(a.bridges.is_empty());
}

#[test]
fn a_bowtie_centre_is_a_chokepoint_without_bridges() {
    let (ids, v) = view(5, &[(0, 1), (1, 2), (2, 0), (2, 3), (3, 4), (4, 2)]);
    let a = analyze_topology(&v);
    assert_eq!(a.chokepoints.len(), 1);
    assert_eq!(a.chokepoints[0].entity_id, ids[2]);
    assert_eq!(a.chokepoints[0].splits_into, 2);
    assert_eq!(a.chokepoints[0].degree, 4);
    assert!(a.bridges.is_empty());
}

#[test]
fn a_doubled_link_is_not_a_bridge() {
    let (ids, v) = view(3, &[(0, 1), (0, 1), (1, 2)]);
    let a = analyze_topology(&v);
    assert_eq!(a.bridges.len(), 1);
    assert_eq!((a.bridges[0].source, a.bridges[0].target), (ids[1], ids[2]));
    assert_eq!(a.chokepoints.len(), 1);
    assert_eq!(a.chokepoints[0].entity_id, ids[1]);
    assert_eq!(a.chokepoints[0].degree, 3);
}

#[test]
fn a_star_centre_ranks_first() {
    let (ids, v) = view(7, &[(0, 1), (0, 2), (0, 3), (0, 4), (5, 6), (6, 1)]);
    let a = analyze_topology(&v);
    assert_eq!(a.chokepoints[0].entity_id, ids[0]);
    assert_eq!(a.chokepoints[0].splits_into, 4);
    assert_eq!(a.bridges.len(), 6);
}

#[test]
fn dangling_edges_isolated_nodes_and_self_loops() {
    let (ids, mut v) = view(3, &[(0, 1), (2, 2)]);
    v.edges.push(TopologyEdgeView {
        entity_id: EntityId::new(),
        source: ids[0],
        target: EntityId::new(),
        kind_label: "Link".into(),
        geometry_mode: GeometryMode::Abstract,
        metrics: Vec::new(),
    });
    let a = analyze_topology(&v);
    assert_eq!(a.edge_count, 3);
    assert_eq!(a.dangling_edges.len(), 1);
    assert_eq!(a.components.len(), 2);
    assert_eq!(a.components[0].len(), 2);
    assert_eq!(a.bridges.len(), 1);
    assert!(a.chokepoints.is_empty());
}

#[test]
fn a_long_chain_does_not_overflow_the_stack() {
    let n = 200_000;
    let pairs: Vec<(usize, usize)> = (1..n).map(|i| (i - 1, i)).collect();
    let (_, v) = view(n, &pairs);
    let a = analyze_topology(&v);
    assert_eq!(a.bridges.len(), n - 1);
    assert_eq!(a.chokepoints.len(), n - 2);
}
