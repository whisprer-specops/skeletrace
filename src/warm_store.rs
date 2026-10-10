// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! SQLite-backed warm history store.
//!
//! The hot path remains in memory. This layer is for slightly longer-lived,
//! queryable history without keeping everything resident in RAM.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::metric::{MetricDefinition, Sample, SampleValue};
use crate::types::{EntityId, MetricId, Quality, SourceId, Timestamp, ValidationError};
use crate::view::TimeRange;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WarmStoreError {
    Validation(String),
    Io(String),
    Sqlite(String),
}

impl std::fmt::Display for WarmStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validation(msg) => write!(f, "warm-store validation error: {msg}"),
            Self::Io(msg) => write!(f, "warm-store I/O error: {msg}"),
            Self::Sqlite(msg) => write!(f, "warm-store SQLite error: {msg}"),
        }
    }
}

impl std::error::Error for WarmStoreError {}

impl From<ValidationError> for WarmStoreError {
    fn from(value: ValidationError) -> Self {
        Self::Validation(value.to_string())
    }
}

impl From<rusqlite::Error> for WarmStoreError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sqlite(value.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WarmStoreMaintenanceReport {
    pub path: PathBuf,
    pub sample_count: usize,
    pub page_size: usize,
    pub page_count: usize,
    pub approx_size_bytes: u64,
    pub index_count: usize,
    pub wal_autocheckpoint_pages: usize,
    pub optimize_ran: bool,
    pub vacuum_ran: bool,
}

pub struct SqliteWarmStore {
    path: PathBuf,
    conn: Connection,
}

impl std::fmt::Debug for SqliteWarmStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteWarmStore")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl SqliteWarmStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, WarmStoreError> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| WarmStoreError::Io(err.to_string()))?;
        }
        let conn = Connection::open(&path)?;
        let store = Self { path, conn };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<(), WarmStoreError> {
        self.conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            CREATE TABLE IF NOT EXISTS warm_samples (
                entity_id TEXT NOT NULL,
                metric_id TEXT NOT NULL,
                source_id TEXT NOT NULL,
                ts_observed TEXT NOT NULL,
                ts_ingested TEXT NOT NULL,
                value_type TEXT NOT NULL,
                value_num REAL,
                value_str TEXT,
                value_flag INTEGER,
                quality REAL NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_warm_samples_lookup
                ON warm_samples(entity_id, metric_id, ts_observed);
            ",
        )?;
        Ok(())
    }

    pub fn insert_sample(&self, sample: &Sample) -> Result<(), WarmStoreError> {
        let (value_type, value_num, value_str, value_flag) = encode_sample_value(&sample.value);
        self.conn.execute(
            "
            INSERT INTO warm_samples (
                entity_id, metric_id, source_id, ts_observed, ts_ingested,
                value_type, value_num, value_str, value_flag, quality
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ",
            params![
                sample.entity_id.to_string(),
                sample.metric_id.to_string(),
                sample.source_id.to_string(),
                sample.ts_observed.to_rfc3339(),
                sample.ts_ingested.to_rfc3339(),
                value_type,
                value_num,
                value_str,
                value_flag,
                sample.quality.get(),
            ],
        )?;
        Ok(())
    }

    pub fn query_samples(
        &self,
        entity_id: EntityId,
        metric_id: MetricId,
        range: TimeRange,
        now: Timestamp,
    ) -> Result<Vec<Sample>, WarmStoreError> {
        let mut stmt = self.conn.prepare(
            "
            SELECT source_id, ts_observed, ts_ingested, value_type, value_num, value_str,
                   value_flag, quality
            FROM warm_samples
            WHERE entity_id = ?1 AND metric_id = ?2
            ORDER BY ts_observed ASC
            ",
        )?;

        let mut rows = stmt.query(params![entity_id.to_string(), metric_id.to_string()])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let sample = Sample {
                entity_id,
                metric_id,
                source_id: parse_source_id(&row.get::<_, String>(0)?)?,
                ts_observed: parse_timestamp(&row.get::<_, String>(1)?)?,
                ts_ingested: parse_timestamp(&row.get::<_, String>(2)?)?,
                value: decode_sample_value(
                    &row.get::<_, String>(3)?,
                    row.get::<_, Option<f64>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                )?,
                quality: Quality::new(row.get::<_, f64>(7)?)?,
            };
            if range.contains(sample.ts_observed, now) {
                out.push(sample);
            }
        }
        Ok(out)
    }

    pub fn prune_for_metrics(
        &self,
        metrics: &HashMap<MetricId, MetricDefinition>,
        now: Timestamp,
    ) -> Result<(), WarmStoreError> {
        for (metric_id, metric) in metrics {
            let cutoff = now - metric.retention.warm_retention_chrono();
            self.conn.execute(
                "DELETE FROM warm_samples WHERE metric_id = ?1 AND ts_observed <= ?2",
                params![metric_id.to_string(), cutoff.to_rfc3339()],
            )?;
        }
        Ok(())
    }

    pub fn sample_count(&self) -> Result<usize, WarmStoreError> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM warm_samples", [], |row| row.get(0))?;
        Ok(count.max(0) as usize)
    }

    pub fn sample_count_for_metric(&self, metric_id: MetricId) -> Result<usize, WarmStoreError> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM warm_samples WHERE metric_id = ?1",
            params![metric_id.to_string()],
            |row| row.get(0),
        )?;
        Ok(count.max(0) as usize)
    }

    pub fn maintenance_report(&self) -> Result<WarmStoreMaintenanceReport, WarmStoreError> {
        let page_size = pragma_usize(&self.conn, "page_size")?;
        let page_count = pragma_usize(&self.conn, "page_count")?;
        let wal_autocheckpoint_pages = pragma_usize(&self.conn, "wal_autocheckpoint")?;
        let index_count = self.index_count()?;
        let sample_count = self.sample_count()?;

        Ok(WarmStoreMaintenanceReport {
            path: self.path.clone(),
            sample_count,
            page_size,
            page_count,
            approx_size_bytes: (page_size as u64).saturating_mul(page_count as u64),
            index_count,
            wal_autocheckpoint_pages,
            optimize_ran: false,
            vacuum_ran: false,
        })
    }

    pub fn optimize(&self, vacuum: bool) -> Result<WarmStoreMaintenanceReport, WarmStoreError> {
        self.conn.execute_batch("PRAGMA optimize;")?;
        if vacuum {
            self.conn.execute_batch("VACUUM;")?;
        }
        let mut report = self.maintenance_report()?;
        report.optimize_ran = true;
        report.vacuum_ran = vacuum;
        Ok(report)
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn index_count(&self) -> Result<usize, WarmStoreError> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND tbl_name = 'warm_samples'",
            [],
            |row| row.get(0),
        )?;
        Ok(count.max(0) as usize)
    }
}

fn encode_sample_value(
    value: &SampleValue,
) -> (&'static str, Option<f64>, Option<String>, Option<i64>) {
    match value {
        SampleValue::Numeric(v) => ("numeric", Some(*v), None, None),
        SampleValue::Code(v) => ("code", None, Some(v.clone()), None),
        SampleValue::Flag(v) => ("flag", None, None, Some(i64::from(*v))),
        SampleValue::Missing => ("missing", None, None, None),
    }
}

fn decode_sample_value(
    value_type: &str,
    value_num: Option<f64>,
    value_str: Option<String>,
    value_flag: Option<i64>,
) -> Result<SampleValue, WarmStoreError> {
    match value_type {
        "numeric" => value_num
            .map(SampleValue::Numeric)
            .ok_or_else(|| WarmStoreError::Validation("numeric sample missing value_num".into())),
        "code" => value_str
            .map(SampleValue::Code)
            .ok_or_else(|| WarmStoreError::Validation("code sample missing value_str".into())),
        "flag" => value_flag
            .map(|flag| SampleValue::Flag(flag != 0))
            .ok_or_else(|| WarmStoreError::Validation("flag sample missing value_flag".into())),
        "missing" => Ok(SampleValue::Missing),
        other => Err(WarmStoreError::Validation(format!(
            "unknown warm-store value type `{other}`"
        ))),
    }
}

fn parse_timestamp(value: &str) -> Result<Timestamp, WarmStoreError> {
    Ok(DateTime::parse_from_rfc3339(value)
        .map_err(|err| WarmStoreError::Validation(err.to_string()))?
        .with_timezone(&Utc))
}

fn parse_source_id(value: &str) -> Result<SourceId, WarmStoreError> {
    Ok(SourceId::from_uuid(Uuid::parse_str(value).map_err(
        |err| WarmStoreError::Validation(err.to_string()),
    )?))
}

fn pragma_usize(conn: &Connection, pragma: &str) -> Result<usize, WarmStoreError> {
    let query = format!("PRAGMA {pragma}");
    let value: i64 = conn.query_row(&query, [], |row| row.get(0))?;
    Ok(value.max(0) as usize)
}
