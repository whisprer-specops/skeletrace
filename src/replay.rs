// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Replay/fixture harness for deterministic engine testing without live sources.

use std::collections::{HashMap, VecDeque};

use serde::{Deserialize, Serialize};

use crate::adapter::SourcePull;
use crate::types::{SourceId, Timestamp};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayBatch {
    pub source_id: SourceId,
    pub due_at: Timestamp,
    pub pull: SourcePull,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReplayHarness {
    batches_by_source: HashMap<SourceId, VecDeque<ReplayBatch>>,
}

impl ReplayHarness {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_batch(&mut self, batch: ReplayBatch) {
        self.batches_by_source
            .entry(batch.source_id)
            .or_default()
            .push_back(batch);
    }

    #[must_use]
    pub fn pending_for(&self, source_id: SourceId) -> usize {
        self.batches_by_source
            .get(&source_id)
            .map_or(0, VecDeque::len)
    }

    #[must_use]
    pub fn total_pending(&self) -> usize {
        self.batches_by_source.values().map(VecDeque::len).sum()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.batches_by_source.is_empty()
    }
    pub fn drain_ready_for(&mut self, source_id: SourceId, now: Timestamp) -> Vec<SourcePull> {
        let Some(queue) = self.batches_by_source.get_mut(&source_id) else {
            return Vec::new();
        };
        let mut ready = Vec::new();
        while queue.front().is_some_and(|batch| batch.due_at <= now) {
            if let Some(batch) = queue.pop_front() {
                ready.push(batch.pull);
            }
        }
        if queue.is_empty() {
            self.batches_by_source.remove(&source_id);
        }
        ready
    }

    pub fn drain_all_ready(&mut self, now: Timestamp) -> Vec<(SourceId, SourcePull)> {
        let mut source_ids: Vec<SourceId> = self.batches_by_source.keys().copied().collect();
        source_ids.sort_unstable();
        let mut out = Vec::new();
        for source_id in source_ids {
            for pull in self.drain_ready_for(source_id, now) {
                out.push((source_id, pull));
            }
        }
        out
    }
}
