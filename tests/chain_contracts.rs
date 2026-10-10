// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Contract tests for the strongest chain: the pure function, the operator API and the command line.

use std::collections::HashMap;
use std::process::Command;

use chrono::Utc;
use skeletrace::{
    find_node, strongest_chain, ChainError, CliCommand, Confidence, Edge, EdgeDirection, EdgeKind,
    EngineConfig, EntityId, EntityStatus, GeometryMode, Node, NodeKind, OperatorApi,
    OperatorRequest, OperatorResponse, Priority, SkeletraceEngine, SnapshotExporter, TimeRange,
    TopologyEdgeView, TopologyNodeView, TopologyViewMaterialization, ViewJob, ViewJobId, ViewKind,
};

/// A view with `labels.len()` nodes and one link per `(a, b)` pair, plus the links' IDs.
fn view(labels: &[&str], pairs: &[(usize, usize)]) -> (TopologyViewMaterialization, Vec<EntityId>) {
    let ids: Vec<EntityId> = labels.iter().map(|_| EntityId::new()).collect();
    let links: Vec<EntityId> = pairs.iter().map(|_| EntityId::new()).collect();
    let view = TopologyViewMaterialization {
        view_id: ViewJobId::new(),
        nodes: labels
            .iter()
            .zip(&ids)
            .map(|(label, id)| TopologyNodeView {
                entity_id: *id,
                label: (*label).into(),
                kind_label: "Relay".into(),
                position: None,
                metrics: Vec::new(),
            })
            .collect(),
        edges: pairs
            .iter()
            .zip(&links)
            .map(|(&(a, b), link)| TopologyEdgeView {
                entity_id: *link,
                source: ids[a],
                target: ids[b],
                kind_label: "Link".into(),
                geometry_mode: GeometryMode::Abstract,
                metrics: Vec::new(),
            })
            .collect(),
        boundaries: Vec::new(),
    };
    (view, links)
}

fn scores(links: &[EntityId], confidences: &[f64]) -> HashMap<EntityId, f64> {
    links
        .iter()
        .copied()
        .zip(confidences.iter().copied())
        .collect()
}

#[test]
fn two_solid_links_beat_one_shaky_one() {
    let (v, links) = view(&["a", "b", "c"], &[(0, 1), (1, 2), (0, 2)]);
    let known = scores(&links, &[0.9, 0.9, 0.5]);
    let chain = strongest_chain(&v, |id| known.get(&id).copied(), "a", "c").unwrap();
    assert!(chain.found);
    assert!((chain.confidence.unwrap() - 0.81).abs() < 1e-12);
    let labels: Vec<&str> = chain.nodes.iter().map(|n| n.label.as_str()).collect();
    assert_eq!(labels, ["a", "b", "c"]);
    let used: Vec<EntityId> = chain.links.iter().map(|l| l.entity_id).collect();
    assert_eq!(used, [links[0], links[1]]);
    assert_eq!(chain.links[0].confidence, 0.9);
    // The same, asked by ID instead of label.
    let by_id = strongest_chain(
        &v,
        |id| known.get(&id).copied(),
        &v.nodes[0].entity_id.0.to_string(),
        &v.nodes[2].entity_id.0.to_string(),
    )
    .unwrap();
    assert_eq!(by_id, chain);
}

#[test]
fn names_must_pick_out_exactly_one_node() {
    let (v, _) = view(&["a", "twin", "twin"], &[]);
    assert_eq!(find_node(&v, "a"), Ok(0));
    assert_eq!(
        find_node(&v, "nobody"),
        Err(ChainError::UnknownNode("nobody".into()))
    );
    match find_node(&v, "twin") {
        Err(ChainError::AmbiguousLabel { matches, .. }) => {
            assert_eq!(matches, [v.nodes[1].entity_id, v.nodes[2].entity_id])
        }
        other => panic!("expected an ambiguous label, got {other:?}"),
    }
    assert_eq!(
        find_node(&v, &EntityId::new().0.to_string())
            .map_err(|e| matches!(e, ChainError::UnknownNode(_))),
        Err(true),
        "an ID that isn't in the view"
    );
}

#[test]
fn unbelieved_dangling_and_bad_links() {
    // a - b is not believed at all; b - c is fine; the third link points outside the view.
    let (mut v, links) = view(&["a", "b", "c"], &[(0, 1), (1, 2), (0, 2)]);
    v.edges[2].target = EntityId::new();
    let known = scores(&links, &[0.0, 0.8, 1.0]);
    let chain = strongest_chain(&v, |id| known.get(&id).copied(), "a", "c").unwrap();
    assert!(!chain.found);
    assert_eq!(chain.confidence, None);
    assert!(chain.nodes.is_empty() && chain.links.is_empty());
    assert_eq!(chain.unbelieved_links, [links[0]]);
    assert_eq!(chain.dangling_links, [links[2]]);
    assert_eq!(chain.from.label, "a");
    assert_eq!(chain.to.label, "c");
    // A link the confidence lookup doesn't know counts as not believed.
    let chain = strongest_chain(&v, |_| None, "b", "c").unwrap();
    assert!(!chain.found);
    // Out-of-range confidences are refused, naming the link.
    for bad in [1.5, -0.1, f64::NAN] {
        let known = scores(&links, &[0.5, bad, 0.5]);
        match strongest_chain(&v, |id| known.get(&id).copied(), "a", "c") {
            Err(ChainError::BadConfidence { link, .. }) => assert_eq!(link, links[1]),
            other => panic!("expected a bad confidence, got {other:?}"),
        }
    }
    // From a node to itself: a chain of one, fully believable.
    let alone = strongest_chain(&v, |_| None, "b", "b").unwrap();
    assert!(alone.found);
    assert_eq!(alone.confidence, Some(1.0));
    assert_eq!(alone.nodes.len(), 1);
}

/// The best product over every simple route, by trying them all; links at 0 are never used.
fn best_by_trying(
    pairs: &[(usize, usize)],
    conf: &[f64],
    at: usize,
    goal: usize,
    seen: &mut Vec<usize>,
) -> Option<f64> {
    if at == goal {
        return Some(1.0);
    }
    seen.push(at);
    let mut best: Option<f64> = None;
    for (k, &(a, b)) in pairs.iter().enumerate() {
        if conf[k] == 0.0 {
            continue;
        }
        for (x, y) in [(a, b), (b, a)] {
            if x == at && !seen.contains(&y) {
                if let Some(rest) = best_by_trying(pairs, conf, y, goal, seen) {
                    let p = conf[k] * rest;
                    if best.is_none_or(|q| p > q) {
                        best = Some(p);
                    }
                }
            }
        }
    }
    seen.pop();
    best
}

#[test]
fn chains_match_trying_every_route() {
    let mut state: u64 = 2026;
    let mut below = |n: u64| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) % n
    };
    let levels = [0.0, 0.2, 0.5, 0.6, 0.75, 0.9, 1.0];
    let names = ["a", "b", "c", "d", "e", "f", "g"];
    for _ in 0..400 {
        let n = 2 + below(6) as usize;
        let pairs: Vec<(usize, usize)> = (0..below(12))
            .map(|_| (below(n as u64) as usize, below(n as u64) as usize))
            .collect();
        let conf: Vec<f64> = pairs
            .iter()
            .map(|_| levels[below(levels.len() as u64) as usize])
            .collect();
        let (v, links) = view(&names[..n], &pairs);
        let known = scores(&links, &conf);
        let (from, to) = (below(n as u64) as usize, below(n as u64) as usize);
        let want = best_by_trying(&pairs, &conf, from, to, &mut Vec::new());
        let chain =
            strongest_chain(&v, |id| known.get(&id).copied(), names[from], names[to]).unwrap();
        assert_eq!(
            chain.found,
            want.is_some(),
            "{pairs:?} {conf:?} {from}->{to}"
        );
        if let Some(best) = want {
            let got = chain.confidence.unwrap();
            assert!(
                (got - best).abs() < 1e-12,
                "{pairs:?} {conf:?} {from}->{to}"
            );
            // The listed links really join the listed nodes, and multiply to the stated confidence.
            let mut product = 1.0;
            for (k, link) in chain.links.iter().enumerate() {
                let ends = (chain.nodes[k].entity_id, chain.nodes[k + 1].entity_id);
                assert!(ends == (link.source, link.target) || ends == (link.target, link.source));
                product *= link.confidence;
            }
            assert!((product - got).abs() < 1e-15);
        }
    }
}

fn node(label: &str) -> Node {
    let now = Utc::now();
    Node {
        id: EntityId::new(),
        kind: NodeKind::Relay,
        label: label.into(),
        position: None,
        position_confidence: Confidence::new(1.0).unwrap(),
        status: EntityStatus::Active,
        priority: Priority::HIGH,
        tags: vec![],
        first_seen: now,
        last_seen: now,
    }
}

fn link(a: &Node, b: &Node, confidence: f64) -> Edge {
    let now = Utc::now();
    Edge {
        id: EntityId::new(),
        kind: EdgeKind::Link,
        direction: EdgeDirection::Undirected,
        source: a.id,
        target: b.id,
        geometry_mode: GeometryMode::Straight,
        waypoints: vec![],
        confidence: Confidence::new(confidence).unwrap(),
        status: EntityStatus::Active,
        priority: Priority::HIGH,
        tags: vec![],
        first_seen: now,
        last_seen: now,
    }
}

#[test]
fn the_operator_api_reads_link_confidences_from_the_engine() {
    let mut engine = SkeletraceEngine::new(EngineConfig::default()).unwrap();
    let (email, handle, phone) = (
        node("lantern@example.org"),
        node("@lantern_ops"),
        node("+44 7700 900123"),
    );
    for n in [&email, &handle, &phone] {
        engine.register_node(n.clone()).unwrap();
    }
    for e in [
        link(&email, &handle, 0.95),
        link(&handle, &phone, 0.7),
        link(&email, &phone, 0.4),
    ] {
        engine.register_edge(e).unwrap();
    }
    let dir = std::env::temp_dir().join(format!("skeletrace-chain-{}", uuid::Uuid::new_v4()));
    let exporter = SnapshotExporter::new(dir.join("exports"), None::<std::path::PathBuf>).unwrap();
    let mut api = OperatorApi::new(engine, exporter);
    let view = ViewJob {
        id: ViewJobId::new(),
        kind: ViewKind::Topology,
        entities: vec![email.id, handle.id, phone.id],
        metrics: vec![],
        time_range: TimeRange::LatestOnly,
        detail_override: None,
        viewport: None,
    };
    let request = OperatorRequest::StrongestChain {
        view,
        from: "lantern@example.org".into(),
        to: "+44 7700 900123".into(),
        now: Utc::now(),
    };
    match api.execute(request).unwrap() {
        OperatorResponse::StrongestChain(chain) => {
            assert!(chain.found);
            assert!((chain.confidence.unwrap() - 0.665).abs() < 1e-12);
            assert_eq!(chain.nodes[1].label, "@lantern_ops");
        }
        other => panic!("expected a chain, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_command_needs_exactly_four_arguments() {
    let parsed = CliCommand::parse_from([
        "skeletrace",
        "strongest-chain",
        "p.json",
        "v.json",
        "a",
        "c",
    ]);
    assert_eq!(
        parsed.unwrap(),
        CliCommand::StrongestChain {
            profile_path: "p.json".into(),
            view_path: "v.json".into(),
            from: "a".into(),
            to: "c".into(),
        }
    );
    for args in [
        vec!["strongest-chain"],
        vec!["strongest-chain", "p.json", "v.json", "a"],
        vec!["strongest-chain", "p.json", "v.json", "a", "c", "extra"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_skeletrace"))
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "args: {args:?}");
        assert!(output.stdout.is_empty());
    }
}
