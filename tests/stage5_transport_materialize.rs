// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use chrono::Utc;
use skeletrace::{
    AdapterKind, AuthConfig, Boundary, BoundaryKind, Confidence, Edge, EdgeDirection, EdgeKind,
    EngineStore, EntityId, EntityMapping, EntityStatus, GeoBBox, GeoCoord, HeaderPair,
    HttpJsonAdapter, HttpRequestProfile, InterpolationMethod, MetricBinding, MetricDefinition,
    MetricId, MetricValueType, Node, NodeKind, PollCadence, Priority, ProxyRoute, Quality,
    RetentionPolicy, Sample, SampleValue, SourceAdapter, SourceDefinition, SourceHealth, SourceId,
    SourceKind, SourceMappingConfig, SourceSchedule, SparseGeoMaterializer, Tag, TimeRange,
    TopologyMaterializer, TorHttpJsonAdapter, ValueSelector, ViewJob, ViewJobId, ViewKind,
};

fn numeric_metric(metric_id: MetricId, source_id: SourceId, name: &str) -> MetricDefinition {
    MetricDefinition {
        id: metric_id,
        name: name.into(),
        unit: "units".into(),
        value_type: MetricValueType::Numeric,
        cadence: PollCadence::Fixed(Duration::from_secs(30)),
        interpolation: InterpolationMethod::Linear,
        retention: RetentionPolicy {
            hot_duration: Duration::from_secs(60),
            warm_duration: Duration::from_secs(120),
            store_on_change_only: false,
            change_threshold: None,
            relative_change_threshold: None,
            max_silent_gap: Duration::from_secs(60),
        },
        source_ids: vec![source_id],
        show_in_popup: true,
        popup_priority: 1,
        description: name.into(),
    }
}

fn source(source_id: SourceId, adapter: AdapterKind, endpoint: String) -> SourceDefinition {
    SourceDefinition {
        id: source_id,
        name: format!("source-{source_id}"),
        kind: if matches!(adapter, AdapterKind::Manual) {
            SourceKind::Manual
        } else {
            SourceKind::Api
        },
        adapter,
        schedule: SourceSchedule::Manual,
        endpoint,
        auth_ref: Some("primary-auth".into()),
        health: SourceHealth::Pending,
        last_polled: None,
        last_error: None,
        backoff: Duration::from_secs(5),
        max_backoff: Duration::from_secs(30),
        tags: vec![],
    }
}

fn sample(
    entity_id: EntityId,
    metric_id: MetricId,
    source_id: SourceId,
    value: SampleValue,
) -> Sample {
    let now = Utc::now();
    Sample {
        entity_id,
        metric_id,
        ts_observed: now,
        ts_ingested: now,
        value,
        quality: Quality::new(1.0).unwrap(),
        source_id,
    }
}

fn spawn_request_recording_server(
    body: String,
    content_type: &'static str,
) -> (String, Arc<Mutex<String>>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let captured = Arc::new(Mutex::new(String::new()));
    let captured_clone = Arc::clone(&captured);
    let handle = thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut request_buf = [0u8; 8192];
            let read = stream.read(&mut request_buf).unwrap_or(0);
            *captured_clone.lock().unwrap() =
                String::from_utf8_lossy(&request_buf[..read]).into_owned();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    (format!("http://{addr}"), captured, handle)
}

#[test]
fn http_adapter_applies_headers_and_auth() {
    let source_id = SourceId::new();
    let entity_id = EntityId::new();
    let metric_id = MetricId::new();

    let (endpoint, captured, handle) =
        spawn_request_recording_server(r#"{"score":42.0}"#.to_string(), "application/json");

    let mapping = SourceMappingConfig {
        entity_mapping: EntityMapping::Static(entity_id),
        metric_bindings: vec![MetricBinding {
            metric_id,
            selector: ValueSelector::JsonPointer {
                pointer: "/score".into(),
            },
            value_type: Some(MetricValueType::Numeric),
            required: true,
        }],
        default_quality: Quality::new(0.9).unwrap(),
    };

    let profile = HttpRequestProfile {
        timeout: Duration::from_secs(2),
        headers: vec![HeaderPair {
            name: "X-Test-Header".into(),
            value: "stage5".into(),
        }],
        auth: Some(AuthConfig::BearerToken {
            token: "secret-token".into(),
        }),
        proxy_route: ProxyRoute::Direct,
        user_agent: Some("skeletrace-stage5-test".into()),
    };

    let mut adapter = HttpJsonAdapter::with_request_profile(mapping, profile).unwrap();
    let pull = adapter
        .pull(
            &source(source_id, AdapterKind::HttpPoller, endpoint),
            Utc::now(),
        )
        .unwrap();
    handle.join().unwrap();

    let request = captured.lock().unwrap().to_ascii_lowercase();
    assert!(request.contains("x-test-header: stage5"));
    assert!(request.contains("authorization: bearer secret-token"));
    assert!(request.contains("user-agent: skeletrace-stage5-test"));
    assert_eq!(pull.sample_count(), 1);
}

#[test]
fn tor_http_adapter_uses_tor_kind() {
    let mapping = SourceMappingConfig {
        entity_mapping: EntityMapping::Static(EntityId::new()),
        metric_bindings: vec![MetricBinding {
            metric_id: MetricId::new(),
            selector: ValueSelector::LiteralNumeric(1.0),
            value_type: Some(MetricValueType::Numeric),
            required: true,
        }],
        default_quality: Quality::new(1.0).unwrap(),
    };

    let adapter = TorHttpJsonAdapter::new(mapping, Duration::from_secs(3)).unwrap();
    assert_eq!(adapter.kind(), AdapterKind::TorHttpPoller);
    assert_eq!(
        HttpRequestProfile::tor_default(Duration::from_secs(1))
            .proxy_route
            .proxy_url(),
        Some("socks5://127.0.0.1:9050")
    );
}

#[test]
fn topology_and_sparse_geo_materializers_emit_expected_shapes() {
    let now = Utc::now();
    let source_id = SourceId::new();
    let metric_id = MetricId::new();
    let node_a = EntityId::new();
    let node_b = EntityId::new();
    let edge_id = EntityId::new();
    let boundary_id = EntityId::new();

    let mut store = EngineStore::new();
    store
        .register_source(source(source_id, AdapterKind::Manual, String::new()))
        .unwrap();
    store
        .register_metric(numeric_metric(metric_id, source_id, "volume"))
        .unwrap();

    store
        .upsert_node(Node {
            id: node_a,
            kind: NodeKind::Identity,
            label: "alpha".into(),
            position: Some(GeoCoord::new(51.5, -0.1, None).unwrap()),
            position_confidence: Confidence::new(0.9).unwrap(),
            status: EntityStatus::Active,
            priority: Priority::HIGH,
            tags: vec![Tag::new("role", "source").unwrap()],
            first_seen: now,
            last_seen: now,
        })
        .unwrap();
    store
        .upsert_node(Node {
            id: node_b,
            kind: NodeKind::Identity,
            label: "beta".into(),
            position: Some(GeoCoord::new(48.85, 2.35, None).unwrap()),
            position_confidence: Confidence::new(0.9).unwrap(),
            status: EntityStatus::Active,
            priority: Priority::HIGH,
            tags: vec![Tag::new("role", "target").unwrap()],
            first_seen: now,
            last_seen: now,
        })
        .unwrap();
    store
        .upsert_edge(Edge {
            id: edge_id,
            kind: EdgeKind::Route,
            direction: EdgeDirection::Directed,
            source: node_a,
            target: node_b,
            geometry_mode: skeletrace::GeometryMode::Geodesic,
            waypoints: vec![],
            confidence: Confidence::new(0.8).unwrap(),
            status: EntityStatus::Active,
            priority: Priority::NORMAL,
            tags: vec![],
            first_seen: now,
            last_seen: now,
        })
        .unwrap();
    store
        .upsert_boundary(Boundary {
            id: boundary_id,
            kind: BoundaryKind::NetworkZone,
            label: "seam".into(),
            extent: Some(GeoBBox::new(45.0, -5.0, 55.0, 5.0).unwrap()),
            related_entities: vec![node_a, node_b],
            confidence: Confidence::new(0.7).unwrap(),
            status: EntityStatus::Active,
            priority: Priority::NORMAL,
            tags: vec![],
            first_seen: now,
            last_seen: now,
        })
        .unwrap();

    store
        .ingest_sample(sample(
            node_a,
            metric_id,
            source_id,
            SampleValue::Numeric(11.0),
        ))
        .unwrap();
    store
        .ingest_sample(sample(
            edge_id,
            metric_id,
            source_id,
            SampleValue::Numeric(22.0),
        ))
        .unwrap();
    store
        .ingest_sample(sample(
            boundary_id,
            metric_id,
            source_id,
            SampleValue::Numeric(33.0),
        ))
        .unwrap();

    let topology_view = ViewJob {
        id: ViewJobId::new(),
        kind: ViewKind::Topology,
        entities: vec![node_a, node_b, edge_id, boundary_id],
        metrics: vec![metric_id],
        time_range: TimeRange::LatestOnly,
        detail_override: None,
        viewport: None,
    };
    let topology = TopologyMaterializer::build(&store, &topology_view, now).unwrap();
    assert_eq!(topology.nodes.len(), 2);
    assert_eq!(topology.edges.len(), 1);
    assert_eq!(topology.boundaries.len(), 1);
    assert_eq!(
        topology
            .nodes
            .iter()
            .map(|node| node.metrics.len())
            .sum::<usize>(),
        1
    );

    let sparse_view = ViewJob {
        id: ViewJobId::new(),
        kind: ViewKind::SparseGeo,
        entities: vec![node_a, node_b, edge_id, boundary_id],
        metrics: vec![metric_id],
        time_range: TimeRange::LatestOnly,
        detail_override: None,
        viewport: Some(GeoBBox::new(40.0, -10.0, 60.0, 10.0).unwrap()),
    };
    let sparse = SparseGeoMaterializer::build(&store, &sparse_view, now).unwrap();
    let feature_collection = sparse.to_feature_collection();
    assert_eq!(feature_collection.feature_count, 4);
    assert_eq!(feature_collection.geojson["type"], "FeatureCollection");
    assert_eq!(
        feature_collection.geojson["features"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
}
