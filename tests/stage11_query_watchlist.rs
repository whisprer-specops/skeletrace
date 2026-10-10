// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

use std::time::Duration;

use chrono::Utc;
use skeletrace::{
    AdapterKind, AdapterProfile, Confidence, EngineConfig, EngineProfile, EntityId, EntityStatus,
    FlowId, GeoCoord, InterpolationMethod, MetricDefinition, MetricId, MetricValueType,
    NumericPredicate, OperatorApi, OperatorRequest, OperatorResponse, PollCadence, Priority,
    Quality, QueryFilter, RetentionPolicy, Sample, SampleValue, SnapshotExporter, SourceDefinition,
    SourceHealth, SourceId, SourceKind, SourceProfile, SourcePull, SourceSchedule, Tag, WatchItem,
    Watchlist,
};

fn manual_source(source_id: SourceId) -> SourceDefinition {
    SourceDefinition {
        id: source_id,
        name: format!("manual-{source_id}"),
        kind: SourceKind::Manual,
        adapter: AdapterKind::Manual,
        schedule: SourceSchedule::Manual,
        endpoint: String::new(),
        auth_ref: None,
        health: SourceHealth::Pending,
        last_polled: None,
        last_error: None,
        backoff: Duration::from_secs(5),
        max_backoff: Duration::from_secs(30),
        tags: vec![Tag::new("env", "test").unwrap()],
    }
}

fn numeric_metric(metric_id: MetricId, source_id: SourceId, name: &str) -> MetricDefinition {
    MetricDefinition {
        id: metric_id,
        name: name.into(),
        unit: "units".into(),
        value_type: MetricValueType::Numeric,
        cadence: PollCadence::Manual,
        interpolation: InterpolationMethod::StepForward,
        retention: RetentionPolicy {
            hot_duration: Duration::from_secs(300),
            warm_duration: Duration::from_secs(1800),
            store_on_change_only: false,
            change_threshold: None,
            relative_change_threshold: None,
            max_silent_gap: Duration::from_secs(300),
        },
        source_ids: vec![source_id],
        show_in_popup: true,
        popup_priority: 1,
        description: name.into(),
    }
}

fn node(entity_id: EntityId, label: &str, now: chrono::DateTime<chrono::Utc>) -> skeletrace::Node {
    skeletrace::Node {
        id: entity_id,
        kind: skeletrace::NodeKind::Identity,
        label: label.into(),
        position: Some(GeoCoord::new(51.5, -0.1, None).unwrap()),
        position_confidence: Confidence::new(0.95).unwrap(),
        status: EntityStatus::Active,
        priority: Priority::HIGH,
        tags: vec![Tag::new("role", "source").unwrap()],
        first_seen: now,
        last_seen: now,
    }
}

fn sample(
    entity_id: EntityId,
    metric_id: MetricId,
    source_id: SourceId,
    now: chrono::DateTime<chrono::Utc>,
    value: f64,
) -> Sample {
    Sample {
        entity_id,
        metric_id,
        ts_observed: now,
        ts_ingested: now,
        value: SampleValue::Numeric(value),
        quality: Quality::new(1.0).unwrap(),
        source_id,
    }
}

#[test]
fn query_latest_filters_by_label_metric_and_threshold() {
    let now = Utc::now();
    let source_id = SourceId::new();
    let metric_id = MetricId::new();
    let alpha_id = EntityId::new();
    let beta_id = EntityId::new();

    let temp_dir = std::env::temp_dir().join(format!("skeletrace-stage11-query-{}", FlowId::new()));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let profile = EngineProfile {
        name: "query-profile".into(),
        config: EngineConfig::default(),
        metrics: vec![numeric_metric(metric_id, source_id, "volume")],
        nodes: vec![node(alpha_id, "alpha", now), node(beta_id, "beta", now)],
        edges: vec![],
        boundaries: vec![],
        sources: vec![SourceProfile {
            definition: manual_source(source_id),
            adapter_profile: AdapterProfile::ManualPush,
        }],
    };

    let exporter =
        SnapshotExporter::new(temp_dir.join("exports"), None::<std::path::PathBuf>).unwrap();
    let mut api = OperatorApi::from_profile(&profile, exporter, now).unwrap();
    api.engine_mut()
        .enqueue_manual_batch(
            source_id,
            SourcePull {
                raw_records: vec![],
                samples: vec![
                    sample(alpha_id, metric_id, source_id, now, 12.0),
                    sample(beta_id, metric_id, source_id, now, 3.0),
                ],
                touched_entities: vec![alpha_id, beta_id],
                nodes: vec![],
                edges: vec![],
                boundaries: vec![],
            },
        )
        .unwrap();
    api.engine_mut().poll_source_now(source_id, now).unwrap();

    let response = api
        .execute(OperatorRequest::QueryLatest {
            filter: QueryFilter {
                entities: skeletrace::EntitySelector {
                    entity_ids: vec![],
                    label_contains: Some("alp".into()),
                    class: Some(skeletrace::EntityClass::Node),
                },
                metric_ids: vec![metric_id],
                only_hot: true,
                numeric_predicate: Some(NumericPredicate::AtLeast(10.0)),
                limit: Some(10),
                source_ids: vec![],
                entity_statuses: vec![],
                required_tags: vec![],
            },
            now,
        })
        .unwrap();

    let query = match response {
        OperatorResponse::Query(query) => query,
        other => panic!("unexpected response: {other:?}"),
    };

    assert_eq!(query.rows.len(), 1);
    assert_eq!(query.rows[0].entity_id, alpha_id);
    assert_eq!(query.rows[0].metric_id, metric_id);
    assert_eq!(query.rows[0].display_value, "12");
}

#[test]
fn watchlist_evaluation_emits_alerts_for_matching_latest_values() {
    let now = Utc::now();
    let source_id = SourceId::new();
    let metric_id = MetricId::new();
    let entity_id = EntityId::new();

    let temp_dir = std::env::temp_dir().join(format!("skeletrace-stage11-watch-{}", FlowId::new()));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let profile = EngineProfile {
        name: "watch-profile".into(),
        config: EngineConfig::default(),
        metrics: vec![numeric_metric(metric_id, source_id, "volume")],
        nodes: vec![node(entity_id, "alpha", now)],
        edges: vec![],
        boundaries: vec![],
        sources: vec![SourceProfile {
            definition: manual_source(source_id),
            adapter_profile: AdapterProfile::ManualPush,
        }],
    };

    let exporter =
        SnapshotExporter::new(temp_dir.join("exports"), None::<std::path::PathBuf>).unwrap();
    let mut api = OperatorApi::from_profile(&profile, exporter, now).unwrap();
    api.engine_mut()
        .enqueue_manual_batch(
            source_id,
            SourcePull {
                raw_records: vec![],
                samples: vec![sample(entity_id, metric_id, source_id, now, 42.0)],
                touched_entities: vec![entity_id],
                nodes: vec![],
                edges: vec![],
                boundaries: vec![],
            },
        )
        .unwrap();
    api.engine_mut().poll_source_now(source_id, now).unwrap();

    let response = api
        .execute(OperatorRequest::EvaluateWatchlist {
            watchlist: Watchlist {
                label: "priority-watch".into(),
                items: vec![WatchItem {
                    label: "alpha-volume-high".into(),
                    entity_id,
                    metric_id,
                    rule: skeletrace::AlertRule::NumericAtLeast(40.0),
                }],
            },
            now,
        })
        .unwrap();

    let evaluation = match response {
        OperatorResponse::Watchlist(evaluation) => evaluation,
        other => panic!("unexpected response: {other:?}"),
    };

    assert_eq!(evaluation.checked_items, 1);
    assert_eq!(evaluation.missing_items, 0);
    assert_eq!(evaluation.alerts.len(), 1);
    assert_eq!(evaluation.alerts[0].entity_id, entity_id);
    assert_eq!(evaluation.alerts[0].metric_id, metric_id);
    assert_eq!(evaluation.alerts[0].display_value, "42");
}
