// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Complete offline ingest/query/view/export session using only SkeleTrace.
//! Run: cargo run --locked --example standalone_demo -- ./output/demo

use std::error::Error;
use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use chrono::Utc;
use serde::Serialize;
use skeletrace::{
    AdapterKind, AdapterProfile, AlertRule, Confidence, Edge, EdgeDirection, EdgeKind,
    EngineConfig, EngineProfile, EntityId, EntitySelector, EntityStatus, ExportFormat,
    FileSampleRecord, GeoBBox, GeoCoord, GeometryMode, InterpolationMethod, MetricDefinition,
    MetricId, MetricValueType, Node, NodeKind, OperatorApi, OperatorRequest, OperatorResponse,
    PollCadence, Priority, Quality, QueryFilter, QueryGroupKey, QueryRequest, QuerySortKey,
    RetentionPolicy, SampleValue, SavedQuery, SnapshotExportJob, SnapshotExporter, SnapshotRequest,
    SourceDefinition, SourceHealth, SourceId, SourceKind, SourceProfile, SourceSchedule, TimeRange,
    ViewJob, ViewJobId, ViewKind, WatchItem, Watchlist,
};
use uuid::Uuid;

fn stable_id(name: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("skeletrace:offline-demo:{name}").as_bytes(),
    )
}

fn write_json(path: impl AsRef<Path>, value: &impl Serialize) -> Result<(), Box<dyn Error>> {
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let output = args
        .next()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "output/demo".into());
    if args.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: standalone_demo [output-directory]",
        )
        .into());
    }
    fs::create_dir_all(&output)?;
    let output = fs::canonicalize(output)?;
    let now = Utc::now();
    let source_id = SourceId::from_uuid(stable_id("source"));
    let metric_id = MetricId::from_uuid(stable_id("metric"));
    let entity_ids: Vec<_> = ["a", "b", "c"]
        .iter()
        .map(|name| EntityId::from_uuid(stable_id(name)))
        .collect();
    let labels = [
        "Synthetic station A",
        "Synthetic relay B",
        "Synthetic station C",
    ];
    let nodes = entity_ids
        .iter()
        .enumerate()
        .map(|(i, id)| {
            Ok(Node {
                id: *id,
                kind: NodeKind::Relay,
                label: labels[i].into(),
                position: Some(GeoCoord::new(52.0 + i as f64 * 0.05, -3.0, None)?),
                position_confidence: Confidence::new(1.0)?,
                status: EntityStatus::Active,
                priority: Priority::HIGH,
                tags: vec![],
                first_seen: now,
                last_seen: now,
            })
        })
        .collect::<Result<Vec<_>, skeletrace::ValidationError>>()?;
    let edges = [(0, 1), (1, 2)]
        .iter()
        .enumerate()
        .map(|(i, &(a, b))| {
            Ok(Edge {
                id: EntityId::from_uuid(stable_id(&format!("edge-{i}"))),
                kind: EdgeKind::Link,
                direction: EdgeDirection::Undirected,
                source: entity_ids[a],
                target: entity_ids[b],
                geometry_mode: GeometryMode::Straight,
                waypoints: vec![],
                confidence: Confidence::new(1.0)?,
                status: EntityStatus::Active,
                priority: Priority::HIGH,
                tags: vec![],
                first_seen: now,
                last_seen: now,
            })
        })
        .collect::<Result<Vec<_>, skeletrace::ValidationError>>()?;
    let records = entity_ids
        .iter()
        .enumerate()
        .map(|(i, id)| {
            Ok(FileSampleRecord {
                entity_id: *id,
                metric_id,
                ts_observed: now,
                ts_ingested: None,
                value: SampleValue::Numeric((i + 1) as f64 * 10.0),
                quality: Quality::new(1.0)?,
                raw_payload: Some(serde_json::json!({"synthetic": true, "station": labels[i]})),
            })
        })
        .collect::<Result<Vec<_>, skeletrace::ValidationError>>()?;
    let mut ndjson = String::new();
    for record in records {
        ndjson.push_str(&serde_json::to_string(&record)?);
        ndjson.push('\n');
    }
    let input_path = output.join("observations.ndjson");
    fs::write(&input_path, ndjson)?;
    let profile = EngineProfile {
        name: "SkeleTrace offline demonstration".into(),
        config: EngineConfig {
            warm_store_path: Some(output.join("samples.sqlite")),
            ..EngineConfig::default()
        },
        metrics: vec![MetricDefinition {
            id: metric_id,
            name: "synthetic volume".into(),
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
                max_silent_gap: Duration::from_secs(60),
            },
            source_ids: vec![source_id],
            show_in_popup: true,
            popup_priority: 1,
            description: "Synthetic demonstration values, not real intelligence".into(),
        }],
        nodes,
        edges,
        boundaries: vec![],
        sources: vec![SourceProfile {
            definition: SourceDefinition {
                id: source_id,
                name: "offline synthetic observations".into(),
                kind: SourceKind::File,
                adapter: AdapterKind::FileImport,
                schedule: SourceSchedule::Manual,
                endpoint: input_path.to_string_lossy().into(),
                auth_ref: None,
                health: SourceHealth::Pending,
                last_polled: None,
                last_error: None,
                backoff: Duration::from_secs(5),
                max_backoff: Duration::from_secs(30),
                tags: vec![],
            },
            adapter_profile: AdapterProfile::NdjsonFile,
        }],
    };
    profile.save_json_file(output.join("profile.json"))?;
    let topology = ViewJob {
        id: ViewJobId::from_uuid(stable_id("topology")),
        kind: ViewKind::Topology,
        entities: entity_ids.clone(),
        metrics: vec![metric_id],
        time_range: TimeRange::LatestOnly,
        detail_override: None,
        viewport: None,
    };
    let geo = ViewJob {
        id: ViewJobId::from_uuid(stable_id("geo")),
        kind: ViewKind::SparseGeo,
        viewport: Some(GeoBBox::new(51.0, -4.0, 53.0, -2.0)?),
        ..topology.clone()
    };
    let filter = QueryFilter {
        entities: EntitySelector {
            entity_ids: vec![],
            label_contains: None,
            class: None,
        },
        metric_ids: vec![metric_id],
        only_hot: false,
        numeric_predicate: None,
        limit: None,
        source_ids: vec![],
        entity_statuses: vec![],
        required_tags: vec![],
    };
    let saved_query = SavedQuery {
        label: "Synthetic volume".into(),
        request: QueryRequest {
            label: Some("Offline demonstration".into()),
            filter: filter.clone(),
            sort: Some(QuerySortKey::ValueDescending),
            group_by: vec![QueryGroupKey::SourceId],
            compare_range: None,
        },
    };
    let watchlist = Watchlist {
        label: "Synthetic threshold demonstration".into(),
        items: vec![WatchItem {
            label: "Station C above 25".into(),
            entity_id: entity_ids[2],
            metric_id,
            rule: AlertRule::NumericAtLeast(25.0),
        }],
    };
    let job = SnapshotExportJob {
        request: SnapshotRequest {
            entities: entity_ids,
            metrics: vec![metric_id],
            time_range: TimeRange::LatestOnly,
            format: ExportFormat::GeoJson,
            notes: Some("Synthetic offline demonstration".into()),
        },
        view: geo.clone(),
        output_stem: Some("synthetic-map".into()),
    };
    write_json(output.join("topology-view.json"), &topology)?;
    write_json(output.join("geo-view.json"), &geo)?;
    write_json(output.join("filter.json"), &filter)?;
    write_json(output.join("saved-query.json"), &saved_query)?;
    write_json(output.join("watchlist.json"), &watchlist)?;
    write_json(output.join("snapshot-job.json"), &job)?;
    let exporter =
        SnapshotExporter::new(output.join("exports"), Some(output.join("catalog.sqlite")))?;
    let mut api = OperatorApi::from_profile(&profile, exporter, now)?;
    let ingested = api.execute(OperatorRequest::PollSource { source_id, now })?;
    if !matches!(&ingested, OperatorResponse::Tick(report) if report.samples_seen == 3 && report.samples_stored == 3)
    {
        return Err(io::Error::other("offline ingest contract failed").into());
    }
    let query = api.execute(OperatorRequest::QueryAdvanced {
        request: saved_query.request,
        now,
    })?;
    if !matches!(&query, OperatorResponse::QueryEnvelope(result) if result.rows.len() == 3) {
        return Err(io::Error::other("offline query contract failed").into());
    }
    let analysis = api.execute(OperatorRequest::AnalyzeTopology {
        view: topology.clone(),
        now,
    })?;
    if !matches!(&analysis, OperatorResponse::TopologyAnalysis(result) if result.chokepoints.len() == 1 && result.bridges.len() == 2)
    {
        return Err(io::Error::other("offline topology analysis contract failed").into());
    }
    let report = serde_json::json!({
        "synthetic": true,
        "ingest": ingested,
        "topology": api.execute(OperatorRequest::MaterializeTopology { view: topology, now })?,
        "geography": api.execute(OperatorRequest::MaterializeSparseGeo { view: geo, now })?,
        "analysis": analysis,
        "query": query,
        "watchlist": api.execute(OperatorRequest::EvaluateWatchlist { watchlist, now })?,
        "export": api.execute(OperatorRequest::ExportSnapshot { job, now })?,
        "health": api.execute(OperatorRequest::HealthReport { now })?,
    });
    write_json(output.join("report.json"), &report)?;
    println!("Offline workflow passed: 3 samples, 3 query rows, 1 chokepoint, 2 bridges.");
    println!("Report: {}", output.join("report.json").display());
    Ok(())
}
