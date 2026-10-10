// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Ingest definitions — adapters, scheduling, and raw record capture.

use std::time::Duration;

use chrono::TimeDelta;
use serde::{Deserialize, Serialize};

use crate::types::{SourceId, Tag, Timestamp, ValidationError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SourceKind {
    Database,
    Stream,
    Api,
    File,
    Manual,
    Scraper,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AdapterKind {
    HttpPoller,
    FeedPoller,
    StreamAdapter,
    TorHttpPoller,
    FileImport,
    DatabaseQuery,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SourceSchedule {
    Fixed(Duration),
    EventDriven,
    Manual,
}

impl SourceSchedule {
    #[must_use]
    pub const fn is_automatic(self) -> bool {
        matches!(self, Self::Fixed(_))
    }

    pub fn validate(self) -> Result<(), ValidationError> {
        match self {
            Self::Fixed(interval) if interval.is_zero() => Err(ValidationError::ZeroCapacity(
                "source_schedule.fixed".into(),
            )),
            _ => Ok(()),
        }
    }

    #[must_use]
    pub fn next_due_after(self, now: Timestamp, jitter: Duration) -> Option<Timestamp> {
        match self {
            Self::Fixed(interval) => add_duration(now, interval.saturating_add(jitter)),
            Self::EventDriven | Self::Manual => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SourceHealth {
    Healthy,
    Degraded,
    Unreachable,
    Disabled,
    Pending,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceDefinition {
    pub id: SourceId,
    pub name: String,
    pub kind: SourceKind,
    pub adapter: AdapterKind,
    pub schedule: SourceSchedule,
    pub endpoint: String,
    pub auth_ref: Option<String>,
    pub health: SourceHealth,
    pub last_polled: Option<Timestamp>,
    pub last_error: Option<Timestamp>,
    pub backoff: Duration,
    pub max_backoff: Duration,
    pub tags: Vec<Tag>,
}

impl SourceDefinition {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.name.trim().is_empty() {
            return Err(ValidationError::EmptyField("source.name".into()));
        }
        if self.endpoint.trim().is_empty() && !matches!(self.kind, SourceKind::Manual) {
            return Err(ValidationError::EmptyField("source.endpoint".into()));
        }
        self.schedule.validate()?;
        if self.max_backoff < self.backoff {
            return Err(ValidationError::InvalidWindow {
                start_field: "backoff".into(),
                end_field: "max_backoff".into(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleEntry {
    pub source_id: SourceId,
    pub next_due: Timestamp,
    pub priority: u8,
    pub jitter: Duration,
    pub consecutive_failures: u32,
}

impl ScheduleEntry {
    pub fn new(
        source_id: SourceId,
        schedule: SourceSchedule,
        now: Timestamp,
        priority: u8,
        jitter: Duration,
    ) -> Result<Self, ValidationError> {
        let next_due = schedule.next_due_after(now, jitter).unwrap_or(now);
        Ok(Self {
            source_id,
            next_due,
            priority,
            jitter,
            consecutive_failures: 0,
        })
    }

    #[must_use]
    pub fn is_due(&self, now: Timestamp) -> bool {
        self.next_due <= now
    }

    pub fn mark_success(
        &mut self,
        schedule: SourceSchedule,
        now: Timestamp,
    ) -> Result<(), ValidationError> {
        self.consecutive_failures = 0;
        self.next_due = schedule.next_due_after(now, self.jitter).unwrap_or(now);
        Ok(())
    }

    pub fn mark_failure(
        &mut self,
        base_backoff: Duration,
        max_backoff: Duration,
        now: Timestamp,
    ) -> Result<(), ValidationError> {
        if base_backoff.is_zero() {
            return Err(ValidationError::ZeroCapacity("source.backoff".into()));
        }
        if max_backoff < base_backoff {
            return Err(ValidationError::InvalidWindow {
                start_field: "source.backoff".into(),
                end_field: "source.max_backoff".into(),
            });
        }

        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        let multiplier = 1u32
            .checked_shl(self.consecutive_failures.saturating_sub(1).min(30))
            .unwrap_or(u32::MAX);
        let delay = base_backoff
            .checked_mul(multiplier)
            .unwrap_or(max_backoff)
            .min(max_backoff)
            .saturating_add(self.jitter);
        self.next_due = add_duration(now, delay).unwrap_or(now);
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawIngestRecord {
    pub source_id: SourceId,
    pub source_timestamp: Option<Timestamp>,
    pub ingested_at: Timestamp,
    pub payload: serde_json::Value,
}

fn add_duration(now: Timestamp, duration: Duration) -> Option<Timestamp> {
    TimeDelta::from_std(duration).ok().map(|delta| now + delta)
}
