// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

use std::collections::HashMap;
use std::env;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

use chrono::{TimeDelta, Utc};
use skeletrace::{
    AdapterKind, EngineStore, EntityId, FeedField, FeedPollAdapter, HttpJsonAdapter,
    InterpolationMethod, MetricBinding, MetricDefinition, MetricId, MetricValueType, PollCadence,
    Quality, RetentionPolicy, Sample, SampleValue, SourceAdapter, SourceDefinition, SourceHealth,
    SourceId, SourceKind, SourceMappingConfig, SourceSchedule, TimeRange, ValueSelector,
};
use uuid::Uuid;

fn numeric_metric(metric_id: MetricId, source_id: SourceId, name: &str) -> MetricDefinition {
    MetricDefinition {
        id: metric_id,
        name: name.into(),
        unit: String::new(),
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

fn code_metric(metric_id: MetricId, source_id: SourceId, name: &str) -> MetricDefinition {
    MetricDefinition {
        id: metric_id,
        name: name.into(),
        unit: String::new(),
        value_type: MetricValueType::Code,
        cadence: PollCadence::Fixed(Duration::from_secs(30)),
        interpolation: InterpolationMethod::StepForward,
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
        popup_priority: 2,
        description: name.into(),
    }
}

fn source(source_id: SourceId, adapter: AdapterKind, endpoint: String) -> SourceDefinition {
    SourceDefinition {
        id: source_id,
        name: format!("source-{source_id}"),
        kind: match adapter {
            AdapterKind::FeedPoller => SourceKind::Api,
            AdapterKind::HttpPoller => SourceKind::Api,
            _ => SourceKind::Manual,
        },
        adapter,
        schedule: SourceSchedule::Manual,
        endpoint,
        auth_ref: None,
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
    ts_observed: chrono::DateTime<Utc>,
    value: SampleValue,
) -> Sample {
    Sample {
        entity_id,
        metric_id,
        ts_observed,
        ts_ingested: ts_observed,
        value,
        quality: Quality::new(1.0).unwrap(),
        source_id,
    }
}

fn spawn_http_server(body: String, content_type: &'static str) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut request_buf = [0u8; 4096];
            let _ = stream.read(&mut request_buf);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    (format!("http://{addr}"), handle)
}

#[test]
fn sqlite_warm_store_retains_history_beyond_hot_buffer() {
    let db_path = env::temp_dir().join(format!("skeletrace-stage4-{}.sqlite3", Uuid::new_v4()));
    let now = Utc::now();
    let source_id = SourceId::new();
    let metric_id = MetricId::new();
    let entity_id = EntityId::new();

    let mut store = EngineStore::with_sqlite_warm_store(&db_path).unwrap();
    store
        .register_source(source(source_id, AdapterKind::Manual, String::new()))
        .unwrap();
    store
        .register_metric(numeric_metric(metric_id, source_id, "throughput"))
        .unwrap();

    store
        .ingest_sample(sample(
            entity_id,
            metric_id,
            source_id,
            now,
            SampleValue::Numeric(10.0),
        ))
        .unwrap();
    store
        .ingest_sample(sample(
            entity_id,
            metric_id,
            source_id,
            now + TimeDelta::seconds(10),
            SampleValue::Numeric(20.0),
        ))
        .unwrap();

    assert_eq!(store.stats().warm_sample_count, 2);

    let query_now = now + TimeDelta::seconds(61);
    store.prune_all(query_now).unwrap();
    assert_eq!(store.stats().retained_sample_count, 1);

    let history = store
        .samples_for_result(
            entity_id,
            metric_id,
            TimeRange::Window {
                start: now,
                end: query_now,
            },
            query_now,
        )
        .unwrap();
    assert_eq!(history.len(), 2);
}

#[test]
fn http_json_adapter_maps_numeric_and_code_fields() {
    let source_id = SourceId::new();
    let entity_id = EntityId::new();
    let metric_score = MetricId::new();
    let metric_label = MetricId::new();
    let now = Utc::now();

    let body = r#"{"score":42.5,"label":"storm"}"#.to_string();
    let (endpoint, handle) = spawn_http_server(body, "application/json");

    let mapping = SourceMappingConfig {
        entity_mapping: skeletrace::EntityMapping::Static(entity_id),
        metric_bindings: vec![
            MetricBinding {
                metric_id: metric_score,
                selector: ValueSelector::JsonPointer {
                    pointer: "/score".into(),
                },
                value_type: Some(MetricValueType::Numeric),
                required: true,
            },
            MetricBinding {
                metric_id: metric_label,
                selector: ValueSelector::JsonPointer {
                    pointer: "/label".into(),
                },
                value_type: Some(MetricValueType::Code),
                required: true,
            },
        ],
        default_quality: Quality::new(0.95).unwrap(),
    };
    let mut adapter = HttpJsonAdapter::new(mapping, Duration::from_secs(2)).unwrap();

    let source = source(source_id, AdapterKind::HttpPoller, endpoint);
    let pull = adapter.pull(&source, now).unwrap();
    let _ = handle.join();

    assert_eq!(pull.raw_count(), 1);
    assert_eq!(pull.sample_count(), 2);
    assert_eq!(pull.touched_entities, vec![entity_id]);

    let mut metrics = HashMap::new();
    metrics.insert(
        metric_score,
        numeric_metric(metric_score, source_id, "score"),
    );
    metrics.insert(metric_label, code_metric(metric_label, source_id, "label"));
    pull.validate_against(&metrics).unwrap();
}

#[test]
fn feed_adapter_maps_rss_items_to_deterministic_entities() {
    let source_id = SourceId::new();
    let metric_title = MetricId::new();
    let metric_link = MetricId::new();
    let now = Utc::now();

    let rss = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Example Feed</title>
    <link>https://example.invalid/</link>
    <description>Feed</description>
    <item>
      <title>Item One</title>
      <link>https://example.invalid/posts/1</link>
      <guid>item-1</guid>
      <description>Hello</description>
    </item>
  </channel>
</rss>
"#
    .to_string();

    let (endpoint_a, handle_a) = spawn_http_server(rss.clone(), "application/rss+xml");
    let (endpoint_b, handle_b) = spawn_http_server(rss, "application/rss+xml");

    let mapping = SourceMappingConfig {
        entity_mapping: skeletrace::EntityMapping::FeedGuidOrLink,
        metric_bindings: vec![
            MetricBinding {
                metric_id: metric_title,
                selector: ValueSelector::FeedField(FeedField::Title),
                value_type: Some(MetricValueType::Code),
                required: true,
            },
            MetricBinding {
                metric_id: metric_link,
                selector: ValueSelector::FeedField(FeedField::Link),
                value_type: Some(MetricValueType::Code),
                required: true,
            },
        ],
        default_quality: Quality::new(0.9).unwrap(),
    };

    let mut adapter_a = FeedPollAdapter::new(mapping.clone(), Duration::from_secs(2)).unwrap();
    let mut adapter_b = FeedPollAdapter::new(mapping, Duration::from_secs(2)).unwrap();

    let pull_a = adapter_a
        .pull(&source(source_id, AdapterKind::FeedPoller, endpoint_a), now)
        .unwrap();
    let pull_b = adapter_b
        .pull(&source(source_id, AdapterKind::FeedPoller, endpoint_b), now)
        .unwrap();
    let _ = handle_a.join();
    let _ = handle_b.join();

    assert_eq!(pull_a.raw_count(), 1);
    assert_eq!(pull_a.sample_count(), 2);
    assert_eq!(pull_b.sample_count(), 2);
    assert_eq!(pull_a.touched_entities.len(), 1);
    assert_eq!(pull_b.touched_entities.len(), 1);
    assert_eq!(pull_a.touched_entities[0], pull_b.touched_entities[0]);
}
