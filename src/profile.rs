// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Runtime profile/config loading.
//!
//! Profiles are deliberately JSON-first for low-dependency portability, while
//! SQLite is used as an optional local catalog for saved named profiles.

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::adapter::{
    AdapterError, FeedPollAdapter, HttpJsonAdapter, ManualPushAdapter, NdjsonSampleFileAdapter,
    SourceAdapter, TorHttpJsonAdapter,
};
use crate::engine::{EngineConfig, EngineError, SkeletraceEngine};
use crate::entity::{Boundary, Edge, Node};
use crate::ingest::{AdapterKind, SourceDefinition};
use crate::lookyloo::{
    LookylooSourceConfig, LookylooSummaryAdapter, LookylooTopologyAdapter, LookylooTopologyConfig,
};
use crate::mapping::SourceMappingConfig;
use crate::metric::MetricDefinition;
use crate::transport::HttpRequestProfile;
use crate::types::{Timestamp, ValidationError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProfileError {
    Validation(String),
    Io(String),
    Serde(String),
    Sqlite(String),
    Adapter(String),
    Engine(String),
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validation(msg) => write!(f, "profile validation error: {msg}"),
            Self::Io(msg) => write!(f, "profile I/O error: {msg}"),
            Self::Serde(msg) => write!(f, "profile serialization error: {msg}"),
            Self::Sqlite(msg) => write!(f, "profile sqlite error: {msg}"),
            Self::Adapter(msg) => write!(f, "profile adapter error: {msg}"),
            Self::Engine(msg) => write!(f, "profile engine error: {msg}"),
        }
    }
}

impl std::error::Error for ProfileError {}

impl From<ValidationError> for ProfileError {
    fn from(value: ValidationError) -> Self {
        Self::Validation(value.to_string())
    }
}

impl From<AdapterError> for ProfileError {
    fn from(value: AdapterError) -> Self {
        Self::Adapter(value.to_string())
    }
}

impl From<EngineError> for ProfileError {
    fn from(value: EngineError) -> Self {
        Self::Engine(value.to_string())
    }
}

impl From<rusqlite::Error> for ProfileError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sqlite(value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AdapterProfile {
    ManualPush,
    NdjsonFile,
    HttpJson {
        mapping: SourceMappingConfig,
        request_profile: HttpRequestProfile,
    },
    TorHttpJson {
        mapping: SourceMappingConfig,
        request_profile: HttpRequestProfile,
    },
    FeedPoll {
        mapping: SourceMappingConfig,
        request_profile: HttpRequestProfile,
    },
    LookylooSummary {
        config: LookylooSourceConfig,
        request_profile: HttpRequestProfile,
    },
    LookylooTopology {
        config: LookylooTopologyConfig,
        request_profile: HttpRequestProfile,
    },
}

impl AdapterProfile {
    #[must_use]
    pub const fn expected_kind(&self) -> AdapterKind {
        match self {
            Self::ManualPush => AdapterKind::Manual,
            Self::NdjsonFile => AdapterKind::FileImport,
            Self::HttpJson { .. } => AdapterKind::HttpPoller,
            Self::TorHttpJson { .. } => AdapterKind::TorHttpPoller,
            Self::FeedPoll { .. } => AdapterKind::FeedPoller,
            Self::LookylooSummary { .. } => AdapterKind::HttpPoller,
            Self::LookylooTopology { .. } => AdapterKind::HttpPoller,
        }
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::ManualPush | Self::NdjsonFile => Ok(()),
            Self::HttpJson {
                mapping,
                request_profile,
            }
            | Self::TorHttpJson {
                mapping,
                request_profile,
            }
            | Self::FeedPoll {
                mapping,
                request_profile,
            } => {
                mapping.validate()?;
                request_profile.validate()?;
                Ok(())
            }
            Self::LookylooSummary {
                config,
                request_profile,
            } => {
                config.validate()?;
                request_profile.validate()?;
                Ok(())
            }
            Self::LookylooTopology {
                config,
                request_profile,
            } => {
                config.validate()?;
                request_profile.validate()?;
                Ok(())
            }
        }
    }

    pub fn build_adapter(&self) -> Result<Box<dyn SourceAdapter>, ProfileError> {
        Ok(match self {
            Self::ManualPush => Box::new(ManualPushAdapter::new()),
            Self::NdjsonFile => Box::new(NdjsonSampleFileAdapter::new()),
            Self::HttpJson {
                mapping,
                request_profile,
            } => Box::new(HttpJsonAdapter::with_request_profile(
                mapping.clone(),
                request_profile.clone(),
            )?),
            Self::TorHttpJson {
                mapping,
                request_profile,
            } => Box::new(TorHttpJsonAdapter::with_request_profile(
                mapping.clone(),
                request_profile.clone(),
            )?),
            Self::FeedPoll {
                mapping,
                request_profile,
            } => Box::new(FeedPollAdapter::with_request_profile(
                mapping.clone(),
                request_profile.clone(),
            )?),
            Self::LookylooSummary {
                config,
                request_profile,
            } => Box::new(LookylooSummaryAdapter::with_request_profile(
                config.clone(),
                request_profile.clone(),
            )?),
            Self::LookylooTopology {
                config,
                request_profile,
            } => Box::new(LookylooTopologyAdapter::with_request_profile(
                config.clone(),
                request_profile.clone(),
            )?),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceProfile {
    pub definition: SourceDefinition,
    pub adapter_profile: AdapterProfile,
}

impl SourceProfile {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.definition.validate()?;
        self.adapter_profile.validate()?;
        if self.definition.adapter != self.adapter_profile.expected_kind() {
            return Err(ValidationError::InvalidState(format!(
                "source `{}` adapter kind {:?} does not match profile {:?}",
                self.definition.name,
                self.definition.adapter,
                self.adapter_profile.expected_kind()
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineProfile {
    pub name: String,
    pub config: EngineConfig,
    pub metrics: Vec<MetricDefinition>,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub boundaries: Vec<Boundary>,
    pub sources: Vec<SourceProfile>,
}

impl EngineProfile {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.name.trim().is_empty() {
            return Err(ValidationError::EmptyField("profile.name".into()));
        }
        self.config.validate()?;
        for metric in &self.metrics {
            metric.validate()?;
        }
        for node in &self.nodes {
            node.validate()?;
        }
        for edge in &self.edges {
            edge.validate()?;
        }
        for boundary in &self.boundaries {
            boundary.validate()?;
        }
        for source in &self.sources {
            source.validate()?;
        }
        Ok(())
    }

    pub fn save_json_file(&self, path: impl AsRef<Path>) -> Result<(), ProfileError> {
        self.validate()?;
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| ProfileError::Io(err.to_string()))?;
        }
        let bytes =
            serde_json::to_vec_pretty(self).map_err(|err| ProfileError::Serde(err.to_string()))?;
        fs::write(path, bytes).map_err(|err| ProfileError::Io(err.to_string()))?;
        Ok(())
    }

    pub fn load_json_file(path: impl AsRef<Path>) -> Result<Self, ProfileError> {
        let bytes = fs::read(path).map_err(|err| ProfileError::Io(err.to_string()))?;
        let profile: Self =
            serde_json::from_slice(&bytes).map_err(|err| ProfileError::Serde(err.to_string()))?;
        profile.validate()?;
        Ok(profile)
    }

    pub fn instantiate(&self, now: Timestamp) -> Result<SkeletraceEngine, ProfileError> {
        self.validate()?;
        let mut engine = SkeletraceEngine::new(self.config.clone())?;
        for metric in &self.metrics {
            engine.register_metric(metric.clone())?;
        }
        for node in &self.nodes {
            engine.register_node(node.clone())?;
        }
        for edge in &self.edges {
            engine.register_edge(edge.clone())?;
        }
        for boundary in &self.boundaries {
            engine.register_boundary(boundary.clone())?;
        }
        for source in &self.sources {
            engine.register_source(
                source.definition.clone(),
                source.adapter_profile.build_adapter()?,
                now,
            )?;
        }
        Ok(engine)
    }
}

pub struct SqliteProfileStore {
    path: PathBuf,
    conn: Connection,
}

impl std::fmt::Debug for SqliteProfileStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteProfileStore")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl SqliteProfileStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, ProfileError> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| ProfileError::Io(err.to_string()))?;
        }
        let conn = Connection::open(&path)?;
        let store = Self { path, conn };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<(), ProfileError> {
        self.conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            CREATE TABLE IF NOT EXISTS profiles (
                name TEXT PRIMARY KEY,
                saved_at TEXT NOT NULL,
                json TEXT NOT NULL
            );
            ",
        )?;
        Ok(())
    }

    pub fn save_profile(
        &self,
        profile: &EngineProfile,
        now: Timestamp,
    ) -> Result<(), ProfileError> {
        profile.validate()?;
        let json =
            serde_json::to_string(profile).map_err(|err| ProfileError::Serde(err.to_string()))?;
        self.conn.execute(
            "
            INSERT INTO profiles(name, saved_at, json)
            VALUES(?1, ?2, ?3)
            ON CONFLICT(name) DO UPDATE SET saved_at = excluded.saved_at, json = excluded.json
            ",
            params![&profile.name, now.to_rfc3339(), json],
        )?;
        Ok(())
    }

    pub fn load_profile(&self, name: &str) -> Result<Option<EngineProfile>, ProfileError> {
        let mut stmt = self
            .conn
            .prepare("SELECT json FROM profiles WHERE name = ?1")?;
        let mut rows = stmt.query(params![name])?;
        if let Some(row) = rows.next()? {
            let json: String = row.get(0)?;
            let profile: EngineProfile =
                serde_json::from_str(&json).map_err(|err| ProfileError::Serde(err.to_string()))?;
            profile.validate()?;
            Ok(Some(profile))
        } else {
            Ok(None)
        }
    }

    pub fn list_profiles(&self) -> Result<Vec<String>, ProfileError> {
        let mut stmt = self
            .conn
            .prepare("SELECT name FROM profiles ORDER BY name ASC")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}
