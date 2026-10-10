// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Metric dictionary and append-only sample records.

use std::time::Duration;

use chrono::TimeDelta;
use serde::{Deserialize, Serialize};

use crate::types::{EntityId, MetricId, Quality, SourceId, Timestamp, ValidationError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MetricValueType {
    Numeric,
    Code,
    Flag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InterpolationMethod {
    None,
    Linear,
    StepForward,
    StepBackward,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PollCadence {
    Fixed(Duration),
    EventDriven,
    Manual,
    Adaptive {
        min_interval: Duration,
        max_interval: Duration,
    },
}

impl PollCadence {
    pub fn validate(self) -> Result<(), ValidationError> {
        match self {
            Self::Fixed(interval) if interval.is_zero() => {
                Err(ValidationError::ZeroCapacity("poll_cadence.fixed".into()))
            }
            Self::Adaptive {
                min_interval,
                max_interval,
            } => {
                if min_interval.is_zero() {
                    return Err(ValidationError::ZeroCapacity(
                        "poll_cadence.adaptive.min_interval".into(),
                    ));
                }
                if max_interval < min_interval {
                    return Err(ValidationError::InvalidWindow {
                        start_field: "min_interval".into(),
                        end_field: "max_interval".into(),
                    });
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RetentionPolicy {
    pub hot_duration: Duration,
    pub warm_duration: Duration,
    pub store_on_change_only: bool,
    pub change_threshold: Option<f64>,
    pub relative_change_threshold: Option<f64>,
    pub max_silent_gap: Duration,
}

impl RetentionPolicy {
    pub fn validate(self, value_type: MetricValueType) -> Result<(), ValidationError> {
        if self.hot_duration.is_zero() {
            return Err(ValidationError::ZeroCapacity(
                "retention.hot_duration".into(),
            ));
        }
        if self.warm_duration < self.hot_duration {
            return Err(ValidationError::InvalidWindow {
                start_field: "hot_duration".into(),
                end_field: "warm_duration".into(),
            });
        }
        if self.store_on_change_only && self.max_silent_gap.is_zero() {
            return Err(ValidationError::ZeroCapacity(
                "retention.max_silent_gap".into(),
            ));
        }
        if value_type != MetricValueType::Numeric
            && (self.change_threshold.is_some() || self.relative_change_threshold.is_some())
        {
            return Err(ValidationError::Unsupported(
                "numeric thresholds are only valid for numeric metrics".into(),
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn should_store(self, previous: Option<&LatestValue>, incoming: &Sample) -> bool {
        if !self.store_on_change_only {
            return true;
        }

        let Some(previous) = previous else {
            return true;
        };

        let elapsed = incoming
            .ts_observed
            .signed_duration_since(previous.timestamp)
            .to_std()
            .ok();

        if let Some(elapsed) = elapsed {
            if elapsed >= self.max_silent_gap {
                return true;
            }
        }

        match (&previous.value, &incoming.value) {
            (SampleValue::Numeric(old), SampleValue::Numeric(new)) => {
                let abs_changed = (new - old).abs();
                let rel_changed = if old.abs() > f64::EPSILON {
                    abs_changed / old.abs()
                } else {
                    abs_changed
                };

                let abs_ok = self.change_threshold.is_some_and(|t| abs_changed >= t);
                let rel_ok = self
                    .relative_change_threshold
                    .is_some_and(|t| rel_changed >= t);

                abs_ok || rel_ok
            }
            (SampleValue::Code(old), SampleValue::Code(new)) => old != new,
            (SampleValue::Flag(old), SampleValue::Flag(new)) => old != new,
            (_, _) => previous.value != incoming.value,
        }
    }

    #[must_use]
    pub fn warm_retention_chrono(self) -> TimeDelta {
        TimeDelta::from_std(self.warm_duration).unwrap_or_else(|_| TimeDelta::days(36_500))
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct RetentionTuning {
    pub hot_duration: Option<Duration>,
    pub warm_duration: Option<Duration>,
    pub store_on_change_only: Option<bool>,
    pub change_threshold: Option<Option<f64>>,
    pub relative_change_threshold: Option<Option<f64>>,
    pub max_silent_gap: Option<Duration>,
}

impl RetentionTuning {
    pub fn apply_to(
        self,
        mut policy: RetentionPolicy,
        value_type: MetricValueType,
    ) -> Result<RetentionPolicy, ValidationError> {
        if let Some(value) = self.hot_duration {
            policy.hot_duration = value;
        }
        if let Some(value) = self.warm_duration {
            policy.warm_duration = value;
        }
        if let Some(value) = self.store_on_change_only {
            policy.store_on_change_only = value;
        }
        if let Some(value) = self.change_threshold {
            policy.change_threshold = value;
        }
        if let Some(value) = self.relative_change_threshold {
            policy.relative_change_threshold = value;
        }
        if let Some(value) = self.max_silent_gap {
            policy.max_silent_gap = value;
        }
        policy.validate(value_type)?;
        Ok(policy)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricRetentionReport {
    pub metric_id: MetricId,
    pub metric_name: String,
    pub hot_sample_count: usize,
    pub warm_sample_count: usize,
    pub hot_duration: Duration,
    pub warm_duration: Duration,
    pub store_on_change_only: bool,
    pub change_threshold: Option<f64>,
    pub relative_change_threshold: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricDefinition {
    pub id: MetricId,
    pub name: String,
    pub unit: String,
    pub value_type: MetricValueType,
    pub cadence: PollCadence,
    pub interpolation: InterpolationMethod,
    pub retention: RetentionPolicy,
    pub source_ids: Vec<SourceId>,
    pub show_in_popup: bool,
    /// Lower numbers appear earlier in popup cards.
    pub popup_priority: u16,
    pub description: String,
}

impl MetricDefinition {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.name.trim().is_empty() {
            return Err(ValidationError::EmptyField("metric.name".into()));
        }
        self.cadence.validate()?;
        self.retention.validate(self.value_type)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SampleValue {
    Numeric(f64),
    Code(String),
    Flag(bool),
    Missing,
}

impl SampleValue {
    #[must_use]
    pub fn value_type(&self) -> Option<MetricValueType> {
        match self {
            Self::Numeric(_) => Some(MetricValueType::Numeric),
            Self::Code(_) => Some(MetricValueType::Code),
            Self::Flag(_) => Some(MetricValueType::Flag),
            Self::Missing => None,
        }
    }

    #[must_use]
    pub fn approx_bytes(&self) -> usize {
        match self {
            Self::Numeric(_) => std::mem::size_of::<f64>(),
            Self::Flag(_) => std::mem::size_of::<bool>(),
            Self::Code(s) => s.len(),
            Self::Missing => 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    pub entity_id: EntityId,
    pub metric_id: MetricId,
    pub ts_observed: Timestamp,
    pub ts_ingested: Timestamp,
    pub value: SampleValue,
    pub quality: Quality,
    pub source_id: SourceId,
}

impl Sample {
    pub fn validate(&self, definition: &MetricDefinition) -> Result<(), ValidationError> {
        if self.ts_ingested < self.ts_observed {
            return Err(ValidationError::InvalidWindow {
                start_field: "ts_observed".into(),
                end_field: "ts_ingested".into(),
            });
        }

        match (&self.value, definition.value_type) {
            (SampleValue::Missing, _) => Ok(()),
            (SampleValue::Numeric(_), MetricValueType::Numeric)
            | (SampleValue::Code(_), MetricValueType::Code)
            | (SampleValue::Flag(_), MetricValueType::Flag) => Ok(()),
            _ => Err(ValidationError::InvalidState(
                "sample value type does not match metric definition".into(),
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatestValue {
    pub metric_id: MetricId,
    pub value: SampleValue,
    pub timestamp: Timestamp,
    pub quality: Quality,
    pub source_id: SourceId,
}

impl LatestValue {
    #[must_use]
    pub fn from_sample(sample: &Sample) -> Self {
        Self {
            metric_id: sample.metric_id,
            value: sample.value.clone(),
            timestamp: sample.ts_observed,
            quality: sample.quality,
            source_id: sample.source_id,
        }
    }

    #[must_use]
    pub fn approx_bytes(&self) -> usize {
        std::mem::size_of::<MetricId>()
            + self.value.approx_bytes()
            + std::mem::size_of::<Timestamp>()
            + std::mem::size_of::<Quality>()
            + std::mem::size_of::<SourceId>()
    }
}
