// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Source-agnostic governance: capability descriptors, policy gates,
//! provenance/audit trail, and failure taxonomy.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::adapter::AdapterError;
use crate::ingest::{AdapterKind, SourceDefinition, SourceSchedule};
use crate::snapshot::ExportFormat;
use crate::types::{EntityId, MetricId, SnapshotId, SourceId, Timestamp, ValidationError};
use crate::view::ViewKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SourceAccessMode {
    Public,
    Authenticated,
    Restricted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SourceCostModel {
    Free,
    Paid,
    Metered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AutomationSupport {
    AutomaticPolling,
    EventDrivenOnly,
    ManualOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TransportSupport {
    DirectOnly,
    DirectOrTor,
    TorOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceCapabilityProfile {
    pub access_mode: SourceAccessMode,
    pub cost_model: SourceCostModel,
    pub automation_support: AutomationSupport,
    pub transport_support: TransportSupport,
    pub rate_limited: bool,
    pub geo_sensitive: bool,
    pub contains_personal_data: bool,
    pub supports_raw_payload_export: bool,
}

impl Default for SourceCapabilityProfile {
    fn default() -> Self {
        Self {
            access_mode: SourceAccessMode::Public,
            cost_model: SourceCostModel::Free,
            automation_support: AutomationSupport::AutomaticPolling,
            transport_support: TransportSupport::DirectOrTor,
            rate_limited: false,
            geo_sensitive: false,
            contains_personal_data: false,
            supports_raw_payload_export: true,
        }
    }
}

impl SourceCapabilityProfile {
    #[must_use]
    pub fn recommended_for(source: &SourceDefinition) -> Self {
        let automation_support = match source.schedule {
            SourceSchedule::Fixed(_) => AutomationSupport::AutomaticPolling,
            SourceSchedule::EventDriven => AutomationSupport::EventDrivenOnly,
            SourceSchedule::Manual => AutomationSupport::ManualOnly,
        };
        let transport_support = match source.adapter {
            AdapterKind::TorHttpPoller => TransportSupport::TorOnly,
            AdapterKind::Manual | AdapterKind::FileImport | AdapterKind::DatabaseQuery => {
                TransportSupport::DirectOnly
            }
            _ => TransportSupport::DirectOrTor,
        };
        Self {
            automation_support,
            transport_support,
            ..Self::default()
        }
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.transport_support == TransportSupport::TorOnly
            && self.automation_support == AutomationSupport::ManualOnly
        {
            return Err(ValidationError::InvalidState(
                "tor-only sources must not be marked manual-only by default".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourcePolicy {
    pub allow_authenticated_sources: bool,
    pub allow_paid_sources: bool,
    pub allow_metered_sources: bool,
    pub allow_restricted_sources: bool,
    pub allow_tor_transport: bool,
    pub allow_automatic_queries: bool,
    pub allow_raw_payload_export: bool,
    pub allow_personal_data: bool,
    pub allow_rate_limited_sources: bool,
}

impl Default for SourcePolicy {
    fn default() -> Self {
        Self {
            allow_authenticated_sources: true,
            allow_paid_sources: true,
            allow_metered_sources: true,
            allow_restricted_sources: true,
            allow_tor_transport: true,
            allow_automatic_queries: true,
            allow_raw_payload_export: true,
            allow_personal_data: true,
            allow_rate_limited_sources: true,
        }
    }
}

impl SourcePolicy {
    pub fn evaluate_source(
        &self,
        source: &SourceDefinition,
        capability: &SourceCapabilityProfile,
    ) -> PolicyVerdict {
        if !self.allow_authenticated_sources
            && capability.access_mode == SourceAccessMode::Authenticated
        {
            return PolicyVerdict::deny("authenticated sources are disabled by policy");
        }
        if !self.allow_restricted_sources && capability.access_mode == SourceAccessMode::Restricted
        {
            return PolicyVerdict::deny("restricted/private sources are disabled by policy");
        }
        if !self.allow_paid_sources && capability.cost_model == SourceCostModel::Paid {
            return PolicyVerdict::deny("paid sources are disabled by policy");
        }
        if !self.allow_metered_sources && capability.cost_model == SourceCostModel::Metered {
            return PolicyVerdict::deny("metered sources are disabled by policy");
        }
        if !self.allow_personal_data && capability.contains_personal_data {
            return PolicyVerdict::deny("sources carrying personal data are disabled by policy");
        }
        if !self.allow_rate_limited_sources && capability.rate_limited {
            return PolicyVerdict::deny("rate-limited sources are disabled by policy");
        }
        if !self.allow_tor_transport && matches!(source.adapter, AdapterKind::TorHttpPoller) {
            return PolicyVerdict::deny("tor-routed sources are disabled by policy");
        }
        if !self.allow_automatic_queries && source.schedule.is_automatic() {
            return PolicyVerdict::deny("automatic polling is disabled by policy");
        }
        if source.schedule.is_automatic()
            && capability.automation_support == AutomationSupport::ManualOnly
        {
            return PolicyVerdict::deny(
                "source capability is manual-only but schedule is automatic",
            );
        }
        if matches!(source.schedule, SourceSchedule::EventDriven)
            && capability.automation_support == AutomationSupport::ManualOnly
        {
            return PolicyVerdict::deny(
                "source capability is manual-only but schedule is event-driven",
            );
        }
        if matches!(source.adapter, AdapterKind::TorHttpPoller)
            && capability.transport_support == TransportSupport::DirectOnly
        {
            return PolicyVerdict::deny("source capability does not allow tor transport");
        }
        if !matches!(source.adapter, AdapterKind::TorHttpPoller)
            && capability.transport_support == TransportSupport::TorOnly
            && source.schedule.is_automatic()
        {
            return PolicyVerdict::deny(
                "source capability requires tor transport for automatic polling",
            );
        }
        PolicyVerdict::Allow
    }

    #[must_use]
    pub fn allows_raw_payload_export(&self, capability: &SourceCapabilityProfile) -> bool {
        self.allow_raw_payload_export && capability.supports_raw_payload_export
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PolicyVerdict {
    Allow,
    Deny { reason: String },
}

impl PolicyVerdict {
    #[must_use]
    pub fn deny(reason: impl Into<String>) -> Self {
        Self::Deny {
            reason: reason.into(),
        }
    }

    #[must_use]
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allow)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FailureClass {
    Network,
    Timeout,
    Parse,
    Authentication,
    Policy,
    RateLimited,
    Validation,
    Storage,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureRecord {
    pub at: Timestamp,
    pub source_id: Option<SourceId>,
    pub class: FailureClass,
    pub code: String,
    pub detail: String,
}

impl FailureRecord {
    #[must_use]
    pub fn from_adapter_error(source_id: SourceId, at: Timestamp, error: &AdapterError) -> Self {
        let (class, code, detail) = classify_adapter_error(error);
        Self {
            at,
            source_id: Some(source_id),
            class,
            code,
            detail,
        }
    }

    #[must_use]
    pub fn policy(source_id: SourceId, at: Timestamp, reason: impl Into<String>) -> Self {
        Self {
            at,
            source_id: Some(source_id),
            class: FailureClass::Policy,
            code: "policy.denied".into(),
            detail: reason.into(),
        }
    }

    #[must_use]
    pub fn storage(source_id: Option<SourceId>, at: Timestamp, detail: impl Into<String>) -> Self {
        Self {
            at,
            source_id,
            class: FailureClass::Storage,
            code: "store.error".into(),
            detail: detail.into(),
        }
    }
}

fn classify_adapter_error(error: &AdapterError) -> (FailureClass, String, String) {
    match error {
        AdapterError::Validation(msg) => (
            FailureClass::Validation,
            "adapter.validation".into(),
            msg.clone(),
        ),
        AdapterError::Parse(msg) => (FailureClass::Parse, "adapter.parse".into(), msg.clone()),
        AdapterError::Unsupported(msg) => (
            FailureClass::Unsupported,
            "adapter.unsupported".into(),
            msg.clone(),
        ),
        AdapterError::Io(msg) => {
            let lower = msg.to_ascii_lowercase();
            if lower.contains("401")
                || lower.contains("403")
                || lower.contains("unauthor")
                || lower.contains("forbidden")
            {
                (
                    FailureClass::Authentication,
                    "transport.auth".into(),
                    msg.clone(),
                )
            } else if lower.contains("429") || lower.contains("rate limit") {
                (
                    FailureClass::RateLimited,
                    "transport.rate_limit".into(),
                    msg.clone(),
                )
            } else if lower.contains("timed out") || lower.contains("timeout") {
                (
                    FailureClass::Timeout,
                    "transport.timeout".into(),
                    msg.clone(),
                )
            } else {
                (FailureClass::Network, "transport.io".into(), msg.clone())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransformStep {
    AdapterPull,
    ReplayInject,
    Normalize,
    LatestStateUpdate,
    HistoryStore,
    WarmStore,
    MaterializeTopology,
    MaterializeSparseGeo,
    SnapshotExport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SampleProvenance {
    pub source_id: SourceId,
    pub source_name: String,
    pub adapter_kind: AdapterKind,
    pub endpoint: String,
    pub auth_ref: Option<String>,
    pub retrieved_at: Timestamp,
    pub source_timestamp: Option<Timestamp>,
    pub transform: Vec<TransformStep>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SampleAuditRecord {
    pub at: Timestamp,
    pub entity_id: EntityId,
    pub metric_id: MetricId,
    pub stored_history: bool,
    pub provenance: SampleProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportAuditRecord {
    pub at: Timestamp,
    pub snapshot_id: SnapshotId,
    pub format: ExportFormat,
    pub view_kind: ViewKind,
    pub output_path: String,
    pub entity_count: usize,
    pub metric_count: usize,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AuditRecord {
    Sample(SampleAuditRecord),
    Export(ExportAuditRecord),
}

#[derive(Debug, Clone)]
pub struct AuditTrail {
    max_records: usize,
    records: VecDeque<AuditRecord>,
}

impl Default for AuditTrail {
    fn default() -> Self {
        Self::new(2048).expect("default audit trail capacity is valid")
    }
}

impl AuditTrail {
    pub fn new(max_records: usize) -> Result<Self, ValidationError> {
        if max_records == 0 {
            return Err(ValidationError::ZeroCapacity("audit.max_records".into()));
        }
        Ok(Self {
            max_records,
            records: VecDeque::with_capacity(max_records),
        })
    }

    pub fn push(&mut self, record: AuditRecord) {
        if self.records.len() >= self.max_records {
            self.records.pop_front();
        }
        self.records.push_back(record);
    }

    #[must_use]
    pub fn snapshot(&self, limit: usize) -> Vec<AuditRecord> {
        if limit == 0 {
            return Vec::new();
        }
        let take = self.records.len().min(limit);
        self.records
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
    pub fn len(&self) -> usize {
        self.records.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}
