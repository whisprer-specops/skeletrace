// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

use std::fs;
use std::time::Duration;

use chrono::Utc;
use skeletrace::{
    AdapterKind, AdapterProfile, CacheBudget, CliCommand, Confidence, EngineConfig, EngineProfile,
    EntityId, EntityStatus, ExportFormat, GeoBBox, GeoCoord, InterpolationMethod, MetricDefinition,
    MetricId, MetricValueType, Node, NodeKind, OperatorApi, OperatorRequest, OperatorResponse,
    PollCadence, Priority, Quality, RetentionPolicy, Sample, SampleValue, SnapshotExportJob,
    SnapshotExporter, SnapshotRequest, SourceHealth, SourceId, SourceKind, SourceProfile,
    SourcePull, SourceSchedule, SqliteProfileStore, SqliteSnapshotCatalog, Tag, TimeRange, ViewJob,
    ViewJobId, ViewKind,
};

fn temp_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("skeletrace-stage6-{name}-{}", uuid::Uuid::new_v4()))
}

fn manual_source(source_id: SourceId) -> skeletrace::SourceDefinition {
    skeletrace::SourceDefinition {
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

fn node(entity_id: EntityId, now: chrono::DateTime<chrono::Utc>) -> Node {
    Node {
        id: entity_id,
        kind: NodeKind::Identity,
        label: "alpha".into(),
        position: Some(GeoCoord::new(51.5, -0.1, None).unwrap()),
        position_confidence: Confidence::new(0.95).unwrap(),
        status: EntityStatus::Active,
        priority: Priority::HIGH,
        tags: vec![Tag::new("role", "source").unwrap()],
        first_seen: now,
        last_seen: now,
    }
}

fn sample(entity_id: EntityId, metric_id: MetricId, source_id: SourceId) -> Sample {
    let now = Utc::now();
    Sample {
        entity_id,
        metric_id,
        ts_observed: now,
        ts_ingested: now,
        value: SampleValue::Numeric(42.0),
        quality: Quality::new(1.0).unwrap(),
        source_id,
    }
}

#[test]
fn profile_roundtrips_through_json_and_sqlite() {
    let now = Utc::now();
    let source_id = SourceId::new();
    let metric_id = MetricId::new();
    let entity_id = EntityId::new();

    let profile = EngineProfile {
        name: "stage6-profile".into(),
        config: EngineConfig {
            cache_budget: CacheBudget {
                max_active_entities: 128,
                max_total_ring_samples: 512,
                max_highres_entities: 16,
                max_approx_hot_bytes: 2 * 1024 * 1024,
                max_per_entity_hot_bytes: 64 * 1024,
            },
            default_ring_capacity: 16,
            default_entity_ttl: Duration::from_secs(300),
            journal_dir: None,
            warm_store_path: None,
        },
        metrics: vec![numeric_metric(metric_id, source_id, "volume")],
        nodes: vec![node(entity_id, now)],
        edges: vec![],
        boundaries: vec![],
        sources: vec![SourceProfile {
            definition: manual_source(source_id),
            adapter_profile: AdapterProfile::ManualPush,
        }],
    };

    let json_path = temp_path("profile.json");
    profile.save_json_file(&json_path).unwrap();
    let loaded = EngineProfile::load_json_file(&json_path).unwrap();
    assert_eq!(loaded.name, profile.name);
    assert_eq!(loaded.metrics.len(), 1);
    assert_eq!(loaded.nodes.len(), 1);
    assert_eq!(loaded.sources.len(), 1);

    let sqlite_path = temp_path("profiles.sqlite");
    let store = SqliteProfileStore::open(sqlite_path.clone()).unwrap();
    store.save_profile(&profile, now).unwrap();
    let names = store.list_profiles().unwrap();
    assert_eq!(names, vec![profile.name.clone()]);
    let loaded_sqlite = store.load_profile(&profile.name).unwrap().unwrap();
    assert_eq!(loaded_sqlite.name, profile.name);
}

#[test]
fn snapshot_export_pipeline_writes_geojson_and_catalog() {
    let now = Utc::now();
    let source_id = SourceId::new();
    let metric_id = MetricId::new();
    let entity_id = EntityId::new();

    let profile = EngineProfile {
        name: "stage6-export".into(),
        config: EngineConfig::default(),
        metrics: vec![numeric_metric(metric_id, source_id, "volume")],
        nodes: vec![node(entity_id, now)],
        edges: vec![],
        boundaries: vec![],
        sources: vec![SourceProfile {
            definition: manual_source(source_id),
            adapter_profile: AdapterProfile::ManualPush,
        }],
    };

    let out_dir = temp_path("exports");
    let catalog_path = temp_path("snapshots.sqlite");
    let exporter = SnapshotExporter::new(out_dir.clone(), Some(catalog_path.clone())).unwrap();
    let mut api = OperatorApi::from_profile(&profile, exporter, now).unwrap();

    api.engine_mut()
        .enqueue_manual_batch(
            source_id,
            SourcePull {
                raw_records: vec![],
                samples: vec![sample(entity_id, metric_id, source_id)],
                touched_entities: vec![entity_id],
                nodes: vec![],
                edges: vec![],
                boundaries: vec![],
            },
        )
        .unwrap();

    let poll = api
        .execute(OperatorRequest::PollSource { source_id, now })
        .unwrap();
    match poll {
        OperatorResponse::Tick(report) => assert_eq!(report.samples_seen, 1),
        _ => panic!("expected tick report"),
    }

    let view = ViewJob {
        id: ViewJobId::new(),
        kind: ViewKind::SparseGeo,
        entities: vec![entity_id],
        metrics: vec![metric_id],
        time_range: TimeRange::LatestOnly,
        detail_override: None,
        viewport: Some(GeoBBox::new(45.0, -5.0, 60.0, 5.0).unwrap()),
    };
    let job = SnapshotExportJob {
        request: SnapshotRequest {
            entities: vec![entity_id],
            metrics: vec![metric_id],
            time_range: TimeRange::LatestOnly,
            format: ExportFormat::GeoJson,
            notes: Some("stage6 export".into()),
        },
        view,
        output_stem: Some("sample-export".into()),
    };

    let result = api
        .execute(OperatorRequest::ExportSnapshot { job, now })
        .unwrap();
    let export = match result {
        OperatorResponse::Export(export) => export,
        _ => panic!("expected export response"),
    };

    assert!(export.output_path.exists());
    let body = fs::read_to_string(&export.output_path).unwrap();
    assert!(body.contains("FeatureCollection"));

    let catalog = SqliteSnapshotCatalog::open(catalog_path).unwrap();
    let manifest = catalog.load_manifest(export.manifest.id).unwrap().unwrap();
    assert_eq!(manifest.size_bytes, export.manifest.size_bytes);
}

#[test]
fn operator_api_materializes_sparse_geo_and_cli_parses() {
    let now = Utc::now();
    let source_id = SourceId::new();
    let metric_id = MetricId::new();
    let entity_id = EntityId::new();

    let profile = EngineProfile {
        name: "stage6-operator".into(),
        config: EngineConfig::default(),
        metrics: vec![numeric_metric(metric_id, source_id, "volume")],
        nodes: vec![node(entity_id, now)],
        edges: vec![],
        boundaries: vec![],
        sources: vec![SourceProfile {
            definition: manual_source(source_id),
            adapter_profile: AdapterProfile::ManualPush,
        }],
    };

    let exporter = SnapshotExporter::new(temp_path("operator-out"), None).unwrap();
    let mut api = OperatorApi::from_profile(&profile, exporter, now).unwrap();
    api.engine_mut()
        .enqueue_manual_batch(
            source_id,
            SourcePull {
                raw_records: vec![],
                samples: vec![sample(entity_id, metric_id, source_id)],
                touched_entities: vec![entity_id],
                nodes: vec![],
                edges: vec![],
                boundaries: vec![],
            },
        )
        .unwrap();
    api.execute(OperatorRequest::PollSource { source_id, now })
        .unwrap();

    let response = api
        .execute(OperatorRequest::MaterializeSparseGeo {
            view: ViewJob {
                id: ViewJobId::new(),
                kind: ViewKind::SparseGeo,
                entities: vec![entity_id],
                metrics: vec![metric_id],
                time_range: TimeRange::LatestOnly,
                detail_override: None,
                viewport: Some(GeoBBox::new(45.0, -5.0, 60.0, 5.0).unwrap()),
            },
            now,
        })
        .unwrap();

    match response {
        OperatorResponse::SparseGeo(collection) => {
            assert_eq!(collection.feature_count, 1);
        }
        _ => panic!("expected sparse geo response"),
    }

    let cli = CliCommand::parse_from([
        "skeletrace",
        "export-snapshot",
        "profile.json",
        "job.json",
        "out-dir",
    ])
    .unwrap();
    match cli {
        CliCommand::ExportSnapshot { output_dir, .. } => {
            assert_eq!(output_dir, std::path::PathBuf::from("out-dir"));
        }
        _ => panic!("expected export-snapshot cli command"),
    }
}
