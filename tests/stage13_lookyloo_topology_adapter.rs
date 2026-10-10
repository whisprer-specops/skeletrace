// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

use std::time::Duration;

use chrono::{TimeZone, Utc};

use skeletrace::{
    AdapterKind, AdapterProfile, Confidence, DetailTier, EngineConfig, HttpRequestProfile,
    InterpolationMethod, LookylooMetricBindings, LookylooTopologyAdapter, LookylooTopologyConfig,
    ManualPushAdapter, MetricDefinition, MetricId, MetricValueType, PollCadence, Quality,
    RetentionPolicy, SkeletraceEngine, SourceDefinition, SourceHealth, SourceKind, SourceSchedule,
    TimeRange, ViewJob, ViewJobId, ViewKind,
};

fn metric(id: MetricId, name: &str, value_type: MetricValueType) -> MetricDefinition {
    MetricDefinition {
        id,
        name: name.into(),
        unit: String::new(),
        value_type,
        cadence: PollCadence::Fixed(Duration::from_secs(60)),
        interpolation: InterpolationMethod::StepForward,
        retention: RetentionPolicy {
            hot_duration: Duration::from_secs(300),
            warm_duration: Duration::from_secs(1_800),
            store_on_change_only: false,
            change_threshold: None,
            relative_change_threshold: None,
            max_silent_gap: Duration::from_secs(300),
        },
        source_ids: Vec::new(),
        show_in_popup: true,
        popup_priority: 1,
        description: format!("metric {name}"),
    }
}

fn test_source_definition() -> SourceDefinition {
    SourceDefinition {
        id: skeletrace::SourceId::new(),
        name: "lookyloo-topology".into(),
        kind: SourceKind::Api,
        adapter: AdapterKind::Manual,
        schedule: SourceSchedule::Manual,
        endpoint: "https://example.invalid/lookyloo/captures".into(),
        auth_ref: None,
        health: SourceHealth::Pending,
        last_polled: None,
        last_error: None,
        backoff: Duration::from_secs(5),
        max_backoff: Duration::from_secs(30),
        tags: Vec::new(),
    }
}

#[test]
fn lookyloo_topology_profile_builds_http_poller_adapter() {
    let config = LookylooTopologyConfig {
        payload_root_pointer: Some("/captures".into()),
        include_raw_payload: true,
        default_quality: Quality::new(0.95).unwrap(),
        metrics: LookylooMetricBindings {
            title: Some(MetricId::new()),
            root_url: None,
            final_url: None,
            redirect_count: Some(MetricId::new()),
            has_error: None,
            error_text: None,
            no_index: None,
            category_count: None,
            categories_joined: None,
            has_parent: None,
            parent_capture: None,
            user_agent: None,
            referer: None,
            capture_dir: None,
        },
        include_capture_nodes: true,
        include_domain_nodes: true,
        include_capture_root_edge: true,
        include_capture_final_edge: true,
        include_redirect_chain_edges: true,
        include_parent_capture_edges: true,
        include_category_boundaries: true,
        include_error_boundary: true,
        include_no_index_boundary: true,
        relationship_confidence: Confidence::new(0.9).unwrap(),
    };

    let profile = AdapterProfile::LookylooTopology {
        config,
        request_profile: HttpRequestProfile::direct(Duration::from_secs(5)),
    };

    assert_eq!(profile.expected_kind(), AdapterKind::HttpPoller);
    profile.validate().unwrap();
    let mut adapter = profile.build_adapter().unwrap();
    assert_eq!(adapter.kind(), AdapterKind::HttpPoller);
    assert!(adapter
        .as_any_mut()
        .downcast_mut::<LookylooTopologyAdapter>()
        .is_some());
}

#[test]
fn engine_ingests_lookyloo_topology_and_materializes_view() {
    let title_metric = MetricId::new();
    let redirect_metric = MetricId::new();

    let source = test_source_definition();
    let source_id = source.id;
    let topology_config = LookylooTopologyConfig {
        payload_root_pointer: Some("/captures".into()),
        include_raw_payload: true,
        default_quality: Quality::new(0.9).unwrap(),
        metrics: LookylooMetricBindings {
            title: Some(title_metric),
            root_url: None,
            final_url: None,
            redirect_count: Some(redirect_metric),
            has_error: None,
            error_text: None,
            no_index: None,
            category_count: None,
            categories_joined: None,
            has_parent: None,
            parent_capture: None,
            user_agent: None,
            referer: None,
            capture_dir: None,
        },
        include_capture_nodes: true,
        include_domain_nodes: true,
        include_capture_root_edge: true,
        include_capture_final_edge: true,
        include_redirect_chain_edges: true,
        include_parent_capture_edges: true,
        include_category_boundaries: true,
        include_error_boundary: true,
        include_no_index_boundary: true,
        relationship_confidence: Confidence::new(0.8).unwrap(),
    };

    let lookyloo = LookylooTopologyAdapter::with_request_profile(
        topology_config,
        HttpRequestProfile::direct(Duration::from_secs(5)),
    )
    .unwrap();

    let now = Utc.with_ymd_and_hms(2025, 3, 23, 14, 0, 0).unwrap();
    let batch = lookyloo
        .pull_from_text(
            &SourceDefinition {
                adapter: AdapterKind::HttpPoller,
                ..source.clone()
            },
            now,
            &serde_json::json!({
                "captures": [
                    {
                        "uuid": "capture-a",
                        "title": "Capture A",
                        "timestamp": "2025-03-23T13:55:00+0000",
                        "url": "https://root.example.test/start",
                        "redirects": [
                            "https://mid.example.test/step1",
                            "https://final.example.test/end"
                        ],
                        "categories": ["kit"],
                        "parent": "capture-parent",
                        "no_index": true,
                        "error": "blocked"
                    }
                ]
            })
            .to_string(),
        )
        .unwrap();

    let mut engine = SkeletraceEngine::new(EngineConfig::default()).unwrap();
    engine
        .register_metric(metric(title_metric, "title", MetricValueType::Code))
        .unwrap();
    engine
        .register_metric(metric(
            redirect_metric,
            "redirect_count",
            MetricValueType::Numeric,
        ))
        .unwrap();
    engine
        .register_source(source.clone(), Box::new(ManualPushAdapter::new()), now)
        .unwrap();
    engine.enqueue_manual_batch(source_id, batch).unwrap();
    let report = engine.poll_source_now(source_id, now).unwrap();
    assert_eq!(report.polled_sources, 1);
    assert!(engine.stats().node_count >= 5);
    assert!(engine.stats().edge_count >= 4);
    assert!(engine.stats().boundary_count >= 3);

    let mut entity_ids = Vec::new();
    entity_ids.extend(engine.store().nodes_cloned().into_iter().map(|n| n.id));
    entity_ids.extend(engine.store().edges_cloned().into_iter().map(|e| e.id));
    entity_ids.extend(engine.store().boundaries_cloned().into_iter().map(|b| b.id));

    let view = ViewJob {
        id: ViewJobId::new(),
        kind: ViewKind::Topology,
        entities: entity_ids,
        metrics: vec![title_metric, redirect_metric],
        viewport: None,
        time_range: TimeRange::LatestOnly,
        detail_override: Some(DetailTier::Sampled),
    };

    let topology = engine.materialize_topology(&view, now).unwrap();
    assert!(topology.nodes.iter().any(|n| n.label == "Capture A"));
    assert!(topology
        .nodes
        .iter()
        .any(|n| n.label == "root.example.test"));
    assert!(topology.nodes.iter().any(|n| n.label == "mid.example.test"));
    assert!(topology
        .nodes
        .iter()
        .any(|n| n.label == "final.example.test"));
    assert!(topology.edges.iter().any(|e| e.kind_label == "Association"));
    assert!(topology.edges.iter().any(|e| e.kind_label == "Route"));
    assert!(topology
        .boundaries
        .iter()
        .any(|b| b.label == "Category: kit"));
    assert!(topology
        .boundaries
        .iter()
        .any(|b| b.label == "Lookyloo No-Index"));
    assert!(topology
        .boundaries
        .iter()
        .any(|b| b.label == "Lookyloo Error"));
}
