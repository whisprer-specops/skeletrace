// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Snapshot/export pipeline for materialized outputs.

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::engine::{EngineError, SkeletraceEngine};
use crate::materialize::{SparseGeoFeatureCollection, TopologyViewMaterialization};
use crate::snapshot::{ExportFormat, SnapshotManifest, SnapshotRequest};
use crate::types::{SnapshotId, Timestamp, ValidationError};
use crate::view::{DataCard, ViewJob, ViewKind};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportError {
    Validation(String),
    Io(String),
    Serde(String),
    Sqlite(String),
    Engine(String),
    Unsupported(String),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validation(msg) => write!(f, "export validation error: {msg}"),
            Self::Io(msg) => write!(f, "export I/O error: {msg}"),
            Self::Serde(msg) => write!(f, "export serialization error: {msg}"),
            Self::Sqlite(msg) => write!(f, "export sqlite error: {msg}"),
            Self::Engine(msg) => write!(f, "export engine error: {msg}"),
            Self::Unsupported(msg) => write!(f, "export unsupported: {msg}"),
        }
    }
}

impl std::error::Error for ExportError {}

impl From<ValidationError> for ExportError {
    fn from(value: ValidationError) -> Self {
        Self::Validation(value.to_string())
    }
}

impl From<EngineError> for ExportError {
    fn from(value: EngineError) -> Self {
        Self::Engine(value.to_string())
    }
}

impl From<crate::store::StoreError> for ExportError {
    fn from(value: crate::store::StoreError) -> Self {
        Self::Engine(value.to_string())
    }
}

impl From<rusqlite::Error> for ExportError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sqlite(value.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotExportJob {
    pub request: SnapshotRequest,
    pub view: ViewJob,
    pub output_stem: Option<String>,
}

impl SnapshotExportJob {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.request.validate()?;
        self.view.validate()?;
        if let Some(stem) = &self.output_stem {
            if stem.trim().is_empty() {
                return Err(ValidationError::EmptyField("snapshot.output_stem".into()));
            }
            // Reject paths and Windows-invalid filenames on every platform.
            // `output_stem` names one file inside the export directory.
            let reserved_name = stem.split('.').next().unwrap_or("").to_ascii_uppercase();
            let reserved = matches!(
                reserved_name.as_str(),
                "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
            ) || ["COM", "LPT"].iter().any(|prefix| {
                reserved_name.strip_prefix(prefix).is_some_and(|suffix| {
                    matches!(
                        suffix,
                        "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                    )
                })
            });
            if stem.ends_with(['.', ' '])
                || reserved
                || stem.chars().any(|ch| {
                    ch.is_control()
                        || matches!(ch, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')
                })
            {
                return Err(ValidationError::InvalidState(
                    "snapshot.output_stem must be a portable filename, without paths or reserved names".into(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MaterializedSnapshotPayload {
    Topology(TopologyViewMaterialization),
    SparseGeo(SparseGeoFeatureCollection),
    DataCards(Vec<DataCard>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotExportResult {
    pub manifest: SnapshotManifest,
    pub output_path: PathBuf,
}

pub struct SnapshotExporter {
    output_dir: PathBuf,
    catalog: Option<SqliteSnapshotCatalog>,
}

impl std::fmt::Debug for SnapshotExporter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SnapshotExporter")
            .field("output_dir", &self.output_dir)
            .field("catalog", &self.catalog.as_ref().map(|c| c.path()))
            .finish()
    }
}

impl SnapshotExporter {
    pub fn new(
        output_dir: impl Into<PathBuf>,
        catalog_path: Option<PathBuf>,
    ) -> Result<Self, ExportError> {
        let output_dir = output_dir.into();
        fs::create_dir_all(&output_dir).map_err(|err| ExportError::Io(err.to_string()))?;
        let catalog = match catalog_path {
            Some(path) => Some(SqliteSnapshotCatalog::open(path)?),
            None => None,
        };
        Ok(Self {
            output_dir,
            catalog,
        })
    }

    pub fn export(
        &self,
        engine: &SkeletraceEngine,
        job: &SnapshotExportJob,
        now: Timestamp,
    ) -> Result<SnapshotExportResult, ExportError> {
        job.validate()?;
        let snapshot_id = SnapshotId::new();
        let payload = materialize_payload(engine, job, now)?;
        let bytes = serialize_payload(&payload, job.request.format)?;

        let stem = job
            .output_stem
            .clone()
            .unwrap_or_else(|| snapshot_id.to_string());
        let path =
            self.output_dir
                .join(format!("{}.{}", stem, job.request.format.file_extension()));
        fs::write(&path, &bytes).map_err(|err| ExportError::Io(err.to_string()))?;
        let metadata = fs::metadata(&path).map_err(|err| ExportError::Io(err.to_string()))?;
        if metadata.len() != bytes.len() as u64 {
            return Err(ExportError::Io(format!(
                "snapshot write size mismatch: expected {} bytes, found {} bytes",
                bytes.len(),
                metadata.len()
            )));
        }

        let manifest = SnapshotManifest {
            id: snapshot_id,
            created_at: now,
            entity_count: job.request.entities.len(),
            metric_count: job.request.metrics.len(),
            sample_count: count_samples(engine, job, now)?,
            time_range: job.request.time_range,
            format: job.request.format,
            size_bytes: bytes.len() as u64,
            notes: job.request.notes.clone(),
            storage_path: path.to_string_lossy().to_string(),
        };
        manifest.validate()?;

        if let Some(catalog) = &self.catalog {
            catalog.upsert_manifest(&manifest)?;
        }

        Ok(SnapshotExportResult {
            manifest,
            output_path: path,
        })
    }

    #[must_use]
    pub fn output_dir(&self) -> &Path {
        &self.output_dir
    }

    #[must_use]
    pub fn catalog(&self) -> Option<&SqliteSnapshotCatalog> {
        self.catalog.as_ref()
    }
}

fn materialize_payload(
    engine: &SkeletraceEngine,
    job: &SnapshotExportJob,
    now: Timestamp,
) -> Result<MaterializedSnapshotPayload, ExportError> {
    Ok(match job.view.kind {
        ViewKind::Topology => {
            MaterializedSnapshotPayload::Topology(engine.materialize_topology(&job.view, now)?)
        }
        ViewKind::SparseGeo => MaterializedSnapshotPayload::SparseGeo(
            engine
                .materialize_sparse_geo(&job.view, now)?
                .to_feature_collection(),
        ),
        ViewKind::DataCard | ViewKind::Compare | ViewKind::Timeline | ViewKind::SnapshotExport => {
            let mut cards = Vec::new();
            for entity_id in &job.request.entities {
                cards.push(engine.hydrate_data_card(
                    *entity_id,
                    &job.request.metrics,
                    now,
                    job.request.time_range,
                )?);
            }
            MaterializedSnapshotPayload::DataCards(cards)
        }
    })
}

fn count_samples(
    engine: &SkeletraceEngine,
    job: &SnapshotExportJob,
    now: Timestamp,
) -> Result<usize, ExportError> {
    let mut total = 0usize;
    for entity_id in &job.request.entities {
        for metric_id in &job.request.metrics {
            total += engine
                .store()
                .samples_for_result(*entity_id, *metric_id, job.request.time_range, now)?
                .len();
        }
    }
    Ok(total)
}

fn serialize_payload(
    payload: &MaterializedSnapshotPayload,
    format: ExportFormat,
) -> Result<Vec<u8>, ExportError> {
    match format {
        ExportFormat::NativeJson => {
            serde_json::to_vec_pretty(payload).map_err(|err| ExportError::Serde(err.to_string()))
        }
        ExportFormat::GeoJson => match payload {
            MaterializedSnapshotPayload::SparseGeo(collection) => {
                serde_json::to_vec_pretty(&collection.geojson)
                    .map_err(|err| ExportError::Serde(err.to_string()))
            }
            _ => Err(ExportError::Unsupported(
                "GeoJSON export is only supported for sparse-geo materializations".into(),
            )),
        },
        ExportFormat::Csv => serialize_csv(payload),
    }
}

fn serialize_csv(payload: &MaterializedSnapshotPayload) -> Result<Vec<u8>, ExportError> {
    let mut out = String::new();
    match payload {
        MaterializedSnapshotPayload::Topology(view) => {
            out.push_str("section,entity_id,label,kind,source,target,geometry_mode,metric_count\n");
            for node in &view.nodes {
                out.push_str(&format!(
                    "node,{},{},{},,,,{}\n",
                    node.entity_id,
                    csv_escape(&node.label),
                    csv_escape(&node.kind_label),
                    node.metrics.len()
                ));
            }
            for edge in &view.edges {
                out.push_str(&format!(
                    "edge,,{},,{},{},{},{}\n",
                    edge.entity_id,
                    edge.source,
                    edge.target,
                    csv_escape(&format!("{:?}", edge.geometry_mode)),
                    edge.metrics.len()
                ));
            }
            for boundary in &view.boundaries {
                out.push_str(&format!(
                    "boundary,{},{},{},,,,{}\n",
                    boundary.entity_id,
                    csv_escape(&boundary.label),
                    csv_escape(&boundary.kind_label),
                    boundary.metrics.len()
                ));
            }
        }
        MaterializedSnapshotPayload::SparseGeo(collection) => {
            out.push_str("entity_id,label,kind,geometry_type,metric_count\n");
            if let Some(features) = collection.geojson.get("features").and_then(Value::as_array) {
                for feature in features {
                    let id = feature.get("id").and_then(Value::as_str).unwrap_or("");
                    let props = feature.get("properties").unwrap_or(&Value::Null);
                    let label = props.get("label").and_then(Value::as_str).unwrap_or("");
                    let kind = props.get("kind").and_then(Value::as_str).unwrap_or("");
                    let metrics = props
                        .get("metrics")
                        .and_then(Value::as_object)
                        .map_or(0, |m| m.len());
                    let geometry_type = feature
                        .get("geometry")
                        .and_then(|g| g.get("type"))
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    out.push_str(&format!(
                        "{},{},{},{},{}\n",
                        id,
                        csv_escape(label),
                        csv_escape(kind),
                        geometry_type,
                        metrics
                    ));
                }
            }
        }
        MaterializedSnapshotPayload::DataCards(cards) => {
            out.push_str("entity_id,label,metric_name,display_value,unit,timestamp\n");
            for card in cards {
                for field in &card.summary_fields {
                    out.push_str(&format!(
                        "{},{},{},{},{},{}\n",
                        card.entity_id,
                        csv_escape(&card.label),
                        csv_escape(&field.metric_name),
                        csv_escape(&field.display_value),
                        csv_escape(&field.unit),
                        field.timestamp.to_rfc3339(),
                    ));
                }
            }
        }
    }
    Ok(out.into_bytes())
}

fn csv_escape(value: &str) -> String {
    let escaped = value.replace('"', "\"\"");
    format!("\"{escaped}\"")
}

pub struct SqliteSnapshotCatalog {
    path: PathBuf,
    conn: Connection,
}

impl std::fmt::Debug for SqliteSnapshotCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteSnapshotCatalog")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl SqliteSnapshotCatalog {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, ExportError> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| ExportError::Io(err.to_string()))?;
        }
        let conn = Connection::open(&path)?;
        let store = Self { path, conn };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<(), ExportError> {
        self.conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            CREATE TABLE IF NOT EXISTS snapshot_manifests (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                json TEXT NOT NULL
            );
            ",
        )?;
        Ok(())
    }

    pub fn upsert_manifest(&self, manifest: &SnapshotManifest) -> Result<(), ExportError> {
        manifest.validate()?;
        let json =
            serde_json::to_string(manifest).map_err(|err| ExportError::Serde(err.to_string()))?;
        self.conn.execute(
            "
            INSERT INTO snapshot_manifests(id, created_at, json)
            VALUES(?1, ?2, ?3)
            ON CONFLICT(id) DO UPDATE SET created_at = excluded.created_at, json = excluded.json
            ",
            params![
                manifest.id.to_string(),
                manifest.created_at.to_rfc3339(),
                json
            ],
        )?;
        Ok(())
    }

    pub fn load_manifest(&self, id: SnapshotId) -> Result<Option<SnapshotManifest>, ExportError> {
        let mut stmt = self
            .conn
            .prepare("SELECT json FROM snapshot_manifests WHERE id = ?1")?;
        let mut rows = stmt.query(params![id.to_string()])?;
        if let Some(row) = rows.next()? {
            let json: String = row.get(0)?;
            let manifest: SnapshotManifest =
                serde_json::from_str(&json).map_err(|err| ExportError::Serde(err.to_string()))?;
            manifest.validate()?;
            Ok(Some(manifest))
        } else {
            Ok(None)
        }
    }

    pub fn list_manifests(&self) -> Result<Vec<SnapshotManifest>, ExportError> {
        let mut stmt = self
            .conn
            .prepare("SELECT json FROM snapshot_manifests ORDER BY created_at DESC")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            let json = row?;
            let manifest: SnapshotManifest =
                serde_json::from_str(&json).map_err(|err| ExportError::Serde(err.to_string()))?;
            manifest.validate()?;
            out.push(manifest);
        }
        Ok(out)
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}
