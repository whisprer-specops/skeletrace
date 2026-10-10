// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

use std::time::Duration;

use chrono::{TimeDelta, Utc};
use skeletrace::view::TimeRange;
use skeletrace::{
    AdapterKind, AdapterProfile, Confidence, EngineConfig, EngineProfile, EntityId, EntityStatus,
    FlowId, GeoCoord, InterpolationMethod, MetricDefinition, MetricId, MetricValueType,
    OperatorApi, OperatorRequest, OperatorResponse, PollCadence, Priority, Quality, QueryFilter,
    QueryGroupKey, QueryRequest, QuerySortKey, RetentionPolicy, Sample, SampleValue, SavedQuery,
    SnapshotExporter, SourceDefinition, SourceHealth, SourceId, SourceKind, SourceProfile,
    SourcePull, SourceSchedule, Tag,
};

fn manual_source(source_id: SourceId, name: &str) -> SourceDefinition {
    SourceDefinition {
        id: source_id,
        name: name.into(),
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

fn numeric_metric(metric_id: MetricId, source_ids: Vec<SourceId>, name: &str) -> MetricDefinition {
    MetricDefinition {
        id: metric_id,
        name: name.into(),
        unit: "units".into(),
        value_type: MetricValueType::Numeric,
        cadence: PollCadence::Manual,
        interpolation: InterpolationMethod::StepForward,
        retention: RetentionPolicy {
            hot_duration: Duration::from_secs(600),
            warm_duration: Duration::from_secs(3600),
            store_on_change_only: false,
            change_threshold: None,
            relative_change_threshold: None,
            max_silent_gap: Duration::from_secs(300),
        },
        source_ids,
        show_in_popup: true,
        popup_priority: 1,
        description: name.into(),
    }
}

fn node(
    entity_id: EntityId,
    label: &str,
    status: EntityStatus,
    role: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> skeletrace::Node {
    skeletrace::Node {
        id: entity_id,
        kind: skeletrace::NodeKind::Identity,
        label: label.into(),
        position: Some(GeoCoord::new(51.5, -0.1, None).unwrap()),
        position_confidence: Confidence::new(0.95).unwrap(),
        status,
        priority: Priority::HIGH,
        tags: vec![Tag::new("role", role).unwrap()],
        first_seen: now,
        last_seen: now,
    }
}

fn sample(
    entity_id: EntityId,
    metric_id: MetricId,
    source_id: SourceId,
    observed_at: chrono::DateTime<chrono::Utc>,
    ingested_at: chrono::DateTime<chrono::Utc>,
    value: f64,
) -> Sample {
    Sample {
        entity_id,
        metric_id,
        ts_observed: observed_at,
        ts_ingested: ingested_at,
        value: SampleValue::Numeric(value),
        quality: Quality::new(1.0).unwrap(),
        source_id,
    }
}

#[test]
fn advanced_query_supports_saved_shape_filters_sort_groups_and_compare() {
    let now = Utc::now();
    let earlier = now - TimeDelta::minutes(5);
    let source_a = SourceId::new();
    let source_b = SourceId::new();
    let metric_id = MetricId::new();
    let alpha_id = EntityId::new();
    let beta_id = EntityId::new();

    let temp_dir = std::env::temp_dir().join(format!("skeletrace-stage14-query-{}", FlowId::new()));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let profile = EngineProfile {
        name: "advanced-query-profile".into(),
        config: EngineConfig::default(),
        metrics: vec![numeric_metric(
            metric_id,
            vec![source_a, source_b],
            "volume",
        )],
        nodes: vec![
            node(alpha_id, "alpha", EntityStatus::Active, "source", now),
            node(beta_id, "beta", EntityStatus::Alerting, "other", now),
        ],
        edges: vec![],
        boundaries: vec![],
        sources: vec![
            SourceProfile {
                definition: manual_source(source_a, "manual-a"),
                adapter_profile: AdapterProfile::ManualPush,
            },
            SourceProfile {
                definition: manual_source(source_b, "manual-b"),
                adapter_profile: AdapterProfile::ManualPush,
            },
        ],
    };

    let exporter =
        SnapshotExporter::new(temp_dir.join("exports"), None::<std::path::PathBuf>).unwrap();
    let mut api = OperatorApi::from_profile(&profile, exporter, now).unwrap();

    api.engine_mut()
        .enqueue_manual_batch(
            source_a,
            SourcePull {
                raw_records: vec![],
                samples: vec![
                    sample(alpha_id, metric_id, source_a, earlier, earlier, 10.0),
                    sample(alpha_id, metric_id, source_a, now, now, 15.0),
                ],
                touched_entities: vec![alpha_id],
                nodes: vec![],
                edges: vec![],
                boundaries: vec![],
            },
        )
        .unwrap();
    api.engine_mut().poll_source_now(source_a, now).unwrap();

    api.engine_mut()
        .enqueue_manual_batch(
            source_b,
            SourcePull {
                raw_records: vec![],
                samples: vec![sample(beta_id, metric_id, source_b, now, now, 20.0)],
                touched_entities: vec![beta_id],
                nodes: vec![],
                edges: vec![],
                boundaries: vec![],
            },
        )
        .unwrap();
    api.engine_mut().poll_source_now(source_b, now).unwrap();

    let saved = SavedQuery {
        label: "ops-volume".into(),
        request: QueryRequest {
            label: Some("ops-volume".into()),
            filter: QueryFilter {
                entities: skeletrace::EntitySelector {
                    entity_ids: vec![],
                    label_contains: None,
                    class: Some(skeletrace::EntityClass::Node),
                },
                metric_ids: vec![metric_id],
                only_hot: true,
                numeric_predicate: Some(skeletrace::NumericPredicate::AtLeast(12.0)),
                limit: None,
                source_ids: vec![source_a],
                entity_statuses: vec![EntityStatus::Active],
                required_tags: vec![Tag::new("role", "source").unwrap()],
            },
            sort: Some(QuerySortKey::ValueDescending),
            group_by: vec![QueryGroupKey::EntityClass, QueryGroupKey::SourceId],
            compare_range: Some(TimeRange::Window {
                start: earlier - TimeDelta::seconds(1),
                end: earlier + TimeDelta::seconds(1),
            }),
        },
    };
    saved.validate().unwrap();

    let response = api
        .execute(OperatorRequest::QueryAdvanced {
            request: saved.request,
            now,
        })
        .unwrap();

    let query = match response {
        OperatorResponse::QueryEnvelope(query) => query,
        other => panic!("unexpected response: {other:?}"),
    };

    assert_eq!(query.request_label.as_deref(), Some("ops-volume"));
    assert_eq!(query.total_rows, 1);
    assert_eq!(query.rows.len(), 1);
    assert_eq!(query.rows[0].entity_id, alpha_id);
    assert_eq!(query.rows[0].source_id, source_a);
    let comparison = query.rows[0]
        .comparison
        .as_ref()
        .expect("comparison present");
    assert_eq!(comparison.baseline_display_value, "10");
    assert_eq!(comparison.numeric_delta, Some(5.0));
    assert!(comparison.changed);
    assert!(query
        .groups
        .iter()
        .any(|group| group.key == QueryGroupKey::EntityClass
            && group.value == "Node"
            && group.row_count == 1));
    assert!(query
        .groups
        .iter()
        .any(|group| group.key == QueryGroupKey::SourceId
            && group.value == source_a.to_string()
            && group.row_count == 1));
}
