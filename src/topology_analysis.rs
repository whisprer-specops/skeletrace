// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Chokepoints and bridges in a materialized topology view.
//!
//! This is the structure pane's "where must everything pass through?" answer.
//! Edges are treated as undirected connections: a relay is a chokepoint
//! whichever way traffic flows over it. An articulation point is a node whose
//! removal splits its component; a bridge is an edge whose removal does the
//! same. Tarjan's low-link method, run iteratively (no recursion-depth limit),
//! The traversal is O(nodes + edges); report ranking adds sorting work.
//! Parallel edges are handled correctly (a doubled link is
//! never a bridge); self-loops are ignored.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::materialize::TopologyViewMaterialization;
use crate::types::EntityId;

/// A node whose removal disconnects part of the graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chokepoint {
    pub entity_id: EntityId,
    pub label: String,
    pub kind_label: String,
    /// Edge endpoints at this node (parallel edges counted separately).
    pub degree: usize,
    /// How many pieces its component falls into without it (always >= 2).
    pub splits_into: usize,
}

/// An edge whose removal disconnects part of the graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bridge {
    pub entity_id: EntityId,
    pub source: EntityId,
    pub target: EntityId,
    pub kind_label: String,
}

/// Connectivity summary of one topology view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyAnalysis {
    pub node_count: usize,
    pub edge_count: usize,
    /// Connected components, largest first; members in DFS discovery order.
    pub components: Vec<Vec<EntityId>>,
    /// Articulation points, most disruptive first.
    pub chokepoints: Vec<Chokepoint>,
    /// Bridge edges, in view order.
    pub bridges: Vec<Bridge>,
    /// Edges whose endpoints are not both in the view; left out of the analysis.
    pub dangling_edges: Vec<EntityId>,
}

const UNSEEN: usize = usize::MAX;

/// Find components, chokepoints and bridges in `view`.
#[must_use]
pub fn analyze_topology(view: &TopologyViewMaterialization) -> TopologyAnalysis {
    let n = view.nodes.len();
    let mut index: HashMap<EntityId, usize> = HashMap::with_capacity(n);
    for (i, node) in view.nodes.iter().enumerate() {
        index.entry(node.entity_id).or_insert(i);
    }

    // adjacency[u] = (neighbour, edge index); both directions for every edge.
    let mut adjacency: Vec<Vec<(usize, usize)>> = vec![Vec::new(); n];
    let mut dangling_edges = Vec::new();
    for (e, edge) in view.edges.iter().enumerate() {
        match (index.get(&edge.source), index.get(&edge.target)) {
            (Some(&a), Some(&b)) => {
                if a != b {
                    adjacency[a].push((b, e));
                    adjacency[b].push((a, e));
                }
            }
            _ => dangling_edges.push(edge.entity_id),
        }
    }

    let mut disc = vec![UNSEEN; n];
    let mut low = vec![0_usize; n];
    let mut separated = vec![0_usize; n];
    let mut is_root = vec![false; n];
    let mut bridge_edges: Vec<usize> = Vec::new();
    let mut components: Vec<Vec<usize>> = Vec::new();
    let mut timer = 0_usize;
    // DFS frame: (node, edge we arrived by, next adjacency slot to try).
    let mut stack: Vec<(usize, Option<usize>, usize)> = Vec::new();

    for root in 0..n {
        if disc[root] != UNSEEN {
            continue;
        }
        is_root[root] = true;
        disc[root] = timer;
        low[root] = timer;
        timer += 1;
        let mut members = vec![root];
        stack.push((root, None, 0));
        while let Some(frame) = stack.last_mut() {
            let (u, arrived_by) = (frame.0, frame.1);
            if let Some(&(v, e)) = adjacency[u].get(frame.2) {
                frame.2 += 1;
                if Some(e) == arrived_by {
                    continue;
                }
                if disc[v] == UNSEEN {
                    disc[v] = timer;
                    low[v] = timer;
                    timer += 1;
                    members.push(v);
                    stack.push((v, Some(e), 0));
                } else {
                    low[u] = low[u].min(disc[v]);
                }
            } else {
                stack.pop();
                if let Some(&(parent, _, _)) = stack.last() {
                    low[parent] = low[parent].min(low[u]);
                    if low[u] >= disc[parent] {
                        separated[parent] += 1;
                    }
                    if let (true, Some(e)) = (low[u] > disc[parent], arrived_by) {
                        bridge_edges.push(e);
                    }
                }
            }
        }
        components.push(members);
    }

    let mut chokepoints: Vec<Chokepoint> = (0..n)
        .filter_map(|u| {
            let (articulation, splits_into) = if is_root[u] {
                (separated[u] >= 2, separated[u])
            } else {
                (separated[u] >= 1, separated[u] + 1)
            };
            articulation.then(|| {
                let node = &view.nodes[u];
                Chokepoint {
                    entity_id: node.entity_id,
                    label: node.label.clone(),
                    kind_label: node.kind_label.clone(),
                    degree: adjacency[u].len(),
                    splits_into,
                }
            })
        })
        .collect();
    chokepoints.sort_by(|a, b| {
        b.splits_into
            .cmp(&a.splits_into)
            .then(b.degree.cmp(&a.degree))
    });

    bridge_edges.sort_unstable();
    let bridges = bridge_edges
        .into_iter()
        .map(|e| {
            let edge = &view.edges[e];
            Bridge {
                entity_id: edge.entity_id,
                source: edge.source,
                target: edge.target,
                kind_label: edge.kind_label.clone(),
            }
        })
        .collect();

    components.sort_by_key(|members| std::cmp::Reverse(members.len()));
    let components = components
        .into_iter()
        .map(|members| {
            members
                .into_iter()
                .map(|u| view.nodes[u].entity_id)
                .collect()
        })
        .collect();

    TopologyAnalysis {
        node_count: n,
        edge_count: view.edges.len(),
        components,
        chokepoints,
        bridges,
        dangling_edges,
    }
}
