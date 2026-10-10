// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Source adapter contracts and a few built-in low-overhead adapters.
//!
//! Adapters are responsible for acquiring data and yielding normalized batches
//! that the engine can validate, store, and cache.

use std::any::Any;
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{BufRead, BufReader, Cursor};
use std::time::Duration;

use rss::{Channel, Item};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::entity::{Boundary, Edge, Node};
use crate::ingest::{AdapterKind, RawIngestRecord, SourceDefinition};
use crate::mapping::{EntityMapping, FeedField, SourceMappingConfig, ValueSelector};
use crate::metric::{MetricDefinition, MetricValueType, Sample, SampleValue};
use crate::transport::{http_get_text, HttpRequestProfile, TransportError};
use crate::types::{EntityId, MetricId, Quality, SourceId, Timestamp, ValidationError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdapterError {
    Validation(String),
    Io(String),
    Parse(String),
    Unsupported(String),
}

impl std::fmt::Display for AdapterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validation(msg) => write!(f, "adapter validation error: {msg}"),
            Self::Io(msg) => write!(f, "adapter I/O error: {msg}"),
            Self::Parse(msg) => write!(f, "adapter parse error: {msg}"),
            Self::Unsupported(msg) => write!(f, "adapter unsupported: {msg}"),
        }
    }
}

impl std::error::Error for AdapterError {}

impl From<ValidationError> for AdapterError {
    fn from(value: ValidationError) -> Self {
        Self::Validation(value.to_string())
    }
}

impl From<TransportError> for AdapterError {
    fn from(value: TransportError) -> Self {
        Self::Io(value.to_string())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SourcePull {
    pub raw_records: Vec<RawIngestRecord>,
    pub samples: Vec<Sample>,
    pub touched_entities: Vec<EntityId>,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub boundaries: Vec<Boundary>,
}

impl SourcePull {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.raw_records.is_empty()
            && self.samples.is_empty()
            && self.touched_entities.is_empty()
            && self.nodes.is_empty()
            && self.edges.is_empty()
            && self.boundaries.is_empty()
    }

    #[must_use]
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }

    #[must_use]
    pub fn raw_count(&self) -> usize {
        self.raw_records.len()
    }

    #[must_use]
    pub fn topology_count(&self) -> usize {
        self.nodes.len() + self.edges.len() + self.boundaries.len()
    }

    pub fn validate_against(
        &self,
        metrics: &HashMap<MetricId, MetricDefinition>,
    ) -> Result<(), ValidationError> {
        for sample in &self.samples {
            let definition = metrics.get(&sample.metric_id).ok_or_else(|| {
                ValidationError::InvalidReference(format!(
                    "sample references unknown metric {}",
                    sample.metric_id
                ))
            })?;
            sample.validate(definition)?;
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
        Ok(())
    }
}

pub trait SourceAdapter: Send {
    fn kind(&self) -> AdapterKind;
    fn pull(
        &mut self,
        source: &SourceDefinition,
        now: Timestamp,
    ) -> Result<SourcePull, AdapterError>;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// Simple FIFO adapter for tests, operator injection, and synthetic/manual feeds.
#[derive(Debug, Default)]
pub struct ManualPushAdapter {
    queue: VecDeque<SourcePull>,
}

impl ManualPushAdapter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
        }
    }

    pub fn push_batch(&mut self, batch: SourcePull) {
        self.queue.push_back(batch);
    }

    #[must_use]
    pub fn pending_batches(&self) -> usize {
        self.queue.len()
    }
}

impl SourceAdapter for ManualPushAdapter {
    fn kind(&self) -> AdapterKind {
        AdapterKind::Manual
    }

    fn pull(
        &mut self,
        _source: &SourceDefinition,
        _now: Timestamp,
    ) -> Result<SourcePull, AdapterError> {
        Ok(self.queue.pop_front().unwrap_or_default())
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Line-delimited JSON file adapter for pre-normalized sample ingestion.
///
/// Each line must deserialize to [`FileSampleRecord`]. The adapter remembers how
/// many lines it has already consumed per source, so repeated polls only ingest
/// newly appended lines.
#[derive(Debug, Default)]
pub struct NdjsonSampleFileAdapter {
    offsets_by_source: HashMap<SourceId, usize>,
}

impl NdjsonSampleFileAdapter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            offsets_by_source: HashMap::new(),
        }
    }
}

impl SourceAdapter for NdjsonSampleFileAdapter {
    fn kind(&self) -> AdapterKind {
        AdapterKind::FileImport
    }

    fn pull(
        &mut self,
        source: &SourceDefinition,
        now: Timestamp,
    ) -> Result<SourcePull, AdapterError> {
        let file = File::open(&source.endpoint)
            .map_err(|err| AdapterError::Io(format!("{}: {err}", source.endpoint)))?;
        let reader = BufReader::new(file);
        let already_seen = self.offsets_by_source.get(&source.id).copied().unwrap_or(0);

        let mut batch = SourcePull::default();
        let mut total_lines = 0usize;

        for (idx, line) in reader.lines().enumerate() {
            total_lines = idx + 1;
            if idx < already_seen {
                continue;
            }

            let line = line.map_err(|err| AdapterError::Io(err.to_string()))?;
            if line.trim().is_empty() {
                continue;
            }

            let record: FileSampleRecord = serde_json::from_str(&line)
                .map_err(|err| AdapterError::Parse(format!("line {}: {err}", idx + 1)))?;
            let raw_payload = record.raw_payload.clone().unwrap_or_else(|| {
                serde_json::json!({
                    "entity_id": record.entity_id,
                    "metric_id": record.metric_id,
                    "value": record.value.clone(),
                    "observed_at": record.ts_observed,
                })
            });

            batch.raw_records.push(RawIngestRecord {
                source_id: source.id,
                source_timestamp: Some(record.ts_observed),
                ingested_at: now,
                payload: raw_payload,
            });
            batch.samples.push(Sample {
                entity_id: record.entity_id,
                metric_id: record.metric_id,
                ts_observed: record.ts_observed,
                ts_ingested: record.ts_ingested.unwrap_or(now),
                value: record.value,
                quality: record.quality,
                source_id: source.id,
            });
            batch.touched_entities.push(record.entity_id);
        }

        self.offsets_by_source.insert(source.id, total_lines);
        Ok(batch)
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// HTTP GET adapter that maps JSON object/array responses into normalized samples.
#[derive(Debug, Clone)]
pub struct HttpJsonAdapter {
    mapping: SourceMappingConfig,
    request_profile: HttpRequestProfile,
}

impl HttpJsonAdapter {
    pub fn new(mapping: SourceMappingConfig, timeout: Duration) -> Result<Self, AdapterError> {
        Self::with_request_profile(mapping, HttpRequestProfile::direct(timeout))
    }

    pub fn with_request_profile(
        mapping: SourceMappingConfig,
        request_profile: HttpRequestProfile,
    ) -> Result<Self, AdapterError> {
        mapping.validate()?;
        request_profile.validate()?;
        Ok(Self {
            mapping,
            request_profile,
        })
    }
}

impl SourceAdapter for HttpJsonAdapter {
    fn kind(&self) -> AdapterKind {
        AdapterKind::HttpPoller
    }

    fn pull(
        &mut self,
        source: &SourceDefinition,
        now: Timestamp,
    ) -> Result<SourcePull, AdapterError> {
        let body = http_get_text(&source.endpoint, &self.request_profile)?;
        let payload: Value =
            serde_json::from_str(&body).map_err(|err| AdapterError::Parse(err.to_string()))?;

        let items: Vec<&Value> = match &payload {
            Value::Array(items) => items.iter().collect(),
            _ => vec![&payload],
        };

        let mut batch = SourcePull::default();
        for item in items {
            let entity_id = resolve_json_entity(source.id, &self.mapping.entity_mapping, item)?;
            batch.raw_records.push(RawIngestRecord {
                source_id: source.id,
                source_timestamp: None,
                ingested_at: now,
                payload: item.clone(),
            });

            let samples = build_samples_from_json(source.id, entity_id, item, &self.mapping, now)?;
            if !samples.is_empty() {
                batch.touched_entities.push(entity_id);
                batch.samples.extend(samples);
            }
        }
        Ok(batch)
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[derive(Debug, Clone)]
pub struct TorHttpJsonAdapter {
    inner: HttpJsonAdapter,
}

impl TorHttpJsonAdapter {
    pub fn new(mapping: SourceMappingConfig, timeout: Duration) -> Result<Self, AdapterError> {
        Self::with_request_profile(mapping, HttpRequestProfile::tor_default(timeout))
    }

    pub fn with_request_profile(
        mapping: SourceMappingConfig,
        mut request_profile: HttpRequestProfile,
    ) -> Result<Self, AdapterError> {
        request_profile.proxy_route = crate::transport::ProxyRoute::TorDefault;
        Ok(Self {
            inner: HttpJsonAdapter::with_request_profile(mapping, request_profile)?,
        })
    }
}

impl SourceAdapter for TorHttpJsonAdapter {
    fn kind(&self) -> AdapterKind {
        AdapterKind::TorHttpPoller
    }

    fn pull(
        &mut self,
        source: &SourceDefinition,
        now: Timestamp,
    ) -> Result<SourcePull, AdapterError> {
        self.inner.pull(source, now)
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// RSS/Atom-like feed adapter producing one entity per item, usually by GUID or link.
#[derive(Debug, Clone)]
pub struct FeedPollAdapter {
    mapping: SourceMappingConfig,
    request_profile: HttpRequestProfile,
}

impl FeedPollAdapter {
    pub fn new(mapping: SourceMappingConfig, timeout: Duration) -> Result<Self, AdapterError> {
        Self::with_request_profile(mapping, HttpRequestProfile::direct(timeout))
    }

    pub fn with_request_profile(
        mapping: SourceMappingConfig,
        request_profile: HttpRequestProfile,
    ) -> Result<Self, AdapterError> {
        mapping.validate()?;
        request_profile.validate()?;
        Ok(Self {
            mapping,
            request_profile,
        })
    }
}

impl SourceAdapter for FeedPollAdapter {
    fn kind(&self) -> AdapterKind {
        AdapterKind::FeedPoller
    }

    fn pull(
        &mut self,
        source: &SourceDefinition,
        now: Timestamp,
    ) -> Result<SourcePull, AdapterError> {
        let body = http_get_text(&source.endpoint, &self.request_profile)?;
        let channel = Channel::read_from(Cursor::new(body.into_bytes()))
            .map_err(|err| AdapterError::Parse(err.to_string()))?;

        let mut batch = SourcePull::default();
        for item in channel.items() {
            let entity_id = resolve_feed_entity(source.id, &self.mapping.entity_mapping, item)?;
            batch.raw_records.push(RawIngestRecord {
                source_id: source.id,
                source_timestamp: None,
                ingested_at: now,
                payload: feed_item_payload(item),
            });

            let samples = build_samples_from_feed(source.id, entity_id, item, &self.mapping, now)?;
            if !samples.is_empty() {
                batch.touched_entities.push(entity_id);
                batch.samples.extend(samples);
            }
        }

        Ok(batch)
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSampleRecord {
    pub entity_id: EntityId,
    pub metric_id: MetricId,
    pub ts_observed: Timestamp,
    pub ts_ingested: Option<Timestamp>,
    pub value: SampleValue,
    pub quality: Quality,
    pub raw_payload: Option<Value>,
}

fn build_samples_from_json(
    source_id: SourceId,
    entity_id: EntityId,
    item: &Value,
    mapping: &SourceMappingConfig,
    now: Timestamp,
) -> Result<Vec<Sample>, AdapterError> {
    let mut out = Vec::new();
    for binding in &mapping.metric_bindings {
        let sample_value = match &binding.selector {
            ValueSelector::LiteralNumeric(v) => {
                coerce_json_value(&Value::from(*v), binding.value_type)?
            }
            ValueSelector::LiteralCode(v) => {
                coerce_json_value(&Value::from(v.clone()), binding.value_type)?
            }
            ValueSelector::LiteralFlag(v) => {
                coerce_json_value(&Value::from(*v), binding.value_type)?
            }
            _ => {
                let Some(value) = resolve_json_selector(item, &binding.selector)? else {
                    if binding.required {
                        return Err(AdapterError::Parse(format!(
                            "required selector missing for metric {}",
                            binding.metric_id
                        )));
                    }
                    continue;
                };
                coerce_json_value(value, binding.value_type)?
            }
        };
        out.push(Sample {
            entity_id,
            metric_id: binding.metric_id,
            ts_observed: now,
            ts_ingested: now,
            value: sample_value,
            quality: mapping.default_quality,
            source_id,
        });
    }
    Ok(out)
}

fn build_samples_from_feed(
    source_id: SourceId,
    entity_id: EntityId,
    item: &Item,
    mapping: &SourceMappingConfig,
    now: Timestamp,
) -> Result<Vec<Sample>, AdapterError> {
    let mut out = Vec::new();
    for binding in &mapping.metric_bindings {
        let Some(value) = resolve_feed_selector(item, &binding.selector)? else {
            if binding.required {
                return Err(AdapterError::Parse(format!(
                    "required feed selector missing for metric {}",
                    binding.metric_id
                )));
            }
            continue;
        };
        let sample_value = coerce_string_value(&value, binding.value_type)?;
        out.push(Sample {
            entity_id,
            metric_id: binding.metric_id,
            ts_observed: now,
            ts_ingested: now,
            value: sample_value,
            quality: mapping.default_quality,
            source_id,
        });
    }
    Ok(out)
}

fn resolve_json_entity(
    source_id: SourceId,
    mapping: &EntityMapping,
    item: &Value,
) -> Result<EntityId, AdapterError> {
    match mapping {
        EntityMapping::Static(id) => Ok(*id),
        EntityMapping::JsonPointer { pointer } => {
            let value = item.pointer(pointer).ok_or_else(|| {
                AdapterError::Parse(format!("entity pointer `{pointer}` not present in payload"))
            })?;
            let discriminator = match value {
                Value::String(v) => v.clone(),
                Value::Number(v) => v.to_string(),
                Value::Bool(v) => v.to_string(),
                _ => {
                    return Err(AdapterError::Parse(format!(
                        "entity pointer `{pointer}` must resolve to string/number/bool"
                    )))
                }
            };
            Ok(EntityMapping::deterministic_entity_id(
                source_id,
                &discriminator,
            ))
        }
        EntityMapping::FeedField(_) | EntityMapping::FeedGuidOrLink => {
            Err(AdapterError::Unsupported(
                "feed entity mapping cannot be used by HTTP JSON adapter".into(),
            ))
        }
    }
}

fn resolve_feed_entity(
    source_id: SourceId,
    mapping: &EntityMapping,
    item: &Item,
) -> Result<EntityId, AdapterError> {
    match mapping {
        EntityMapping::Static(id) => Ok(*id),
        EntityMapping::FeedGuidOrLink => {
            let discriminator = item
                .guid()
                .map(|guid| guid.value().to_owned())
                .or_else(|| item.link().map(ToOwned::to_owned))
                .or_else(|| item.title().map(ToOwned::to_owned))
                .ok_or_else(|| {
                    AdapterError::Parse(
                        "feed item lacks guid/link/title for deterministic entity mapping".into(),
                    )
                })?;
            Ok(EntityMapping::deterministic_entity_id(
                source_id,
                &discriminator,
            ))
        }
        EntityMapping::FeedField(field) => {
            let discriminator = feed_field_string(item, *field).ok_or_else(|| {
                AdapterError::Parse(format!("feed field {:?} missing for entity mapping", field))
            })?;
            Ok(EntityMapping::deterministic_entity_id(
                source_id,
                &discriminator,
            ))
        }
        EntityMapping::JsonPointer { .. } => Err(AdapterError::Unsupported(
            "JSON pointer entity mapping cannot be used by feed adapter".into(),
        )),
    }
}

fn resolve_json_selector<'a>(
    item: &'a Value,
    selector: &ValueSelector,
) -> Result<Option<&'a Value>, AdapterError> {
    match selector {
        ValueSelector::JsonPointer { pointer } => Ok(item.pointer(pointer)),
        ValueSelector::LiteralNumeric(_)
        | ValueSelector::LiteralCode(_)
        | ValueSelector::LiteralFlag(_) => Ok(None),
        ValueSelector::FeedField(_) => Err(AdapterError::Unsupported(
            "feed selector cannot be used by HTTP JSON adapter".into(),
        )),
    }
}

fn resolve_feed_selector(
    item: &Item,
    selector: &ValueSelector,
) -> Result<Option<String>, AdapterError> {
    match selector {
        ValueSelector::FeedField(field) => Ok(feed_field_string(item, *field)),
        ValueSelector::LiteralNumeric(v) => Ok(Some(v.to_string())),
        ValueSelector::LiteralCode(v) => Ok(Some(v.clone())),
        ValueSelector::LiteralFlag(v) => Ok(Some(v.to_string())),
        ValueSelector::JsonPointer { .. } => Err(AdapterError::Unsupported(
            "JSON pointer selector cannot be used by feed adapter".into(),
        )),
    }
}

fn coerce_json_value(
    value: &Value,
    value_type: Option<MetricValueType>,
) -> Result<SampleValue, AdapterError> {
    match value_type {
        Some(MetricValueType::Numeric) => match value {
            Value::Number(v) => v
                .as_f64()
                .map(SampleValue::Numeric)
                .ok_or_else(|| AdapterError::Parse("numeric value out of range".into())),
            Value::String(v) => v
                .parse::<f64>()
                .map(SampleValue::Numeric)
                .map_err(|err| AdapterError::Parse(err.to_string())),
            Value::Bool(v) => Ok(SampleValue::Numeric(if *v { 1.0 } else { 0.0 })),
            Value::Null => Ok(SampleValue::Missing),
            other => Err(AdapterError::Parse(format!(
                "cannot coerce JSON value {other:?} to numeric"
            ))),
        },
        Some(MetricValueType::Code) => match value {
            Value::String(v) => Ok(SampleValue::Code(v.clone())),
            Value::Null => Ok(SampleValue::Missing),
            other => Ok(SampleValue::Code(other.to_string())),
        },
        Some(MetricValueType::Flag) => match value {
            Value::Bool(v) => Ok(SampleValue::Flag(*v)),
            Value::Number(v) => Ok(SampleValue::Flag(v.as_i64().unwrap_or_default() != 0)),
            Value::String(v) => match v.trim().to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" | "y" => Ok(SampleValue::Flag(true)),
                "false" | "0" | "no" | "n" => Ok(SampleValue::Flag(false)),
                _ => Err(AdapterError::Parse(format!(
                    "cannot coerce string `{v}` to bool"
                ))),
            },
            Value::Null => Ok(SampleValue::Missing),
            other => Err(AdapterError::Parse(format!(
                "cannot coerce JSON value {other:?} to bool"
            ))),
        },
        None => match value {
            Value::Number(v) => v
                .as_f64()
                .map(SampleValue::Numeric)
                .ok_or_else(|| AdapterError::Parse("numeric value out of range".into())),
            Value::String(v) => Ok(SampleValue::Code(v.clone())),
            Value::Bool(v) => Ok(SampleValue::Flag(*v)),
            Value::Null => Ok(SampleValue::Missing),
            other => Ok(SampleValue::Code(other.to_string())),
        },
    }
}

fn coerce_string_value(
    value: &str,
    value_type: Option<MetricValueType>,
) -> Result<SampleValue, AdapterError> {
    match value_type {
        Some(MetricValueType::Numeric) => value
            .parse::<f64>()
            .map(SampleValue::Numeric)
            .map_err(|err| AdapterError::Parse(err.to_string())),
        Some(MetricValueType::Flag) => match value.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "y" => Ok(SampleValue::Flag(true)),
            "false" | "0" | "no" | "n" => Ok(SampleValue::Flag(false)),
            _ => Err(AdapterError::Parse(format!(
                "cannot coerce string `{value}` to bool"
            ))),
        },
        Some(MetricValueType::Code) | None => Ok(SampleValue::Code(value.to_owned())),
    }
}

fn feed_field_string(item: &Item, field: FeedField) -> Option<String> {
    match field {
        FeedField::Title => item.title().map(ToOwned::to_owned),
        FeedField::Link => item.link().map(ToOwned::to_owned),
        FeedField::Description => item.description().map(ToOwned::to_owned),
        FeedField::Guid => item.guid().map(|guid| guid.value().to_owned()),
        FeedField::Author => item.author().map(ToOwned::to_owned),
        FeedField::CategoryFirst => item
            .categories()
            .first()
            .map(|category| category.name().to_owned()),
        FeedField::PubDate => item.pub_date().map(ToOwned::to_owned),
    }
}

fn feed_item_payload(item: &Item) -> Value {
    serde_json::json!({
        "title": item.title(),
        "link": item.link(),
        "description": item.description(),
        "guid": item.guid().map(|guid| guid.value()),
        "author": item.author(),
        "category_first": item.categories().first().map(|category| category.name()),
        "pub_date": item.pub_date(),
    })
}
