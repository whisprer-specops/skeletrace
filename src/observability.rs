// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Lightweight structured observability for the local engine.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::types::{EntityId, MetricId, SourceId, Timestamp, ValidationError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventLevel {
    Trace,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventKind {
    SourceRegistered,
    SourcePollSuccess,
    SourcePollFailure,
    CacheEvicted,
    MetricRetentionTuned,
    WarmStorePruned,
    SnapshotExported,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineEvent {
    pub at: Timestamp,
    pub level: EventLevel,
    pub kind: EventKind,
    pub source_id: Option<SourceId>,
    pub entity_id: Option<EntityId>,
    pub metric_id: Option<MetricId>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct EventCounts {
    pub total: usize,
    pub trace: usize,
    pub info: usize,
    pub warn: usize,
    pub error: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct EventBufferConfig {
    pub max_events: usize,
    pub include_trace: bool,
}

impl Default for EventBufferConfig {
    fn default() -> Self {
        Self {
            max_events: 1024,
            include_trace: false,
        }
    }
}

impl EventBufferConfig {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.max_events == 0 {
            return Err(ValidationError::ZeroCapacity(
                "observability.max_events".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct EventBuffer {
    config: EventBufferConfig,
    events: VecDeque<EngineEvent>,
}

impl Default for EventBuffer {
    fn default() -> Self {
        Self::new(EventBufferConfig::default()).expect("default event buffer config is valid")
    }
}

impl EventBuffer {
    pub fn new(config: EventBufferConfig) -> Result<Self, ValidationError> {
        config.validate()?;
        Ok(Self {
            config,
            events: VecDeque::with_capacity(config.max_events),
        })
    }

    pub fn push(&mut self, event: EngineEvent) {
        if matches!(event.level, EventLevel::Trace) && !self.config.include_trace {
            return;
        }
        if self.events.len() >= self.config.max_events {
            self.events.pop_front();
        }
        self.events.push_back(event);
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    #[must_use]
    pub fn snapshot(&self, limit: usize) -> Vec<EngineEvent> {
        if limit == 0 {
            return Vec::new();
        }
        let take = self.events.len().min(limit);
        self.events
            .iter()
            .rev()
            .take(take)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    #[must_use]
    pub fn counts(&self) -> EventCounts {
        let mut counts = EventCounts::default();
        for event in &self.events {
            counts.total += 1;
            match event.level {
                EventLevel::Trace => counts.trace += 1,
                EventLevel::Info => counts.info += 1,
                EventLevel::Warn => counts.warn += 1,
                EventLevel::Error => counts.error += 1,
            }
        }
        counts
    }

    #[must_use]
    pub const fn config(&self) -> EventBufferConfig {
        self.config
    }

    pub fn reconfigure(&mut self, config: EventBufferConfig) -> Result<(), ValidationError> {
        config.validate()?;
        self.config = config;
        while self.events.len() > self.config.max_events {
            self.events.pop_front();
        }
        Ok(())
    }
}
