// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! First operator-facing API and thin CLI command layer.

use std::fs;
use std::path::PathBuf;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::chain::{strongest_chain, ChainError, StrongestChain};
use crate::engine::{EngineError, SkeletraceEngine, TickReport};
use crate::export::{ExportError, SnapshotExportJob, SnapshotExportResult, SnapshotExporter};
use crate::geofence::{
    cover_area, fence_check, fences_from_geojson, CoverArea, FenceCheck, FenceError, GeoFence,
};
use crate::governance::{AuditRecord, FailureRecord, SourceCapabilityProfile, SourcePolicy};
use crate::materialize::{SparseGeoFeatureCollection, TopologyViewMaterialization};
use crate::metric::{MetricRetentionReport, RetentionTuning};
use crate::observability::EngineEvent;
use crate::profile::{EngineProfile, ProfileError};
use crate::profiling::{DurationStats, EngineHealthReport, PerfProbeConfig, PerfProbeReport};
use crate::query::{
    QueryFilter, QueryRequest, QueryResult, QueryResultEnvelope, SavedQuery, Watchlist,
    WatchlistEvaluation,
};
use crate::sweep::{parse_terms, sweep_files, SweepError, SweepReport, SweepRequest};
use crate::topology_analysis::{analyze_topology, TopologyAnalysis};
use crate::types::{EntityId, MetricId, SourceId, Timestamp, ValidationError};
use crate::view::{DataCard, ViewJob};
use crate::warm_store::WarmStoreMaintenanceReport;
use crate::workload::{
    ReplayBenchmarkReport, ReplayBenchmarkRequest, ReplayWorkloadRequest, WorkloadFixture,
    WorkloadRunReport,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperatorError {
    Validation(String),
    Profile(String),
    Engine(String),
    Export(String),
    Io(String),
    Serde(String),
    Cli(String),
}

impl std::fmt::Display for OperatorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validation(msg) => write!(f, "operator validation error: {msg}"),
            Self::Profile(msg) => write!(f, "operator profile error: {msg}"),
            Self::Engine(msg) => write!(f, "operator engine error: {msg}"),
            Self::Export(msg) => write!(f, "operator export error: {msg}"),
            Self::Io(msg) => write!(f, "operator I/O error: {msg}"),
            Self::Serde(msg) => write!(f, "operator serialization error: {msg}"),
            Self::Cli(msg) => write!(f, "operator CLI error: {msg}"),
        }
    }
}

impl std::error::Error for OperatorError {}

impl From<ValidationError> for OperatorError {
    fn from(value: ValidationError) -> Self {
        Self::Validation(value.to_string())
    }
}

impl From<ProfileError> for OperatorError {
    fn from(value: ProfileError) -> Self {
        Self::Profile(value.to_string())
    }
}

impl From<EngineError> for OperatorError {
    fn from(value: EngineError) -> Self {
        Self::Engine(value.to_string())
    }
}

impl From<ExportError> for OperatorError {
    fn from(value: ExportError) -> Self {
        Self::Export(value.to_string())
    }
}

impl From<FenceError> for OperatorError {
    fn from(value: FenceError) -> Self {
        Self::Validation(value.to_string())
    }
}

impl From<ChainError> for OperatorError {
    fn from(value: ChainError) -> Self {
        Self::Validation(value.to_string())
    }
}

impl From<SweepError> for OperatorError {
    fn from(value: SweepError) -> Self {
        match value {
            SweepError::Io { .. } => Self::Io(value.to_string()),
            _ => Self::Validation(value.to_string()),
        }
    }
}

pub struct OperatorApi {
    engine: SkeletraceEngine,
    exporter: SnapshotExporter,
    base_profile: Option<EngineProfile>,
}

impl std::fmt::Debug for OperatorApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OperatorApi")
            .field("exporter", &self.exporter)
            .field("engine_stats", &self.engine.stats())
            .field("has_base_profile", &self.base_profile.is_some())
            .finish()
    }
}

impl OperatorApi {
    pub fn new(engine: SkeletraceEngine, exporter: SnapshotExporter) -> Self {
        Self {
            engine,
            exporter,
            base_profile: None,
        }
    }

    pub fn from_profile(
        profile: &EngineProfile,
        exporter: SnapshotExporter,
        now: Timestamp,
    ) -> Result<Self, OperatorError> {
        Ok(Self {
            engine: profile.instantiate(now)?,
            exporter,
            base_profile: Some(profile.clone()),
        })
    }

    #[must_use]
    pub fn engine(&self) -> &SkeletraceEngine {
        &self.engine
    }

    #[must_use]
    pub fn engine_mut(&mut self) -> &mut SkeletraceEngine {
        &mut self.engine
    }

    fn benchmark_replay_workload(
        &self,
        fixture: &WorkloadFixture,
        request: &ReplayBenchmarkRequest,
        now: Timestamp,
    ) -> Result<ReplayBenchmarkReport, OperatorError> {
        fixture.validate()?;
        request.validate()?;
        let profile = self.base_profile.as_ref().ok_or_else(|| {
            OperatorError::Cli("benchmarking requires an operator created from a profile".into())
        })?;

        let started_at = now;
        let mut stats = DurationStats::default();
        let mut total_checkpoints = 0usize;
        let mut total_samples_seen = 0usize;
        let mut total_samples_stored = 0usize;
        let mut last_run = None;

        for _ in 0..request.iterations {
            let mut benchmark_profile = profile.clone();
            benchmark_profile.config.journal_dir = None;
            benchmark_profile.config.warm_store_path = None;
            let mut engine = benchmark_profile.instantiate(now)?;
            let mut harness = fixture.into_harness();
            let started = std::time::Instant::now();
            let report = engine.run_replay_workload(&mut harness, &request.workload)?;
            stats.record(started.elapsed());
            total_checkpoints += report.checkpoints.len();
            total_samples_seen += report
                .checkpoints
                .iter()
                .map(|checkpoint| checkpoint.replay.samples_seen)
                .sum::<usize>();
            total_samples_stored += report
                .checkpoints
                .iter()
                .map(|checkpoint| checkpoint.replay.samples_stored)
                .sum::<usize>();
            last_run = Some(report);
        }

        Ok(ReplayBenchmarkReport {
            label: request.label.clone(),
            iterations: request.iterations,
            started_at,
            finished_at: chrono::Utc::now(),
            stats,
            source_summaries: fixture.source_summaries(),
            total_checkpoints,
            total_samples_seen,
            total_samples_stored,
            last_run,
        })
    }

    pub fn execute(&mut self, request: OperatorRequest) -> Result<OperatorResponse, OperatorError> {
        Ok(match request {
            OperatorRequest::AnalyzeTopology { view, now } => {
                let materialized = self.engine.materialize_topology(&view, now)?;
                OperatorResponse::TopologyAnalysis(analyze_topology(&materialized))
            }
            OperatorRequest::FenceCheck { view, fences, now } => {
                let materialized = self.engine.materialize_topology(&view, now)?;
                OperatorResponse::FenceCheck(fence_check(&materialized, &fences)?)
            }
            OperatorRequest::CoverArea { view, now } => {
                let materialized = self.engine.materialize_topology(&view, now)?;
                OperatorResponse::CoverArea(cover_area(&materialized))
            }
            OperatorRequest::StrongestChain {
                view,
                from,
                to,
                now,
            } => {
                let materialized = self.engine.materialize_topology(&view, now)?;
                let store = self.engine.store();
                OperatorResponse::StrongestChain(strongest_chain(
                    &materialized,
                    |link| store.edge(link).map(|edge| edge.confidence.get()),
                    &from,
                    &to,
                )?)
            }
            OperatorRequest::Tick { now } => OperatorResponse::Tick(self.engine.tick(now)?),
            OperatorRequest::PollSource { source_id, now } => {
                OperatorResponse::Tick(self.engine.poll_source_now(source_id, now)?)
            }
            OperatorRequest::MaterializeTopology { view, now } => {
                OperatorResponse::Topology(self.engine.materialize_topology(&view, now)?)
            }
            OperatorRequest::MaterializeSparseGeo { view, now } => {
                let mat = self.engine.materialize_sparse_geo(&view, now)?;
                OperatorResponse::SparseGeo(mat.to_feature_collection())
            }
            OperatorRequest::HydrateDataCard {
                entity_id,
                metrics,
                now,
                time_range,
            } => OperatorResponse::DataCard(
                self.engine
                    .hydrate_data_card(entity_id, &metrics, now, time_range)?,
            ),
            OperatorRequest::ExportSnapshot { job, now } => {
                let result = self.exporter.export(&self.engine, &job, now)?;
                self.engine.record_export_audit(&result, &job, now);
                OperatorResponse::Export(result)
            }
            OperatorRequest::QueryLatest { filter, now } => {
                OperatorResponse::Query(self.engine.query_latest(&filter, now)?)
            }
            OperatorRequest::QueryAdvanced { request, now } => {
                OperatorResponse::QueryEnvelope(self.engine.query_request(&request, now)?)
            }
            OperatorRequest::EvaluateWatchlist { watchlist, now } => {
                OperatorResponse::Watchlist(self.engine.evaluate_watchlist(&watchlist, now)?)
            }
            OperatorRequest::HealthReport { now } => {
                OperatorResponse::HealthReport(self.engine.health_report(now))
            }
            OperatorRequest::RecentEvents { limit } => {
                OperatorResponse::Events(self.engine.recent_events(limit))
            }
            OperatorRequest::TuneMetricRetention {
                metric_id,
                tuning,
                now,
            } => OperatorResponse::RetentionReport(
                self.engine
                    .retune_metric_retention(metric_id, tuning, now)?,
            ),
            OperatorRequest::ProfileView { view, now, config } => OperatorResponse::Perf(
                self.engine
                    .profile_view_materialization(&view, now, config)?,
            ),
            OperatorRequest::RecentAudit { limit } => {
                OperatorResponse::Audit(self.engine.recent_audit_records(limit))
            }
            OperatorRequest::RecentFailures { limit } => {
                OperatorResponse::Failures(self.engine.recent_failures(limit))
            }
            OperatorRequest::SetSourcePolicy { policy } => {
                self.engine.set_source_policy(policy);
                OperatorResponse::SourcePolicy(policy)
            }
            OperatorRequest::SetSourceCapability {
                source_id,
                capability,
            } => {
                self.engine.set_source_capability(source_id, capability)?;
                OperatorResponse::SourceCapability(capability)
            }
            OperatorRequest::WarmStoreReport => {
                OperatorResponse::WarmStoreReport(self.engine.warm_store_maintenance_report()?)
            }
            OperatorRequest::OptimizeWarmStore { vacuum } => {
                OperatorResponse::WarmStoreReport(self.engine.optimize_warm_store(vacuum)?)
            }
            OperatorRequest::RunReplayWorkload { fixture, request } => {
                fixture.validate()?;
                let mut harness = fixture.into_harness();
                OperatorResponse::Workload(self.engine.run_replay_workload(&mut harness, &request)?)
            }
            OperatorRequest::BenchmarkReplayWorkload {
                fixture,
                request,
                now,
            } => OperatorResponse::ReplayBenchmark(
                self.benchmark_replay_workload(&fixture, &request, now)?,
            ),
            OperatorRequest::SweepText { request } => {
                OperatorResponse::Sweep(sweep_files(&request)?)
            }
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OperatorRequest {
    AnalyzeTopology {
        view: ViewJob,
        now: Timestamp,
    },
    /// Which nodes of a topology view fall inside each fence.
    FenceCheck {
        view: ViewJob,
        fences: Vec<GeoFence>,
        now: Timestamp,
    },
    /// The outline round every positioned node of a topology view, and its area.
    CoverArea {
        view: ViewJob,
        now: Timestamp,
    },
    /// The most believable chain of links between two nodes of a topology view, each named by ID or label.
    StrongestChain {
        view: ViewJob,
        from: String,
        to: String,
        now: Timestamp,
    },
    Tick {
        now: Timestamp,
    },
    PollSource {
        source_id: SourceId,
        now: Timestamp,
    },
    MaterializeTopology {
        view: ViewJob,
        now: Timestamp,
    },
    MaterializeSparseGeo {
        view: ViewJob,
        now: Timestamp,
    },
    HydrateDataCard {
        entity_id: EntityId,
        metrics: Vec<MetricId>,
        now: Timestamp,
        time_range: crate::view::TimeRange,
    },
    ExportSnapshot {
        job: SnapshotExportJob,
        now: Timestamp,
    },
    QueryLatest {
        filter: QueryFilter,
        now: Timestamp,
    },
    QueryAdvanced {
        request: QueryRequest,
        now: Timestamp,
    },
    EvaluateWatchlist {
        watchlist: Watchlist,
        now: Timestamp,
    },
    HealthReport {
        now: Timestamp,
    },
    RecentEvents {
        limit: usize,
    },
    TuneMetricRetention {
        metric_id: MetricId,
        tuning: RetentionTuning,
        now: Timestamp,
    },
    ProfileView {
        view: ViewJob,
        now: Timestamp,
        config: PerfProbeConfig,
    },
    RecentAudit {
        limit: usize,
    },
    RecentFailures {
        limit: usize,
    },
    SetSourcePolicy {
        policy: SourcePolicy,
    },
    SetSourceCapability {
        source_id: SourceId,
        capability: SourceCapabilityProfile,
    },
    WarmStoreReport,
    OptimizeWarmStore {
        vacuum: bool,
    },
    RunReplayWorkload {
        fixture: WorkloadFixture,
        request: ReplayWorkloadRequest,
    },
    BenchmarkReplayWorkload {
        fixture: WorkloadFixture,
        request: ReplayBenchmarkRequest,
        now: Timestamp,
    },
    /// Find every watch-list term in some text files. Needs no engine state.
    SweepText {
        request: SweepRequest,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OperatorResponse {
    Tick(TickReport),
    Topology(TopologyViewMaterialization),
    SparseGeo(SparseGeoFeatureCollection),
    DataCard(DataCard),
    Export(SnapshotExportResult),
    Query(QueryResult),
    QueryEnvelope(QueryResultEnvelope),
    TopologyAnalysis(TopologyAnalysis),
    StrongestChain(StrongestChain),
    FenceCheck(FenceCheck),
    CoverArea(CoverArea),
    Watchlist(WatchlistEvaluation),
    HealthReport(EngineHealthReport),
    Events(Vec<EngineEvent>),
    RetentionReport(MetricRetentionReport),
    Perf(PerfProbeReport),
    Audit(Vec<AuditRecord>),
    Failures(Vec<FailureRecord>),
    SourcePolicy(SourcePolicy),
    SourceCapability(SourceCapabilityProfile),
    WarmStoreReport(Option<WarmStoreMaintenanceReport>),
    Workload(WorkloadRunReport),
    ReplayBenchmark(ReplayBenchmarkReport),
    Sweep(SweepReport),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliCommand {
    ValidateProfile {
        profile_path: PathBuf,
    },
    AnalyzeTopology {
        profile_path: PathBuf,
        view_path: PathBuf,
    },
    StrongestChain {
        profile_path: PathBuf,
        view_path: PathBuf,
        from: String,
        to: String,
    },
    FenceCheck {
        profile_path: PathBuf,
        view_path: PathBuf,
        fences_path: PathBuf,
    },
    CoverArea {
        profile_path: PathBuf,
        view_path: PathBuf,
    },
    Tick {
        profile_path: PathBuf,
    },
    PollSource {
        profile_path: PathBuf,
        source_id: String,
    },
    MaterializeTopology {
        profile_path: PathBuf,
        view_path: PathBuf,
    },
    MaterializeSparseGeo {
        profile_path: PathBuf,
        view_path: PathBuf,
    },
    ExportSnapshot {
        profile_path: PathBuf,
        job_path: PathBuf,
        output_dir: PathBuf,
        catalog_path: Option<PathBuf>,
    },
    QueryLatest {
        profile_path: PathBuf,
        filter_path: PathBuf,
    },
    QueryAdvanced {
        profile_path: PathBuf,
        request_path: PathBuf,
    },
    EvaluateWatchlist {
        profile_path: PathBuf,
        watchlist_path: PathBuf,
    },
    Health {
        profile_path: PathBuf,
    },
    SweepText {
        terms_path: PathBuf,
        input_paths: Vec<PathBuf>,
        exact: bool,
        /// The most typos a near-miss may have: 1 or 2, or 0 for exact matches only.
        fuzzy: usize,
        max_hits: usize,
    },
}

impl CliCommand {
    pub fn parse_from<I, S>(args: I) -> Result<Self, OperatorError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let values: Vec<String> = args.into_iter().map(Into::into).collect();
        if values.len() < 2 {
            return Err(OperatorError::Cli("missing subcommand".into()));
        }
        match values[1].as_str() {
            "profile-validate" if values.len() == 3 => Ok(Self::ValidateProfile {
                profile_path: PathBuf::from(&values[2]),
            }),
            "analyze-topology" if values.len() == 4 => Ok(Self::AnalyzeTopology {
                profile_path: PathBuf::from(&values[2]),
                view_path: PathBuf::from(&values[3]),
            }),
            "strongest-chain" if values.len() == 6 => Ok(Self::StrongestChain {
                profile_path: PathBuf::from(&values[2]),
                view_path: PathBuf::from(&values[3]),
                from: values[4].clone(),
                to: values[5].clone(),
            }),
            "fence-check" if values.len() == 5 => Ok(Self::FenceCheck {
                profile_path: PathBuf::from(&values[2]),
                view_path: PathBuf::from(&values[3]),
                fences_path: PathBuf::from(&values[4]),
            }),
            "cover-area" if values.len() == 4 => Ok(Self::CoverArea {
                profile_path: PathBuf::from(&values[2]),
                view_path: PathBuf::from(&values[3]),
            }),
            "tick" if values.len() == 3 => Ok(Self::Tick {
                profile_path: PathBuf::from(&values[2]),
            }),
            "poll-source" if values.len() == 4 => Ok(Self::PollSource {
                profile_path: PathBuf::from(&values[2]),
                source_id: values[3].clone(),
            }),
            "materialize-topology" if values.len() == 4 => Ok(Self::MaterializeTopology {
                profile_path: PathBuf::from(&values[2]),
                view_path: PathBuf::from(&values[3]),
            }),
            "materialize-sparse-geo" if values.len() == 4 => Ok(Self::MaterializeSparseGeo {
                profile_path: PathBuf::from(&values[2]),
                view_path: PathBuf::from(&values[3]),
            }),
            "export-snapshot" if values.len() == 5 || values.len() == 6 => {
                Ok(Self::ExportSnapshot {
                    profile_path: PathBuf::from(&values[2]),
                    job_path: PathBuf::from(&values[3]),
                    output_dir: PathBuf::from(&values[4]),
                    catalog_path: values.get(5).map(|v| PathBuf::from(v.as_str())),
                })
            }
            "query-latest" if values.len() == 4 => Ok(Self::QueryLatest {
                profile_path: PathBuf::from(&values[2]),
                filter_path: PathBuf::from(&values[3]),
            }),
            "query-advanced" if values.len() == 4 => Ok(Self::QueryAdvanced {
                profile_path: PathBuf::from(&values[2]),
                request_path: PathBuf::from(&values[3]),
            }),
            "evaluate-watchlist" if values.len() == 4 => Ok(Self::EvaluateWatchlist {
                profile_path: PathBuf::from(&values[2]),
                watchlist_path: PathBuf::from(&values[3]),
            }),
            "health" if values.len() == 3 => Ok(Self::Health {
                profile_path: PathBuf::from(&values[2]),
            }),
            "sweep-text" => Self::parse_sweep_text(&values[2..]),
            other => Err(OperatorError::Cli(format!(
                "unsupported or malformed command `{other}`"
            ))),
        }
    }

    /// `sweep-text [--exact] [--fuzzy 1|2] [--max-hits N] [--] <terms.txt> <file>...`. Options may come
    /// anywhere before `--`; after it, everything is a path.
    fn parse_sweep_text(args: &[String]) -> Result<Self, OperatorError> {
        let mut exact = false;
        let mut fuzzy = 0;
        let mut max_hits = crate::sweep::DEFAULT_MAX_HITS;
        let mut paths: Vec<PathBuf> = Vec::new();
        let mut options_over = false;
        let mut rest = args.iter();
        while let Some(arg) = rest.next() {
            if options_over || !arg.starts_with("--") {
                paths.push(PathBuf::from(arg));
                continue;
            }
            match arg.as_str() {
                "--" => options_over = true,
                "--exact" => exact = true,
                "--fuzzy" => {
                    let value = rest
                        .next()
                        .ok_or_else(|| OperatorError::Cli("--fuzzy needs 1 or 2".into()))?;
                    fuzzy = match value.as_str() {
                        "1" => 1,
                        "2" => 2,
                        _ => {
                            return Err(OperatorError::Cli(format!(
                                "--fuzzy needs 1 or 2, not `{value}`"
                            )))
                        }
                    };
                }
                "--max-hits" => {
                    let value = rest
                        .next()
                        .ok_or_else(|| OperatorError::Cli("--max-hits needs a number".into()))?;
                    max_hits = value.parse().map_err(|_| {
                        OperatorError::Cli(format!(
                            "--max-hits needs a whole number, not `{value}`"
                        ))
                    })?;
                }
                other => {
                    return Err(OperatorError::Cli(format!(
                        "unknown sweep-text option `{other}`"
                    )))
                }
            }
        }
        if paths.len() < 2 {
            return Err(OperatorError::Cli(
                "sweep-text needs a terms file and at least one file to search".into(),
            ));
        }
        let terms_path = paths.remove(0);
        Ok(Self::SweepText {
            terms_path,
            input_paths: paths,
            exact,
            fuzzy,
            max_hits,
        })
    }
}

pub fn run_cli_command(command: CliCommand) -> Result<OperatorResponse, OperatorError> {
    let now = Utc::now();
    match command {
        CliCommand::ValidateProfile { profile_path } => {
            let profile = EngineProfile::load_json_file(profile_path)?;
            let view = ViewJob {
                id: crate::types::ViewJobId::new(),
                kind: crate::view::ViewKind::DataCard,
                entities: Vec::new(),
                metrics: Vec::new(),
                time_range: crate::view::TimeRange::LatestOnly,
                detail_override: None,
                viewport: None,
            };
            Ok(OperatorResponse::Topology(TopologyViewMaterialization {
                view_id: view.id,
                nodes: profile
                    .nodes
                    .iter()
                    .map(|node| crate::materialize::TopologyNodeView {
                        entity_id: node.id,
                        label: node.label.clone(),
                        kind_label: format!("{:?}", node.kind),
                        position: node.position,
                        metrics: Vec::new(),
                    })
                    .collect(),
                edges: Vec::new(),
                boundaries: Vec::new(),
            }))
        }
        CliCommand::AnalyzeTopology {
            profile_path,
            view_path,
        } => {
            let profile = EngineProfile::load_json_file(profile_path)?;
            let exporter =
                SnapshotExporter::new(std::env::temp_dir().join("skeletrace-cli"), None)?;
            let mut api = OperatorApi::from_profile(&profile, exporter, now)?;
            let view: ViewJob = load_json(view_path)?;
            api.execute(OperatorRequest::AnalyzeTopology { view, now })
        }
        CliCommand::FenceCheck {
            profile_path,
            view_path,
            fences_path,
        } => {
            let profile = EngineProfile::load_json_file(profile_path)?;
            let exporter =
                SnapshotExporter::new(std::env::temp_dir().join("skeletrace-cli"), None)?;
            let mut api = OperatorApi::from_profile(&profile, exporter, now)?;
            let view: ViewJob = load_json(view_path)?;
            let geojson: serde_json::Value = load_json(fences_path)?;
            let fences = fences_from_geojson(&geojson)?;
            api.execute(OperatorRequest::FenceCheck { view, fences, now })
        }
        CliCommand::CoverArea {
            profile_path,
            view_path,
        } => {
            let profile = EngineProfile::load_json_file(profile_path)?;
            let exporter =
                SnapshotExporter::new(std::env::temp_dir().join("skeletrace-cli"), None)?;
            let mut api = OperatorApi::from_profile(&profile, exporter, now)?;
            let view: ViewJob = load_json(view_path)?;
            api.execute(OperatorRequest::CoverArea { view, now })
        }
        CliCommand::StrongestChain {
            profile_path,
            view_path,
            from,
            to,
        } => {
            let profile = EngineProfile::load_json_file(profile_path)?;
            let exporter =
                SnapshotExporter::new(std::env::temp_dir().join("skeletrace-cli"), None)?;
            let mut api = OperatorApi::from_profile(&profile, exporter, now)?;
            let view: ViewJob = load_json(view_path)?;
            api.execute(OperatorRequest::StrongestChain {
                view,
                from,
                to,
                now,
            })
        }
        CliCommand::Tick { profile_path } => {
            let profile = EngineProfile::load_json_file(profile_path)?;
            let exporter =
                SnapshotExporter::new(std::env::temp_dir().join("skeletrace-cli"), None)?;
            let mut api = OperatorApi::from_profile(&profile, exporter, now)?;
            api.execute(OperatorRequest::Tick { now })
        }
        CliCommand::PollSource {
            profile_path,
            source_id,
        } => {
            let profile = EngineProfile::load_json_file(profile_path)?;
            let exporter =
                SnapshotExporter::new(std::env::temp_dir().join("skeletrace-cli"), None)?;
            let mut api = OperatorApi::from_profile(&profile, exporter, now)?;
            let parsed = uuid::Uuid::parse_str(&source_id)
                .map_err(|err| OperatorError::Cli(err.to_string()))?;
            api.execute(OperatorRequest::PollSource {
                source_id: SourceId::from_uuid(parsed),
                now,
            })
        }
        CliCommand::MaterializeTopology {
            profile_path,
            view_path,
        } => {
            let profile = EngineProfile::load_json_file(profile_path)?;
            let exporter =
                SnapshotExporter::new(std::env::temp_dir().join("skeletrace-cli"), None)?;
            let mut api = OperatorApi::from_profile(&profile, exporter, now)?;
            let view: ViewJob = load_json(view_path)?;
            api.execute(OperatorRequest::MaterializeTopology { view, now })
        }
        CliCommand::MaterializeSparseGeo {
            profile_path,
            view_path,
        } => {
            let profile = EngineProfile::load_json_file(profile_path)?;
            let exporter =
                SnapshotExporter::new(std::env::temp_dir().join("skeletrace-cli"), None)?;
            let mut api = OperatorApi::from_profile(&profile, exporter, now)?;
            let view: ViewJob = load_json(view_path)?;
            api.execute(OperatorRequest::MaterializeSparseGeo { view, now })
        }
        CliCommand::ExportSnapshot {
            profile_path,
            job_path,
            output_dir,
            catalog_path,
        } => {
            let profile = EngineProfile::load_json_file(profile_path)?;
            let exporter = SnapshotExporter::new(output_dir, catalog_path)?;
            let mut api = OperatorApi::from_profile(&profile, exporter, now)?;
            let job: SnapshotExportJob = load_json(job_path)?;
            api.execute(OperatorRequest::ExportSnapshot { job, now })
        }
        CliCommand::QueryLatest {
            profile_path,
            filter_path,
        } => {
            let profile = EngineProfile::load_json_file(profile_path)?;
            let exporter =
                SnapshotExporter::new(std::env::temp_dir().join("skeletrace-cli"), None)?;
            let mut api = OperatorApi::from_profile(&profile, exporter, now)?;
            let filter: QueryFilter = load_json(filter_path)?;
            api.execute(OperatorRequest::QueryLatest { filter, now })
        }
        CliCommand::QueryAdvanced {
            profile_path,
            request_path,
        } => {
            let profile = EngineProfile::load_json_file(profile_path)?;
            let exporter =
                SnapshotExporter::new(std::env::temp_dir().join("skeletrace-cli"), None)?;
            let mut api = OperatorApi::from_profile(&profile, exporter, now)?;
            let saved: SavedQuery = load_json(request_path)?;
            saved.validate()?;
            api.execute(OperatorRequest::QueryAdvanced {
                request: saved.request,
                now,
            })
        }
        CliCommand::EvaluateWatchlist {
            profile_path,
            watchlist_path,
        } => {
            let profile = EngineProfile::load_json_file(profile_path)?;
            let exporter =
                SnapshotExporter::new(std::env::temp_dir().join("skeletrace-cli"), None)?;
            let mut api = OperatorApi::from_profile(&profile, exporter, now)?;
            let watchlist: Watchlist = load_json(watchlist_path)?;
            api.execute(OperatorRequest::EvaluateWatchlist { watchlist, now })
        }
        CliCommand::Health { profile_path } => {
            let profile = EngineProfile::load_json_file(profile_path)?;
            let exporter =
                SnapshotExporter::new(std::env::temp_dir().join("skeletrace-cli"), None)?;
            let mut api = OperatorApi::from_profile(&profile, exporter, now)?;
            api.execute(OperatorRequest::HealthReport { now })
        }
        CliCommand::SweepText {
            terms_path,
            input_paths,
            exact,
            fuzzy,
            max_hits,
        } => {
            let bytes = fs::read(&terms_path).map_err(|err| {
                OperatorError::Io(format!("cannot read {}: {err}", terms_path.display()))
            })?;
            let mut request = SweepRequest::new(parse_terms(&bytes)?, input_paths);
            request.exact = exact;
            request.fuzzy = fuzzy;
            request.max_hits = max_hits;
            Ok(OperatorResponse::Sweep(sweep_files(&request)?))
        }
    }
}

fn load_json<T: for<'de> Deserialize<'de>>(path: PathBuf) -> Result<T, OperatorError> {
    let bytes = fs::read(path).map_err(|err| OperatorError::Io(err.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|err| OperatorError::Serde(err.to_string()))
}
