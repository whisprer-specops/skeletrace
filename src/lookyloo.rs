// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Lookyloo-specific source adapter.
//!
//! This is a Rust-side source/plugin layer for ingesting Lookyloo capture-summary
//! payloads into Skeletrace without trying to reimplement the entire Lookyloo
//! application in Rust.
//!
//! The adapter targets the summary shape exposed by Lookyloo cache/export data:
//! - `uuid`
//! - `title`
//! - `timestamp`
//! - `url`
//! - `redirects`
//! - `error`
//! - `no_index`
//! - `categories`
//! - `parent`
//! - `user_agent`
//! - `referer`
//! - `capture_dir`
//!
//! The input payload may be:
//! - a single summary object
//! - an array of summary objects
//! - an object containing an array at a configured JSON pointer such as
//!   `/captures`.

use std::any::Any;
use std::time::Duration;

use chrono::{DateTime, FixedOffset, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::adapter::{AdapterError, SourceAdapter, SourcePull};
use crate::entity::{
    Boundary, BoundaryKind, Edge, EdgeDirection, EdgeKind, EntityStatus, GeometryMode, Node,
    NodeKind,
};
use crate::ingest::{AdapterKind, RawIngestRecord, SourceDefinition};
use crate::metric::{Sample, SampleValue};
use crate::transport::{http_get_text, HttpRequestProfile};
use crate::types::{
    Confidence, EntityId, MetricId, Priority, Quality, SourceId, Tag, Timestamp, ValidationError,
};

type TopologyBatch = (Vec<Node>, Vec<Edge>, Vec<Boundary>);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LookylooMetricBindings {
    pub title: Option<MetricId>,
    pub root_url: Option<MetricId>,
    pub final_url: Option<MetricId>,
    pub redirect_count: Option<MetricId>,
    pub has_error: Option<MetricId>,
    pub error_text: Option<MetricId>,
    pub no_index: Option<MetricId>,
    pub category_count: Option<MetricId>,
    pub categories_joined: Option<MetricId>,
    pub has_parent: Option<MetricId>,
    pub parent_capture: Option<MetricId>,
    pub user_agent: Option<MetricId>,
    pub referer: Option<MetricId>,
    pub capture_dir: Option<MetricId>,
}

impl LookylooMetricBindings {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.root_url.is_none()
            && self.final_url.is_none()
            && self.redirect_count.is_none()
            && self.has_error.is_none()
            && self.error_text.is_none()
            && self.no_index.is_none()
            && self.category_count.is_none()
            && self.categories_joined.is_none()
            && self.has_parent.is_none()
            && self.parent_capture.is_none()
            && self.user_agent.is_none()
            && self.referer.is_none()
            && self.capture_dir.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LookylooSourceConfig {
    /// Optional JSON pointer to the array/object containing capture summaries.
    /// Examples: `/captures`, `/response/items`.
    pub payload_root_pointer: Option<String>,
    /// Include the original upstream summary object as a raw ingest record.
    pub include_raw_payload: bool,
    pub default_quality: Quality,
    pub metrics: LookylooMetricBindings,
}

impl LookylooSourceConfig {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if let Some(pointer) = &self.payload_root_pointer {
            if pointer.trim().is_empty() {
                return Err(ValidationError::EmptyField(
                    "lookyloo.payload_root_pointer".into(),
                ));
            }
            if !pointer.starts_with('/') {
                return Err(ValidationError::InvalidState(
                    "lookyloo.payload_root_pointer must be a JSON pointer starting with '/'".into(),
                ));
            }
        }
        if self.metrics.is_empty() {
            return Err(ValidationError::ZeroCapacity("lookyloo.metrics".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LookylooTopologyConfig {
    /// Optional JSON pointer to the array/object containing capture summaries.
    pub payload_root_pointer: Option<String>,
    /// Include the original upstream summary object as a raw ingest record.
    pub include_raw_payload: bool,
    pub default_quality: Quality,
    /// Summary metrics are optional here: this adapter may be used for topology only.
    pub metrics: LookylooMetricBindings,
    pub include_capture_nodes: bool,
    pub include_domain_nodes: bool,
    pub include_capture_root_edge: bool,
    pub include_capture_final_edge: bool,
    pub include_redirect_chain_edges: bool,
    pub include_parent_capture_edges: bool,
    pub include_category_boundaries: bool,
    pub include_error_boundary: bool,
    pub include_no_index_boundary: bool,
    pub relationship_confidence: Confidence,
}

impl Default for LookylooTopologyConfig {
    fn default() -> Self {
        Self {
            payload_root_pointer: None,
            include_raw_payload: true,
            default_quality: Quality::default(),
            metrics: LookylooMetricBindings {
                title: None,
                root_url: None,
                final_url: None,
                redirect_count: None,
                has_error: None,
                error_text: None,
                no_index: None,
                category_count: None,
                categories_joined: None,
                has_parent: None,
                parent_capture: None,
                user_agent: None,
                referer: None,
                capture_dir: None,
            },
            include_capture_nodes: true,
            include_domain_nodes: true,
            include_capture_root_edge: true,
            include_capture_final_edge: true,
            include_redirect_chain_edges: true,
            include_parent_capture_edges: true,
            include_category_boundaries: true,
            include_error_boundary: true,
            include_no_index_boundary: true,
            relationship_confidence: Confidence::default(),
        }
    }
}

impl LookylooTopologyConfig {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if let Some(pointer) = &self.payload_root_pointer {
            if pointer.trim().is_empty() {
                return Err(ValidationError::EmptyField(
                    "lookyloo_topology.payload_root_pointer".into(),
                ));
            }
            if !pointer.starts_with('/') {
                return Err(ValidationError::InvalidState(
                    "lookyloo_topology.payload_root_pointer must be a JSON pointer starting with '/'"
                        .into(),
                ));
            }
        }

        let topology_enabled = self.include_capture_nodes
            || self.include_domain_nodes
            || self.include_capture_root_edge
            || self.include_capture_final_edge
            || self.include_redirect_chain_edges
            || self.include_parent_capture_edges
            || self.include_category_boundaries
            || self.include_error_boundary
            || self.include_no_index_boundary;
        if self.metrics.is_empty() && !topology_enabled {
            return Err(ValidationError::ZeroCapacity(
                "lookyloo_topology.outputs".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LookylooCaptureSummary {
    pub uuid: String,
    pub title: Option<String>,
    pub timestamp: Option<Timestamp>,
    pub url: Option<String>,
    pub redirects: Vec<String>,
    pub error: Option<String>,
    pub no_index: bool,
    pub categories: Vec<String>,
    pub parent: Option<String>,
    pub user_agent: Option<String>,
    pub referer: Option<String>,
    pub capture_dir: Option<String>,
}

impl LookylooCaptureSummary {
    pub fn from_value(value: &Value) -> Result<Self, AdapterError> {
        let object = value.as_object().ok_or_else(|| {
            AdapterError::Parse("lookyloo summary entry must be a JSON object".into())
        })?;

        let uuid = object
            .get("uuid")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .ok_or_else(|| AdapterError::Parse("lookyloo summary missing `uuid`".into()))?;

        Ok(Self {
            uuid,
            title: optional_string_field(object.get("title")),
            timestamp: match object.get("timestamp") {
                Some(value) => parse_timestamp_value(value)?,
                None => None,
            },
            url: optional_string_field(object.get("url")),
            redirects: parse_string_list_field(object.get("redirects"))?,
            error: optional_string_field(object.get("error")),
            no_index: parse_boolish_field(object.get("no_index"))?,
            categories: parse_string_list_field(object.get("categories"))?,
            parent: optional_string_field(object.get("parent")),
            user_agent: optional_string_field(object.get("user_agent")),
            referer: optional_string_field(object.get("referer")),
            capture_dir: optional_string_field(object.get("capture_dir")),
        })
    }

    #[must_use]
    pub fn entity_id(&self, source_id: SourceId) -> EntityId {
        if let Ok(uuid) = Uuid::parse_str(&self.uuid) {
            return EntityId::from_uuid(Uuid::new_v5(&source_id.as_uuid(), uuid.as_bytes()));
        }
        EntityId::from_uuid(Uuid::new_v5(&source_id.as_uuid(), self.uuid.as_bytes()))
    }

    #[must_use]
    pub fn observed_at(&self, now: Timestamp) -> Timestamp {
        self.timestamp.unwrap_or(now)
    }

    #[must_use]
    pub fn final_url(&self) -> Option<&str> {
        self.redirects
            .last()
            .map(String::as_str)
            .or(self.url.as_deref())
    }
}

#[derive(Debug, Clone)]
pub struct LookylooSummaryAdapter {
    config: LookylooSourceConfig,
    request_profile: HttpRequestProfile,
}

impl LookylooSummaryAdapter {
    pub fn new(config: LookylooSourceConfig, timeout: Duration) -> Result<Self, AdapterError> {
        Self::with_request_profile(config, HttpRequestProfile::direct(timeout))
    }

    pub fn with_request_profile(
        config: LookylooSourceConfig,
        request_profile: HttpRequestProfile,
    ) -> Result<Self, AdapterError> {
        config.validate()?;
        request_profile.validate()?;
        Ok(Self {
            config,
            request_profile,
        })
    }

    pub fn pull_from_text(
        &self,
        source: &SourceDefinition,
        now: Timestamp,
        body: &str,
    ) -> Result<SourcePull, AdapterError> {
        let payload: Value =
            serde_json::from_str(body).map_err(|err| AdapterError::Parse(err.to_string()))?;
        self.pull_from_value(source, now, payload)
    }

    pub fn pull_from_value(
        &self,
        source: &SourceDefinition,
        now: Timestamp,
        payload: Value,
    ) -> Result<SourcePull, AdapterError> {
        let items = lookyloo_payload_items(&payload, self.config.payload_root_pointer.as_deref())?;

        let mut batch = SourcePull::default();
        for item in items {
            let summary = LookylooCaptureSummary::from_value(item)?;
            let entity_id = summary.entity_id(source.id);
            let observed_at = summary.observed_at(now);

            if self.config.include_raw_payload {
                batch.raw_records.push(RawIngestRecord {
                    source_id: source.id,
                    source_timestamp: Some(observed_at),
                    ingested_at: now,
                    payload: item.clone(),
                });
            }

            let samples = build_samples_from_summary(
                source.id,
                entity_id,
                observed_at,
                now,
                &summary,
                &self.config,
            );
            if !samples.is_empty() {
                batch.touched_entities.push(entity_id);
                batch.samples.extend(samples);
            }
        }

        Ok(batch)
    }
}

impl SourceAdapter for LookylooSummaryAdapter {
    fn kind(&self) -> AdapterKind {
        AdapterKind::HttpPoller
    }

    fn pull(
        &mut self,
        source: &SourceDefinition,
        now: Timestamp,
    ) -> Result<SourcePull, AdapterError> {
        let body = http_get_text(&source.endpoint, &self.request_profile)?;
        self.pull_from_text(source, now, &body)
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[derive(Debug, Clone)]
pub struct LookylooTopologyAdapter {
    config: LookylooTopologyConfig,
    request_profile: HttpRequestProfile,
}

impl LookylooTopologyAdapter {
    pub fn new(config: LookylooTopologyConfig, timeout: Duration) -> Result<Self, AdapterError> {
        Self::with_request_profile(config, HttpRequestProfile::direct(timeout))
    }

    pub fn with_request_profile(
        config: LookylooTopologyConfig,
        request_profile: HttpRequestProfile,
    ) -> Result<Self, AdapterError> {
        config.validate()?;
        request_profile.validate()?;
        Ok(Self {
            config,
            request_profile,
        })
    }

    pub fn pull_from_text(
        &self,
        source: &SourceDefinition,
        now: Timestamp,
        body: &str,
    ) -> Result<SourcePull, AdapterError> {
        let payload: Value =
            serde_json::from_str(body).map_err(|err| AdapterError::Parse(err.to_string()))?;
        self.pull_from_value(source, now, payload)
    }

    pub fn pull_from_value(
        &self,
        source: &SourceDefinition,
        now: Timestamp,
        payload: Value,
    ) -> Result<SourcePull, AdapterError> {
        let items = lookyloo_payload_items(&payload, self.config.payload_root_pointer.as_deref())?;

        let mut batch = SourcePull::default();
        let mut seen_nodes = std::collections::HashSet::new();
        let mut seen_edges = std::collections::HashSet::new();
        let mut seen_boundaries = std::collections::HashSet::new();

        for item in items {
            let summary = LookylooCaptureSummary::from_value(item)?;
            let capture_id = summary.entity_id(source.id);
            let observed_at = summary.observed_at(now);

            if self.config.include_raw_payload {
                batch.raw_records.push(RawIngestRecord {
                    source_id: source.id,
                    source_timestamp: Some(observed_at),
                    ingested_at: now,
                    payload: item.clone(),
                });
            }

            let samples = build_samples_from_summary(
                source.id,
                capture_id,
                observed_at,
                now,
                &summary,
                &LookylooSourceConfig {
                    payload_root_pointer: self.config.payload_root_pointer.clone(),
                    include_raw_payload: self.config.include_raw_payload,
                    default_quality: self.config.default_quality,
                    metrics: self.config.metrics.clone(),
                },
            );
            if !samples.is_empty() {
                batch.samples.extend(samples);
            }

            let topology =
                build_topology_from_summary(source.id, observed_at, &summary, &self.config)?;
            for node in topology.0 {
                if seen_nodes.insert(node.id) {
                    batch.touched_entities.push(node.id);
                    batch.nodes.push(node);
                }
            }
            for edge in topology.1 {
                if seen_edges.insert(edge.id) {
                    batch.touched_entities.push(edge.id);
                    batch.edges.push(edge);
                }
            }
            for boundary in topology.2 {
                if seen_boundaries.insert(boundary.id) {
                    batch.touched_entities.push(boundary.id);
                    batch.boundaries.push(boundary);
                }
            }

            if !batch.touched_entities.contains(&capture_id) {
                batch.touched_entities.push(capture_id);
            }
        }

        Ok(batch)
    }
}

impl SourceAdapter for LookylooTopologyAdapter {
    fn kind(&self) -> AdapterKind {
        AdapterKind::HttpPoller
    }

    fn pull(
        &mut self,
        source: &SourceDefinition,
        now: Timestamp,
    ) -> Result<SourcePull, AdapterError> {
        let body = http_get_text(&source.endpoint, &self.request_profile)?;
        self.pull_from_text(source, now, &body)
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

fn lookyloo_payload_items<'a>(
    payload: &'a Value,
    pointer: Option<&str>,
) -> Result<Vec<&'a Value>, AdapterError> {
    let root = match pointer {
        Some(pointer) => payload.pointer(pointer).ok_or_else(|| {
            AdapterError::Parse(format!(
                "lookyloo payload root pointer `{pointer}` not present"
            ))
        })?,
        None => payload,
    };

    match root {
        Value::Array(items) => Ok(items.iter().collect()),
        Value::Object(map) => {
            if let Some(captures) = map.get("captures") {
                match captures {
                    Value::Array(items) => Ok(items.iter().collect()),
                    other => Ok(vec![other]),
                }
            } else {
                Ok(vec![root])
            }
        }
        _ => Err(AdapterError::Parse(
            "lookyloo payload root must be an object or array".into(),
        )),
    }
}

fn build_samples_from_summary(
    source_id: SourceId,
    entity_id: EntityId,
    observed_at: Timestamp,
    ingested_at: Timestamp,
    summary: &LookylooCaptureSummary,
    config: &LookylooSourceConfig,
) -> Vec<Sample> {
    let mut out = Vec::new();
    let bindings = &config.metrics;

    push_code_sample(
        &mut out,
        bindings.title,
        summary.title.as_deref(),
        source_id,
        entity_id,
        observed_at,
        ingested_at,
        config.default_quality,
    );
    push_code_sample(
        &mut out,
        bindings.root_url,
        summary.url.as_deref(),
        source_id,
        entity_id,
        observed_at,
        ingested_at,
        config.default_quality,
    );
    push_code_sample(
        &mut out,
        bindings.final_url,
        summary.final_url(),
        source_id,
        entity_id,
        observed_at,
        ingested_at,
        config.default_quality,
    );
    push_numeric_sample(
        &mut out,
        bindings.redirect_count,
        summary.redirects.len() as f64,
        source_id,
        entity_id,
        observed_at,
        ingested_at,
        config.default_quality,
    );
    push_flag_sample(
        &mut out,
        bindings.has_error,
        summary.error.is_some(),
        source_id,
        entity_id,
        observed_at,
        ingested_at,
        config.default_quality,
    );
    push_code_sample(
        &mut out,
        bindings.error_text,
        summary.error.as_deref(),
        source_id,
        entity_id,
        observed_at,
        ingested_at,
        config.default_quality,
    );
    push_flag_sample(
        &mut out,
        bindings.no_index,
        summary.no_index,
        source_id,
        entity_id,
        observed_at,
        ingested_at,
        config.default_quality,
    );
    push_numeric_sample(
        &mut out,
        bindings.category_count,
        summary.categories.len() as f64,
        source_id,
        entity_id,
        observed_at,
        ingested_at,
        config.default_quality,
    );
    let joined_categories = if summary.categories.is_empty() {
        None
    } else {
        Some(summary.categories.join("|"))
    };
    push_code_sample_owned(
        &mut out,
        bindings.categories_joined,
        joined_categories,
        source_id,
        entity_id,
        observed_at,
        ingested_at,
        config.default_quality,
    );
    push_flag_sample(
        &mut out,
        bindings.has_parent,
        summary.parent.is_some(),
        source_id,
        entity_id,
        observed_at,
        ingested_at,
        config.default_quality,
    );
    push_code_sample(
        &mut out,
        bindings.parent_capture,
        summary.parent.as_deref(),
        source_id,
        entity_id,
        observed_at,
        ingested_at,
        config.default_quality,
    );
    push_code_sample(
        &mut out,
        bindings.user_agent,
        summary.user_agent.as_deref(),
        source_id,
        entity_id,
        observed_at,
        ingested_at,
        config.default_quality,
    );
    push_code_sample(
        &mut out,
        bindings.referer,
        summary.referer.as_deref(),
        source_id,
        entity_id,
        observed_at,
        ingested_at,
        config.default_quality,
    );
    push_code_sample(
        &mut out,
        bindings.capture_dir,
        summary.capture_dir.as_deref(),
        source_id,
        entity_id,
        observed_at,
        ingested_at,
        config.default_quality,
    );

    out
}

#[allow(
    clippy::too_many_arguments,
    reason = "Per-sample helpers keep timestamps and provenance explicit"
)]
fn push_numeric_sample(
    out: &mut Vec<Sample>,
    metric_id: Option<MetricId>,
    value: f64,
    source_id: SourceId,
    entity_id: EntityId,
    observed_at: Timestamp,
    ingested_at: Timestamp,
    quality: Quality,
) {
    if let Some(metric_id) = metric_id {
        out.push(Sample {
            entity_id,
            metric_id,
            ts_observed: observed_at,
            ts_ingested: ingested_at,
            value: SampleValue::Numeric(value),
            quality,
            source_id,
        });
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "Per-sample helpers keep timestamps and provenance explicit"
)]
fn push_flag_sample(
    out: &mut Vec<Sample>,
    metric_id: Option<MetricId>,
    value: bool,
    source_id: SourceId,
    entity_id: EntityId,
    observed_at: Timestamp,
    ingested_at: Timestamp,
    quality: Quality,
) {
    if let Some(metric_id) = metric_id {
        out.push(Sample {
            entity_id,
            metric_id,
            ts_observed: observed_at,
            ts_ingested: ingested_at,
            value: SampleValue::Flag(value),
            quality,
            source_id,
        });
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "Per-sample helpers keep timestamps and provenance explicit"
)]
fn push_code_sample(
    out: &mut Vec<Sample>,
    metric_id: Option<MetricId>,
    value: Option<&str>,
    source_id: SourceId,
    entity_id: EntityId,
    observed_at: Timestamp,
    ingested_at: Timestamp,
    quality: Quality,
) {
    if let (Some(metric_id), Some(value)) = (metric_id, value) {
        out.push(Sample {
            entity_id,
            metric_id,
            ts_observed: observed_at,
            ts_ingested: ingested_at,
            value: SampleValue::Code(value.to_owned()),
            quality,
            source_id,
        });
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "Per-sample helpers keep timestamps and provenance explicit"
)]
fn push_code_sample_owned(
    out: &mut Vec<Sample>,
    metric_id: Option<MetricId>,
    value: Option<String>,
    source_id: SourceId,
    entity_id: EntityId,
    observed_at: Timestamp,
    ingested_at: Timestamp,
    quality: Quality,
) {
    if let (Some(metric_id), Some(value)) = (metric_id, value) {
        out.push(Sample {
            entity_id,
            metric_id,
            ts_observed: observed_at,
            ts_ingested: ingested_at,
            value: SampleValue::Code(value),
            quality,
            source_id,
        });
    }
}

fn optional_string_field(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(v)) => Some(v.clone()),
        Some(Value::Number(v)) => Some(v.to_string()),
        Some(Value::Bool(v)) => Some(v.to_string()),
        Some(Value::Null) | None => None,
        Some(other) => Some(other.to_string()),
    }
}

fn parse_string_list_field(value: Option<&Value>) -> Result<Vec<String>, AdapterError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };

    match value {
        Value::Null => Ok(Vec::new()),
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                let Some(text) = optional_string_field(Some(item)) else {
                    continue;
                };
                if !text.trim().is_empty() {
                    out.push(text);
                }
            }
            Ok(out)
        }
        Value::String(text) => parse_string_list_string(text),
        other => Ok(vec![other.to_string()]),
    }
}

fn parse_string_list_string(text: &str) -> Result<Vec<String>, AdapterError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if trimmed.starts_with('[') {
        let parsed: Value = serde_json::from_str(trimmed)
            .map_err(|err| AdapterError::Parse(format!("invalid JSON string list: {err}")))?;
        return parse_string_list_field(Some(&parsed));
    }
    Ok(vec![trimmed.to_owned()])
}

fn parse_boolish_field(value: Option<&Value>) -> Result<bool, AdapterError> {
    let Some(value) = value else {
        return Ok(false);
    };

    match value {
        Value::Bool(v) => Ok(*v),
        Value::Number(v) => Ok(v.as_i64().unwrap_or_default() != 0),
        Value::String(v) => match v.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "y" => Ok(true),
            "0" | "false" | "no" | "n" | "" => Ok(false),
            other => Err(AdapterError::Parse(format!(
                "cannot coerce lookyloo boolish field `{other}`"
            ))),
        },
        Value::Null => Ok(false),
        other => Err(AdapterError::Parse(format!(
            "cannot coerce lookyloo boolish field from {other:?}"
        ))),
    }
}

fn parse_timestamp_value(value: &Value) -> Result<Option<Timestamp>, AdapterError> {
    match value {
        Value::Null => Ok(None),
        Value::String(text) => parse_timestamp_string(text).map(Some),
        other => Err(AdapterError::Parse(format!(
            "lookyloo timestamp must be a string, got {other:?}"
        ))),
    }
}

fn parse_timestamp_string(text: &str) -> Result<Timestamp, AdapterError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(AdapterError::Parse(
            "lookyloo timestamp string must not be empty".into(),
        ));
    }

    if let Ok(parsed) = DateTime::parse_from_rfc3339(trimmed) {
        return Ok(parsed.with_timezone(&Utc));
    }

    // Python-style datetimes often serialize microseconds with a numeric timezone
    // like `2025-03-23T12:34:56.123456+0000`. Chrono's `%f` parser will accept
    // that, but interpret the fractional field as raw nanoseconds rather than as
    // six-digit microseconds. Normalize 1..=8 fractional digits to a 9-digit
    // nanosecond field before parsing so `.123456` becomes `123456000ns`.
    if let Some(normalized) = normalize_fractional_timezone_timestamp(trimmed) {
        if let Ok(parsed) = DateTime::parse_from_str(&normalized, "%Y-%m-%dT%H:%M:%S.%f%z") {
            return Ok(parsed.with_timezone(&Utc));
        }
    }

    if let Ok(parsed) = DateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S.%f%z") {
        return Ok(parsed.with_timezone(&Utc));
    }
    if let Ok(parsed) = DateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S%z") {
        return Ok(parsed.with_timezone(&Utc));
    }
    if let Ok(parsed) = DateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S%z") {
        return Ok(parsed.with_timezone(&Utc));
    }
    if let Ok(parsed) = DateTime::<FixedOffset>::parse_from_str(trimmed, "%a, %d %b %Y %H:%M:%S %z")
    {
        return Ok(parsed.with_timezone(&Utc));
    }

    Err(AdapterError::Parse(format!(
        "unsupported lookyloo timestamp `{trimmed}`"
    )))
}

fn normalize_fractional_timezone_timestamp(text: &str) -> Option<String> {
    let t_pos = text.find('T')?;
    let dot_rel = text[t_pos..].find('.')?;
    let dot_pos = t_pos + dot_rel;

    let tz_rel = text[dot_pos + 1..].find(['+', '-', 'Z'])?;
    let tz_pos = dot_pos + 1 + tz_rel;

    let fraction = &text[dot_pos + 1..tz_pos];
    if fraction.is_empty() || fraction.len() >= 9 || !fraction.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }

    let mut normalized = String::with_capacity(text.len() + (9 - fraction.len()));
    normalized.push_str(&text[..dot_pos + 1]);
    normalized.push_str(fraction);
    for _ in 0..(9 - fraction.len()) {
        normalized.push('0');
    }
    normalized.push_str(&text[tz_pos..]);
    Some(normalized)
}

fn build_topology_from_summary(
    source_id: SourceId,
    observed_at: Timestamp,
    summary: &LookylooCaptureSummary,
    config: &LookylooTopologyConfig,
) -> Result<TopologyBatch, AdapterError> {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut boundaries = Vec::new();
    let capture_id = summary.entity_id(source_id);

    if config.include_capture_nodes {
        nodes.push(build_capture_node(capture_id, observed_at, summary));
    }

    let mut domain_chain = Vec::new();
    let mut seen_domains = std::collections::HashSet::new();
    if let Some(url) = summary.url.as_deref() {
        if let Some(domain) = extract_domain(url) {
            if seen_domains.insert(domain.clone()) {
                domain_chain.push(domain);
            }
        }
    }
    for redirect in &summary.redirects {
        if let Some(domain) = extract_domain(redirect) {
            if seen_domains.insert(domain.clone()) {
                domain_chain.push(domain);
            }
        }
    }

    if config.include_domain_nodes {
        for domain in &domain_chain {
            nodes.push(build_domain_node(source_id, observed_at, domain));
        }
    }

    let root_domain = domain_chain.first().cloned();
    let final_domain = domain_chain.last().cloned();

    if config.include_capture_root_edge {
        if let Some(domain) = root_domain.as_deref() {
            let domain_id = domain_entity_id(source_id, domain);
            ensure_domain_node(
                &mut nodes,
                source_id,
                observed_at,
                domain_id,
                domain,
                config.include_domain_nodes,
            );
            edges.push(build_capture_domain_edge(
                source_id,
                capture_id,
                domain_id,
                observed_at,
                config.relationship_confidence,
                "root-domain",
            )?);
        }
    }
    if config.include_capture_final_edge {
        if let Some(domain) = final_domain.as_deref() {
            let domain_id = domain_entity_id(source_id, domain);
            ensure_domain_node(
                &mut nodes,
                source_id,
                observed_at,
                domain_id,
                domain,
                config.include_domain_nodes,
            );
            edges.push(build_capture_domain_edge(
                source_id,
                capture_id,
                domain_id,
                observed_at,
                config.relationship_confidence,
                "final-domain",
            )?);
        }
    }
    if config.include_redirect_chain_edges {
        for pair in domain_chain.windows(2) {
            let left = &pair[0];
            let right = &pair[1];
            if left == right {
                continue;
            }
            let left_id = domain_entity_id(source_id, left);
            let right_id = domain_entity_id(source_id, right);
            ensure_domain_node(
                &mut nodes,
                source_id,
                observed_at,
                left_id,
                left,
                config.include_domain_nodes,
            );
            ensure_domain_node(
                &mut nodes,
                source_id,
                observed_at,
                right_id,
                right,
                config.include_domain_nodes,
            );
            edges.push(build_domain_redirect_edge(
                source_id,
                left_id,
                right_id,
                observed_at,
                config.relationship_confidence,
                left,
                right,
            )?);
        }
    }
    if config.include_parent_capture_edges {
        if let Some(parent_uuid) = summary
            .parent
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            let parent_id = capture_entity_id_from_text(source_id, parent_uuid);
            ensure_capture_stub_node(
                &mut nodes,
                parent_id,
                observed_at,
                parent_uuid,
                config.include_capture_nodes,
            );
            edges.push(build_parent_capture_edge(
                source_id,
                parent_id,
                capture_id,
                observed_at,
                config.relationship_confidence,
            )?);
        }
    }

    if config.include_category_boundaries {
        for category in &summary.categories {
            let boundary = build_category_boundary(
                source_id,
                observed_at,
                capture_id,
                category,
                config.relationship_confidence,
            )?;
            boundaries.push(boundary);
        }
    }
    if config.include_no_index_boundary && summary.no_index {
        boundaries.push(build_boolean_boundary(
            source_id,
            observed_at,
            capture_id,
            "lookyloo-no-index",
            "Lookyloo No-Index",
            BoundaryKind::Visibility,
            config.relationship_confidence,
        )?);
    }
    if config.include_error_boundary && summary.error.is_some() {
        boundaries.push(build_boolean_boundary(
            source_id,
            observed_at,
            capture_id,
            "lookyloo-error",
            "Lookyloo Error",
            BoundaryKind::Other,
            config.relationship_confidence,
        )?);
    }

    Ok((nodes, edges, boundaries))
}

fn build_capture_node(
    capture_id: EntityId,
    observed_at: Timestamp,
    summary: &LookylooCaptureSummary,
) -> Node {
    let label = summary
        .title
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("capture:{}", summary.uuid));
    Node {
        id: capture_id,
        kind: NodeKind::Identity,
        label,
        position: None,
        position_confidence: Confidence::default(),
        status: EntityStatus::Active,
        priority: Priority::HIGH,
        tags: vec![
            Tag {
                key: "source".into(),
                value: "lookyloo".into(),
            },
            Tag {
                key: "role".into(),
                value: "capture".into(),
            },
            Tag {
                key: "capture_uuid".into(),
                value: summary.uuid.clone(),
            },
        ],
        first_seen: observed_at,
        last_seen: observed_at,
    }
}

fn build_domain_node(source_id: SourceId, observed_at: Timestamp, domain: &str) -> Node {
    Node {
        id: domain_entity_id(source_id, domain),
        kind: NodeKind::Endpoint,
        label: domain.to_owned(),
        position: None,
        position_confidence: Confidence::default(),
        status: EntityStatus::Active,
        priority: Priority::NORMAL,
        tags: vec![
            Tag {
                key: "source".into(),
                value: "lookyloo".into(),
            },
            Tag {
                key: "role".into(),
                value: "domain".into(),
            },
        ],
        first_seen: observed_at,
        last_seen: observed_at,
    }
}

fn ensure_domain_node(
    nodes: &mut Vec<Node>,
    source_id: SourceId,
    observed_at: Timestamp,
    domain_id: EntityId,
    domain: &str,
    include_domain_nodes: bool,
) {
    if !include_domain_nodes && nodes.iter().any(|candidate| candidate.id == domain_id) {
        return;
    }
    if !nodes.iter().any(|candidate| candidate.id == domain_id) {
        nodes.push(build_domain_node(source_id, observed_at, domain));
    }
}

fn ensure_capture_stub_node(
    nodes: &mut Vec<Node>,
    capture_id: EntityId,
    observed_at: Timestamp,
    capture_ref: &str,
    include_capture_nodes: bool,
) {
    if !include_capture_nodes && nodes.iter().any(|candidate| candidate.id == capture_id) {
        return;
    }
    if !nodes.iter().any(|candidate| candidate.id == capture_id) {
        nodes.push(Node {
            id: capture_id,
            kind: NodeKind::Identity,
            label: format!("capture:{}", capture_ref),
            position: None,
            position_confidence: Confidence::default(),
            status: EntityStatus::Active,
            priority: Priority::NORMAL,
            tags: vec![
                Tag {
                    key: "source".into(),
                    value: "lookyloo".into(),
                },
                Tag {
                    key: "role".into(),
                    value: "capture-parent".into(),
                },
            ],
            first_seen: observed_at,
            last_seen: observed_at,
        });
    }
}

fn build_capture_domain_edge(
    source_id: SourceId,
    capture_id: EntityId,
    domain_id: EntityId,
    observed_at: Timestamp,
    confidence: Confidence,
    relation: &str,
) -> Result<Edge, AdapterError> {
    if capture_id == domain_id {
        return Err(AdapterError::Parse(
            "capture/domain edge endpoints must differ".into(),
        ));
    }
    Ok(Edge {
        id: named_entity_id(
            source_id,
            &format!("capture-edge:{relation}:{capture_id}:{domain_id}"),
        ),
        kind: EdgeKind::Association,
        direction: EdgeDirection::Directed,
        source: capture_id,
        target: domain_id,
        geometry_mode: GeometryMode::Abstract,
        waypoints: Vec::new(),
        confidence,
        status: EntityStatus::Active,
        priority: Priority::HIGH,
        tags: vec![
            Tag {
                key: "source".into(),
                value: "lookyloo".into(),
            },
            Tag {
                key: "relation".into(),
                value: relation.to_owned(),
            },
        ],
        first_seen: observed_at,
        last_seen: observed_at,
    })
}

fn build_domain_redirect_edge(
    source_id: SourceId,
    left_id: EntityId,
    right_id: EntityId,
    observed_at: Timestamp,
    confidence: Confidence,
    left: &str,
    right: &str,
) -> Result<Edge, AdapterError> {
    if left_id == right_id {
        return Err(AdapterError::Parse(
            "redirect edge endpoints must differ".into(),
        ));
    }
    Ok(Edge {
        id: named_entity_id(source_id, &format!("redirect:{left}->{right}")),
        kind: EdgeKind::Route,
        direction: EdgeDirection::Directed,
        source: left_id,
        target: right_id,
        geometry_mode: GeometryMode::Abstract,
        waypoints: Vec::new(),
        confidence,
        status: EntityStatus::Active,
        priority: Priority::NORMAL,
        tags: vec![
            Tag {
                key: "source".into(),
                value: "lookyloo".into(),
            },
            Tag {
                key: "relation".into(),
                value: "redirect-chain".into(),
            },
        ],
        first_seen: observed_at,
        last_seen: observed_at,
    })
}

fn build_parent_capture_edge(
    source_id: SourceId,
    parent_id: EntityId,
    capture_id: EntityId,
    observed_at: Timestamp,
    confidence: Confidence,
) -> Result<Edge, AdapterError> {
    if parent_id == capture_id {
        return Err(AdapterError::Parse(
            "parent/capture edge endpoints must differ".into(),
        ));
    }
    Ok(Edge {
        id: named_entity_id(
            source_id,
            &format!("capture-parent:{parent_id}:{capture_id}"),
        ),
        kind: EdgeKind::Reference,
        direction: EdgeDirection::Directed,
        source: parent_id,
        target: capture_id,
        geometry_mode: GeometryMode::Abstract,
        waypoints: Vec::new(),
        confidence,
        status: EntityStatus::Active,
        priority: Priority::NORMAL,
        tags: vec![
            Tag {
                key: "source".into(),
                value: "lookyloo".into(),
            },
            Tag {
                key: "relation".into(),
                value: "parent-capture".into(),
            },
        ],
        first_seen: observed_at,
        last_seen: observed_at,
    })
}

fn build_category_boundary(
    source_id: SourceId,
    observed_at: Timestamp,
    capture_id: EntityId,
    category: &str,
    confidence: Confidence,
) -> Result<Boundary, AdapterError> {
    if category.trim().is_empty() {
        return Err(AdapterError::Parse(
            "lookyloo category boundary must not be empty".into(),
        ));
    }
    Ok(Boundary {
        id: named_entity_id(
            source_id,
            &format!("category:{}", category.trim().to_ascii_lowercase()),
        ),
        kind: BoundaryKind::Other,
        label: format!("Category: {}", category.trim()),
        extent: None,
        related_entities: vec![capture_id],
        confidence,
        status: EntityStatus::Active,
        priority: Priority::NORMAL,
        tags: vec![
            Tag {
                key: "source".into(),
                value: "lookyloo".into(),
            },
            Tag {
                key: "category".into(),
                value: category.trim().to_owned(),
            },
        ],
        first_seen: observed_at,
        last_seen: observed_at,
    })
}

fn build_boolean_boundary(
    source_id: SourceId,
    observed_at: Timestamp,
    capture_id: EntityId,
    key: &str,
    label: &str,
    kind: BoundaryKind,
    confidence: Confidence,
) -> Result<Boundary, AdapterError> {
    if key.trim().is_empty() {
        return Err(AdapterError::Parse(
            "lookyloo boundary key must not be empty".into(),
        ));
    }
    Ok(Boundary {
        id: named_entity_id(source_id, key),
        kind,
        label: label.to_owned(),
        extent: None,
        related_entities: vec![capture_id],
        confidence,
        status: EntityStatus::Active,
        priority: Priority::NORMAL,
        tags: vec![Tag {
            key: "source".into(),
            value: "lookyloo".into(),
        }],
        first_seen: observed_at,
        last_seen: observed_at,
    })
}

fn domain_entity_id(source_id: SourceId, domain: &str) -> EntityId {
    named_entity_id(
        source_id,
        &format!("domain:{}", domain.trim().to_ascii_lowercase()),
    )
}

fn capture_entity_id_from_text(source_id: SourceId, capture_ref: &str) -> EntityId {
    if let Ok(uuid) = Uuid::parse_str(capture_ref.trim()) {
        return EntityId::from_uuid(Uuid::new_v5(&source_id.as_uuid(), uuid.as_bytes()));
    }
    named_entity_id(source_id, &format!("capture:{}", capture_ref.trim()))
}

fn named_entity_id(source_id: SourceId, name: &str) -> EntityId {
    EntityId::from_uuid(Uuid::new_v5(&source_id.as_uuid(), name.as_bytes()))
}

fn extract_domain(url: &str) -> Option<String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }
    let without_scheme = if let Some(pos) = trimmed.find("://") {
        &trimmed[pos + 3..]
    } else {
        trimmed
    };
    let host_port = without_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .trim();
    let host = if let Some(idx) = host_port.rfind('@') {
        &host_port[idx + 1..]
    } else {
        host_port
    };
    let host = if host.starts_with('[') {
        host.split(']')
            .next()
            .unwrap_or(host)
            .trim_start_matches('[')
    } else {
        host.split(':').next().unwrap_or(host)
    };
    let host = host.trim().trim_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        None
    } else {
        Some(host)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Timelike};

    use crate::ingest::{SourceHealth, SourceKind, SourceSchedule};
    use crate::types::{Quality, SourceId, Tag};

    fn test_source() -> SourceDefinition {
        SourceDefinition {
            id: SourceId::new(),
            name: "lookyloo-test".into(),
            kind: SourceKind::Api,
            adapter: AdapterKind::HttpPoller,
            schedule: SourceSchedule::Manual,
            endpoint: "https://example.invalid/lookyloo".into(),
            auth_ref: None,
            health: SourceHealth::Pending,
            last_polled: None,
            last_error: None,
            backoff: Duration::from_secs(5),
            max_backoff: Duration::from_secs(30),
            tags: vec![Tag::new("source", "lookyloo").unwrap()],
        }
    }

    fn test_config() -> LookylooSourceConfig {
        LookylooSourceConfig {
            payload_root_pointer: None,
            include_raw_payload: true,
            default_quality: Quality::new(0.9).unwrap(),
            metrics: LookylooMetricBindings {
                title: Some(MetricId::new()),
                root_url: Some(MetricId::new()),
                final_url: Some(MetricId::new()),
                redirect_count: Some(MetricId::new()),
                has_error: Some(MetricId::new()),
                error_text: Some(MetricId::new()),
                no_index: Some(MetricId::new()),
                category_count: Some(MetricId::new()),
                categories_joined: Some(MetricId::new()),
                has_parent: Some(MetricId::new()),
                parent_capture: Some(MetricId::new()),
                user_agent: Some(MetricId::new()),
                referer: Some(MetricId::new()),
                capture_dir: Some(MetricId::new()),
            },
        }
    }

    #[test]
    fn parse_lookyloo_summary_with_python_style_serialized_fields() {
        let value = serde_json::json!({
            "uuid": "8e8cc8dd-fc9e-4717-a7cb-a8b76f7bb6e5",
            "title": "Example Title",
            "timestamp": "2025-03-23T12:34:56.123456+0000",
            "url": "https://example.com",
            "redirects": "[\"https://redirect-1.example\",\"https://redirect-2.example\"]",
            "error": "boom",
            "no_index": 1,
            "categories": "[\"phish\",\"kit\"]",
            "parent": "parent-uuid",
            "user_agent": "Mozilla/5.0",
            "referer": "https://ref.example",
            "capture_dir": "/captures/one"
        });

        let summary = LookylooCaptureSummary::from_value(&value).unwrap();
        assert_eq!(summary.redirects.len(), 2);
        assert_eq!(summary.categories.len(), 2);
        assert!(summary.no_index);
        assert_eq!(summary.final_url(), Some("https://redirect-2.example"));
        assert_eq!(
            summary.timestamp.unwrap(),
            Utc.with_ymd_and_hms(2025, 3, 23, 12, 34, 56)
                .unwrap()
                .with_nanosecond(123_456_000)
                .unwrap()
        );
    }

    #[test]
    fn adapter_builds_samples_from_wrapper_payload() {
        let mut config = test_config();
        config.payload_root_pointer = Some("/captures".into());
        let adapter = LookylooSummaryAdapter::with_request_profile(
            config,
            HttpRequestProfile::direct(Duration::from_secs(5)),
        )
        .unwrap();
        let source = test_source();
        let now = Utc.with_ymd_and_hms(2025, 3, 23, 13, 0, 0).unwrap();
        let body = serde_json::json!({
            "captures": [
                {
                    "uuid": "capture-a",
                    "title": "A",
                    "timestamp": "2025-03-23T12:00:00+0000",
                    "url": "https://example-a.test",
                    "redirects": ["https://example-a.test/landing"],
                    "categories": ["news"],
                    "no_index": false
                },
                {
                    "uuid": "capture-b",
                    "url": "https://example-b.test",
                    "error": "network",
                    "no_index": true,
                    "categories": []
                }
            ]
        })
        .to_string();

        let batch = adapter.pull_from_text(&source, now, &body).unwrap();
        assert_eq!(batch.raw_records.len(), 2);
        assert_eq!(batch.touched_entities.len(), 2);
        assert!(batch.samples.len() >= 10);
        assert!(batch
            .samples
            .iter()
            .any(|sample| matches!(sample.value, SampleValue::Flag(true))));
        assert!(batch
            .samples
            .iter()
            .any(|sample| matches!(&sample.value, SampleValue::Code(code) if code == "https://example-a.test/landing")));
    }

    #[test]
    fn topology_adapter_materializes_capture_domain_relationships() {
        let config = LookylooTopologyConfig {
            payload_root_pointer: Some("/captures".into()),
            include_raw_payload: true,
            default_quality: Quality::new(0.8).unwrap(),
            metrics: LookylooMetricBindings {
                title: Some(MetricId::new()),
                root_url: None,
                final_url: None,
                redirect_count: Some(MetricId::new()),
                has_error: None,
                error_text: None,
                no_index: None,
                category_count: None,
                categories_joined: None,
                has_parent: None,
                parent_capture: None,
                user_agent: None,
                referer: None,
                capture_dir: None,
            },
            include_capture_nodes: true,
            include_domain_nodes: true,
            include_capture_root_edge: true,
            include_capture_final_edge: true,
            include_redirect_chain_edges: true,
            include_parent_capture_edges: true,
            include_category_boundaries: true,
            include_error_boundary: true,
            include_no_index_boundary: true,
            relationship_confidence: Confidence::new(0.85).unwrap(),
        };
        let adapter = LookylooTopologyAdapter::with_request_profile(
            config,
            HttpRequestProfile::direct(Duration::from_secs(5)),
        )
        .unwrap();
        let source = test_source();
        let now = Utc.with_ymd_and_hms(2025, 3, 23, 13, 0, 0).unwrap();
        let body = serde_json::json!({
            "captures": [
                {
                    "uuid": "capture-a",
                    "title": "A",
                    "timestamp": "2025-03-23T12:00:00+0000",
                    "url": "https://root.example.test/path",
                    "redirects": [
                        "https://middle.example.test/landing",
                        "https://final.example.test/final"
                    ],
                    "categories": ["phish"],
                    "parent": "parent-capture",
                    "error": "dns",
                    "no_index": true
                }
            ]
        })
        .to_string();

        let batch = adapter.pull_from_text(&source, now, &body).unwrap();
        assert_eq!(batch.raw_records.len(), 1);
        assert!(!batch.nodes.is_empty());
        assert!(!batch.edges.is_empty());
        assert!(!batch.boundaries.is_empty());
        assert!(batch
            .nodes
            .iter()
            .any(|node| node.label == "root.example.test"));
        assert!(batch
            .nodes
            .iter()
            .any(|node| node.label == "middle.example.test"));
        assert!(batch
            .nodes
            .iter()
            .any(|node| node.label == "final.example.test"));
        assert!(batch.edges.iter().any(|edge| edge
            .tags
            .iter()
            .any(|tag| tag.key == "relation" && tag.value == "redirect-chain")));
        assert!(batch.edges.iter().any(|edge| edge
            .tags
            .iter()
            .any(|tag| tag.key == "relation" && tag.value == "parent-capture")));
        assert!(batch
            .boundaries
            .iter()
            .any(|boundary| boundary.label == "Category: phish"));
        assert!(batch
            .boundaries
            .iter()
            .any(|boundary| boundary.label == "Lookyloo No-Index"));
        assert!(batch
            .boundaries
            .iter()
            .any(|boundary| boundary.label == "Lookyloo Error"));
    }
}
