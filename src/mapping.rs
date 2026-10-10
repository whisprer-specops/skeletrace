// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Source-side mapping contracts.
//!
//! Adapters use these serializable configs to turn heterogeneous upstream
//! payloads into normalized samples without baking source-specific logic into
//! the engine core.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::metric::MetricValueType;
use crate::types::{EntityId, MetricId, Quality, SourceId, ValidationError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FeedField {
    Title,
    Link,
    Description,
    Guid,
    Author,
    CategoryFirst,
    PubDate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntityMapping {
    Static(EntityId),
    JsonPointer { pointer: String },
    FeedField(FeedField),
    FeedGuidOrLink,
}

impl EntityMapping {
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::JsonPointer { pointer } if pointer.trim().is_empty() => {
                Err(ValidationError::EmptyField("mapping.entity.pointer".into()))
            }
            _ => Ok(()),
        }
    }

    #[must_use]
    pub fn deterministic_entity_id(source_id: SourceId, discriminator: &str) -> EntityId {
        EntityId::from_uuid(Uuid::new_v5(&source_id.as_uuid(), discriminator.as_bytes()))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ValueSelector {
    JsonPointer { pointer: String },
    FeedField(FeedField),
    LiteralNumeric(f64),
    LiteralCode(String),
    LiteralFlag(bool),
}

impl ValueSelector {
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::JsonPointer { pointer } if pointer.trim().is_empty() => {
                Err(ValidationError::EmptyField("mapping.metric.pointer".into()))
            }
            Self::LiteralCode(value) if value.trim().is_empty() => Err(
                ValidationError::EmptyField("mapping.metric.literal_code".into()),
            ),
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricBinding {
    pub metric_id: MetricId,
    pub selector: ValueSelector,
    /// Optional coercion target. When omitted, adapters infer from payload shape.
    pub value_type: Option<MetricValueType>,
    /// If true, the adapter errors when the selector cannot be resolved.
    pub required: bool,
}

impl MetricBinding {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.selector.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceMappingConfig {
    pub entity_mapping: EntityMapping,
    pub metric_bindings: Vec<MetricBinding>,
    pub default_quality: Quality,
}

impl SourceMappingConfig {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.entity_mapping.validate()?;
        if self.metric_bindings.is_empty() {
            return Err(ValidationError::ZeroCapacity(
                "mapping.metric_bindings".into(),
            ));
        }
        for binding in &self.metric_bindings {
            binding.validate()?;
        }
        Ok(())
    }
}
