// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! # Skeletrace
//!
//! Sparse spatiotemporal flow-graph engine for OSINT telemetry.
//!
//! Skeletrace keeps the runtime truth as:
//! - identity-bearing entities and relations
//! - metric definitions and append-only samples
//! - a small hot cache of active / selected / visible state
//! - selectively generated view payloads (including GeoJSON)
//!
//! It does **not** keep a full world model in memory.
//!
//! Topology analysis identifies chokepoints and bridges in any materialized
//! topology view, and the strongest chain finds the most believable route
//! between two of its nodes. Geofences say which nodes fall inside an area, and
//! how much ground a set of nodes covers. Text sweeps find every term on a watch list in any number of
//! files, one pass per file, and can list the words a typo or two away from a term as well. External tools
//! can feed observations through source adapters without being linked into the engine.

pub mod adapter;
pub mod cache;
pub mod chain;
pub mod engine;
pub mod entity;
pub mod export;
pub mod geofence;
pub mod governance;
pub mod ingest;
pub mod lookyloo;
pub mod mapping;
pub mod materialize;
pub mod metric;
pub mod observability;
pub mod operator;
pub mod profile;
pub mod profiling;
pub mod query;
pub mod replay;
pub mod snapshot;
pub mod spatial;
pub mod store;
pub mod sweep;
pub mod topology_analysis;
pub mod transport;
pub mod types;
pub mod view;
pub mod warm_store;
pub mod workload;

pub use adapter::{
    AdapterError, FeedPollAdapter, FileSampleRecord, HttpJsonAdapter, ManualPushAdapter,
    NdjsonSampleFileAdapter, SourceAdapter, SourcePull, TorHttpJsonAdapter,
};
pub use cache::{CacheBudget, CacheEntry, DetailTier, EvictionPolicy, RingBuffer};
pub use engine::{EngineConfig, EngineError, SkeletraceEngine, TickReport};
pub use entity::{
    Boundary, BoundaryKind, Edge, EdgeDirection, EdgeKind, EntityStatus, Flow, FlowKind,
    GeometryMode, Node, NodeKind,
};
pub use ingest::{
    AdapterKind, RawIngestRecord, ScheduleEntry, SourceDefinition, SourceHealth, SourceKind,
    SourceSchedule,
};
pub use mapping::{EntityMapping, FeedField, MetricBinding, SourceMappingConfig, ValueSelector};
pub use materialize::{
    MaterializedMetricValue, SparseGeoFeature, SparseGeoFeatureCollection, SparseGeoGeometry,
    SparseGeoMaterializer, SparseGeoViewMaterialization, TopologyBoundaryView, TopologyEdgeView,
    TopologyMaterializer, TopologyNodeView, TopologyViewMaterialization,
};
pub use metric::{
    InterpolationMethod, LatestValue, MetricDefinition, MetricValueType, PollCadence,
    RetentionPolicy, Sample, SampleValue,
};
pub use snapshot::{ExportFormat, SnapshotManifest, SnapshotRequest};
pub use spatial::{CartesianCoord, Ellipsoid, GeoBBox, GeoCoord, WGS84};
pub use store::{BatchIngestReport, EngineStore, SampleIngestOutcome, StoreError, StoreStats};
pub use transport::{AuthConfig, HeaderPair, HttpRequestProfile, ProxyRoute, TransportError};
pub use types::{
    Confidence, EntityId, FlowId, MetricId, Priority, Quality, SnapshotId, SourceId, Tag,
    Timestamp, ValidationError, ViewJobId,
};
pub use view::{DataCard, DataCardField, RenderRule, TimeRange, ViewJob, ViewKind};
pub use warm_store::{SqliteWarmStore, WarmStoreError, WarmStoreMaintenanceReport};

pub use export::{
    ExportError, SnapshotExportJob, SnapshotExportResult, SnapshotExporter, SqliteSnapshotCatalog,
};
pub use operator::{
    run_cli_command, CliCommand, OperatorApi, OperatorError, OperatorRequest, OperatorResponse,
};
pub use profile::{AdapterProfile, EngineProfile, ProfileError, SourceProfile, SqliteProfileStore};

pub use metric::{MetricRetentionReport, RetentionTuning};
pub use observability::{
    EngineEvent, EventBuffer, EventBufferConfig, EventCounts, EventKind, EventLevel,
};
pub use profiling::{
    CacheHealthReport, DurationStats, EngineHealthReport, PerfProbeConfig, PerfProbeReport,
    SourceHealthCount,
};

pub use governance::{
    AuditRecord, AuditTrail, AutomationSupport, ExportAuditRecord, FailureClass, FailureRecord,
    PolicyVerdict, SampleAuditRecord, SampleProvenance, SourceAccessMode, SourceCapabilityProfile,
    SourceCostModel, SourcePolicy, TransformStep, TransportSupport,
};
pub use replay::{ReplayBatch, ReplayHarness};

pub use workload::{
    ReplayBenchmarkReport, ReplayBenchmarkRequest, ReplayIngestReport, ReplayWorkloadRequest,
    SourceWorkloadSummary, ViewProfileTarget, WorkloadCheckpointReport, WorkloadFixture,
    WorkloadRunReport,
};

pub use query::{
    AlertEvent, AlertRule, EntityClass, EntitySelector, NumericPredicate, QueryFilter,
    QueryGroupKey, QueryGroupSummary, QueryRequest, QueryResult, QueryResultEnvelope, QueryRow,
    QueryRowComparison, QuerySortKey, SavedQuery, WatchItem, Watchlist, WatchlistEvaluation,
};

pub use lookyloo::{
    LookylooCaptureSummary, LookylooMetricBindings, LookylooSourceConfig, LookylooSummaryAdapter,
    LookylooTopologyAdapter, LookylooTopologyConfig,
};

pub use topology_analysis::{analyze_topology, Bridge, Chokepoint, TopologyAnalysis};

pub use chain::{find_node, strongest_chain, ChainError, ChainLink, ChainNode, StrongestChain};
pub use geofence::{
    cover_area, fence_check, fences_from_geojson, CoverArea, FenceCheck, FenceError, FencePolygon,
    FenceResult, GeoFence, GeoNode,
};

pub use sweep::{
    parse_terms, sweep_files, SweepError, SweepHit, SweepInputSummary, SweepNearHit, SweepNearSkip,
    SweepReport, SweepRequest, SweepTermCount, Sweeper,
};
