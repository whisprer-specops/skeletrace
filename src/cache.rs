// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Cache and lifecycle types.
//!
//! The hot cache holds only active / recent / visible entities. Everything
//! else belongs in warm storage, cold snapshots, or gets evicted entirely.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::metric::{LatestValue, Sample};
use crate::types::{EntityId, MetricId, Priority, Timestamp, ValidationError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum DetailTier {
    Skeleton,
    Active,
    Sampled,
    HighRes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EvictionPolicy {
    TTL,
    LRU,
    PriorityBased,
    Pinned,
}

/// A fixed-capacity circular buffer of recent samples for one metric
/// on one entity. When full, the oldest sample is overwritten.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RingBuffer {
    pub entity_id: EntityId,
    pub metric_id: MetricId,
    pub capacity: usize,
    /// Physical storage; logical order is oldest -> newest.
    pub samples: Vec<Sample>,
    pub cursor: usize,
}

impl RingBuffer {
    pub fn new(
        entity_id: EntityId,
        metric_id: MetricId,
        capacity: usize,
    ) -> Result<Self, ValidationError> {
        if capacity == 0 {
            return Err(ValidationError::ZeroCapacity("ring_buffer.capacity".into()));
        }
        Ok(Self {
            entity_id,
            metric_id,
            capacity,
            samples: Vec::with_capacity(capacity),
            cursor: 0,
        })
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn push(&mut self, sample: Sample) {
        if self.samples.len() < self.capacity {
            self.samples.push(sample);
            self.cursor = self.samples.len() % self.capacity;
        } else {
            self.samples[self.cursor] = sample;
            self.cursor = (self.cursor + 1) % self.capacity;
        }
    }

    #[must_use]
    pub fn ordered_samples(&self) -> Vec<&Sample> {
        if self.samples.len() < self.capacity || self.cursor == 0 {
            return self.samples.iter().collect();
        }

        let (head, tail) = self.samples.split_at(self.cursor);
        tail.iter().chain(head.iter()).collect()
    }

    #[must_use]
    pub fn latest(&self) -> Option<&Sample> {
        if self.samples.is_empty() {
            return None;
        }
        let newest = if self.samples.len() < self.capacity || self.cursor == 0 {
            self.samples.len() - 1
        } else {
            self.cursor - 1
        };
        self.samples.get(newest)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub entity_id: EntityId,
    pub detail_tier: DetailTier,
    pub latest_by_metric: Vec<LatestValue>,
    pub ring_buffers: Vec<RingBuffer>,
    pub last_accessed: Timestamp,
    pub ttl: Duration,
    pub eviction_policy: EvictionPolicy,
    pub priority: Priority,
    pub is_visible: bool,
    pub is_selected: bool,
    pub is_alerting: bool,
}

impl CacheEntry {
    pub fn new(
        entity_id: EntityId,
        detail_tier: DetailTier,
        ttl: Duration,
        last_accessed: Timestamp,
    ) -> Result<Self, ValidationError> {
        if ttl.is_zero() {
            return Err(ValidationError::ZeroCapacity("cache_entry.ttl".into()));
        }

        Ok(Self {
            entity_id,
            detail_tier,
            latest_by_metric: Vec::new(),
            ring_buffers: Vec::new(),
            last_accessed,
            ttl,
            eviction_policy: EvictionPolicy::TTL,
            priority: Priority::default(),
            is_visible: false,
            is_selected: false,
            is_alerting: false,
        })
    }

    pub fn upsert_latest(&mut self, latest: LatestValue) {
        if let Some(existing) = self
            .latest_by_metric
            .iter_mut()
            .find(|value| value.metric_id == latest.metric_id)
        {
            *existing = latest;
        } else {
            self.latest_by_metric.push(latest);
        }
    }

    #[must_use]
    pub fn latest(&self, metric_id: MetricId) -> Option<&LatestValue> {
        self.latest_by_metric
            .iter()
            .find(|value| value.metric_id == metric_id)
    }

    pub fn touch(&mut self, timestamp: Timestamp) {
        self.last_accessed = timestamp;
    }

    #[must_use]
    pub fn approx_hot_bytes(&self) -> usize {
        let latest_bytes: usize = self
            .latest_by_metric
            .iter()
            .map(LatestValue::approx_bytes)
            .sum();
        let ring_bytes: usize = self
            .ring_buffers
            .iter()
            .map(|buffer| {
                buffer
                    .samples
                    .iter()
                    .map(|sample| sample.value.approx_bytes())
                    .sum::<usize>()
            })
            .sum();

        latest_bytes + ring_bytes
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CacheBudget {
    pub max_active_entities: usize,
    pub max_total_ring_samples: usize,
    pub max_highres_entities: usize,
    /// Soft budget for hot memory in bytes.
    pub max_approx_hot_bytes: usize,
    /// Soft per-entity budget in bytes.
    pub max_per_entity_hot_bytes: usize,
}

impl CacheBudget {
    pub fn validate(self) -> Result<(), ValidationError> {
        if self.max_active_entities == 0 {
            return Err(ValidationError::ZeroCapacity(
                "cache_budget.max_active_entities".into(),
            ));
        }
        if self.max_total_ring_samples == 0 {
            return Err(ValidationError::ZeroCapacity(
                "cache_budget.max_total_ring_samples".into(),
            ));
        }
        if self.max_highres_entities == 0 {
            return Err(ValidationError::ZeroCapacity(
                "cache_budget.max_highres_entities".into(),
            ));
        }
        if self.max_approx_hot_bytes == 0 {
            return Err(ValidationError::ZeroCapacity(
                "cache_budget.max_approx_hot_bytes".into(),
            ));
        }
        if self.max_per_entity_hot_bytes == 0 {
            return Err(ValidationError::ZeroCapacity(
                "cache_budget.max_per_entity_hot_bytes".into(),
            ));
        }
        Ok(())
    }
}
