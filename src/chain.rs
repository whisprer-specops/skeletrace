// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! The strongest chain between two nodes of a topology view.
//!
//! Every link in the engine carries a confidence from 0 to 1: how sure you are that it's real. A chain of links
//! is only as believable as all of them together, so its confidence is their product. This finds the chain
//! between two nodes with the highest product: two solid links at 0.9 each (0.81 together) beat one shaky link
//! at 0.5.
//!
//! Links are treated as undirected connections, as in [`crate::topology_analysis`]. A link with confidence 0 is
//! not believed at all and is never used. Underneath is Dijkstra's method from `skeletrace-algo`: taking −ln of
//! each confidence turns the largest product into the smallest sum. O((V + E) log V).

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::materialize::TopologyViewMaterialization;
use crate::types::EntityId;

/// A node on a chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainNode {
    pub entity_id: EntityId,
    pub label: String,
    pub kind_label: String,
}

/// A link on a chain, with its confidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChainLink {
    pub entity_id: EntityId,
    pub source: EntityId,
    pub target: EntityId,
    pub kind_label: String,
    pub confidence: f64,
}

/// The most believable chain of links between two nodes, or the news that there is none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrongestChain {
    /// Where the chain starts.
    pub from: ChainNode,
    /// Where it ends.
    pub to: ChainNode,
    /// True when some chain of believed links joins the two.
    pub found: bool,
    /// The links' confidences multiplied together: how likely the whole chain is to hold, if each link holds or
    /// fails independently. `None` when no chain was found.
    pub confidence: Option<f64>,
    /// The nodes along the chain, from `from` to `to`, both included. Empty when none was found.
    pub nodes: Vec<ChainNode>,
    /// The links used, in order along the chain.
    pub links: Vec<ChainLink>,
    /// Links with confidence 0, which were never used.
    pub unbelieved_links: Vec<EntityId>,
    /// Links whose ends aren't both in the view, left out.
    pub dangling_links: Vec<EntityId>,
}

/// Why a chain couldn't be worked out.
#[derive(Debug, Clone, PartialEq)]
pub enum ChainError {
    /// No node in the view has that ID or exact label.
    UnknownNode(String),
    /// Several nodes share the label, so it doesn't pick one out.
    AmbiguousLabel {
        label: String,
        matches: Vec<EntityId>,
    },
    /// A link's confidence isn't a number from 0 to 1.
    BadConfidence { link: EntityId, value: f64 },
}

impl fmt::Display for ChainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownNode(wanted) => {
                write!(f, "no node in the view has the ID or label {wanted:?}")
            }
            Self::AmbiguousLabel { label, matches } => {
                let ids: Vec<String> = matches.iter().map(|id| id.0.to_string()).collect();
                write!(
                    f,
                    "{} nodes are labelled {label:?}; use one of their IDs instead: {}",
                    matches.len(),
                    ids.join(", ")
                )
            }
            Self::BadConfidence { link, value } => {
                write!(f, "link {} has confidence {value}, outside 0 to 1", link.0)
            }
        }
    }
}

impl std::error::Error for ChainError {}

/// The position in `view.nodes` of the node `wanted` names: its ID, or failing that, its exact label.
pub fn find_node(view: &TopologyViewMaterialization, wanted: &str) -> Result<usize, ChainError> {
    if let Ok(uuid) = uuid::Uuid::parse_str(wanted.trim()) {
        let id = EntityId::from_uuid(uuid);
        if let Some(position) = view.nodes.iter().position(|node| node.entity_id == id) {
            return Ok(position);
        }
    }
    let matches: Vec<usize> = view
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.label == wanted)
        .map(|(i, _)| i)
        .collect();
    match matches.as_slice() {
        [] => Err(ChainError::UnknownNode(wanted.to_string())),
        [only] => Ok(*only),
        _ => Err(ChainError::AmbiguousLabel {
            label: wanted.to_string(),
            matches: matches.iter().map(|&i| view.nodes[i].entity_id).collect(),
        }),
    }
}

/// The strongest chain from `from` to `to` (each a node ID or exact label) in `view`. `confidence_of` gives
/// each link's confidence by its ID; a link it doesn't know counts as 0, not believed. When equally strong
/// chains tie, the links are tried in ID order, so the same view always gives the same answer.
pub fn strongest_chain(
    view: &TopologyViewMaterialization,
    confidence_of: impl Fn(EntityId) -> Option<f64>,
    from: &str,
    to: &str,
) -> Result<StrongestChain, ChainError> {
    let as_chain_node = |i: usize| ChainNode {
        entity_id: view.nodes[i].entity_id,
        label: view.nodes[i].label.clone(),
        kind_label: view.nodes[i].kind_label.clone(),
    };
    let (start, goal) = (find_node(view, from)?, find_node(view, to)?);
    let mut index: HashMap<EntityId, usize> = HashMap::with_capacity(view.nodes.len());
    for (i, node) in view.nodes.iter().enumerate() {
        index.entry(node.entity_id).or_insert(i);
    }

    let mut order: Vec<usize> = (0..view.edges.len()).collect();
    order.sort_by_key(|&e| view.edges[e].entity_id);
    let mut believed: Vec<(EntityId, EntityId, f64)> = Vec::new();
    let mut believed_edge: Vec<usize> = Vec::new();
    let (mut unbelieved_links, mut dangling_links) = (Vec::new(), Vec::new());
    for e in order {
        let edge = &view.edges[e];
        if !(index.contains_key(&edge.source) && index.contains_key(&edge.target)) {
            dangling_links.push(edge.entity_id);
            continue;
        }
        let confidence = confidence_of(edge.entity_id).unwrap_or(0.0);
        if !(0.0..=1.0).contains(&confidence) {
            return Err(ChainError::BadConfidence {
                link: edge.entity_id,
                value: confidence,
            });
        }
        if confidence == 0.0 {
            unbelieved_links.push(edge.entity_id);
            continue;
        }
        believed.push((edge.source, edge.target, confidence));
        believed_edge.push(e);
    }

    let (from_id, to_id) = (view.nodes[start].entity_id, view.nodes[goal].entity_id);
    let chain = match skeletrace_algo::osint::strongest_chain(&believed, &from_id, &to_id) {
        Ok(chain) => chain,
        Err(skeletrace_algo::osint::ChainError::BadConfidence(at)) => {
            return Err(ChainError::BadConfidence {
                link: view.edges[believed_edge[at]].entity_id,
                value: believed[at].2,
            })
        }
    };
    let mut result = StrongestChain {
        from: as_chain_node(start),
        to: as_chain_node(goal),
        found: false,
        confidence: None,
        nodes: Vec::new(),
        links: Vec::new(),
        unbelieved_links,
        dangling_links,
    };
    if let Some(chain) = chain {
        result.found = true;
        result.confidence = Some(chain.confidence);
        result.nodes = chain
            .nodes
            .iter()
            .map(|id| as_chain_node(index[id]))
            .collect();
        result.links = chain
            .links
            .iter()
            .map(|&l| {
                let edge = &view.edges[believed_edge[l]];
                ChainLink {
                    entity_id: edge.entity_id,
                    source: edge.source,
                    target: edge.target,
                    kind_label: edge.kind_label.clone(),
                    confidence: believed[l].2,
                }
            })
            .collect();
    }
    Ok(result)
}
