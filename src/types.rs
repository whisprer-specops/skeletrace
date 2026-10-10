// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Common type aliases and strongly typed identifiers.

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// All timestamps in skeletrace are UTC.
pub type Timestamp = DateTime<Utc>;

/// Shared validation and invariants error type used by the core modules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValidationError {
    OutOfRange {
        field: String,
        min: String,
        max: String,
        found: String,
    },
    EmptyField(String),
    InvalidWindow {
        start_field: String,
        end_field: String,
    },
    InvalidState(String),
    InvalidReference(String),
    ZeroCapacity(String),
    Unsupported(String),
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfRange {
                field,
                min,
                max,
                found,
            } => write!(
                f,
                "field `{field}` out of range: expected {min}..={max}, found {found}"
            ),
            Self::EmptyField(field) => write!(f, "field `{field}` must not be empty"),
            Self::InvalidWindow {
                start_field,
                end_field,
            } => write!(
                f,
                "invalid time window: `{start_field}` must be <= `{end_field}`"
            ),
            Self::InvalidState(msg) => write!(f, "invalid state: {msg}"),
            Self::InvalidReference(msg) => write!(f, "invalid reference: {msg}"),
            Self::ZeroCapacity(field) => write!(f, "field `{field}` must be greater than zero"),
            Self::Unsupported(msg) => write!(f, "unsupported operation: {msg}"),
        }
    }
}

impl std::error::Error for ValidationError {}

macro_rules! id_newtype {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        pub struct $name(pub Uuid);

        impl Default for $name {
            /// Generate a fresh identifier, as with `new()`.
            fn default() -> Self {
                Self::new()
            }
        }

        impl $name {
            /// Generates a new random v4 identifier.
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// Wraps an existing UUID.
            #[must_use]
            pub const fn from_uuid(uuid: Uuid) -> Self {
                Self(uuid)
            }

            /// Exposes the wrapped UUID by value.
            #[must_use]
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }

            /// Nil UUID helper for tests and placeholders.
            #[must_use]
            pub fn nil() -> Self {
                Self(Uuid::nil())
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

id_newtype!(
    EntityId,
    "Identifies a graph entity (node, edge, or boundary)."
);
id_newtype!(
    MetricId,
    "Identifies a metric definition in the metric dictionary."
);
id_newtype!(
    SourceId,
    "Identifies a data source (API, stream, file, database, etc.)."
);
id_newtype!(
    FlowId,
    "Identifies a flow (a time-bound traversal across relations)."
);
id_newtype!(SnapshotId, "Identifies a persisted snapshot.");
id_newtype!(ViewJobId, "Identifies a view/render job request.");

/// Confidence score clamped to [0.0, 1.0].
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Confidence(pub f64);

impl Confidence {
    pub const MIN: f64 = 0.0;
    pub const MAX: f64 = 1.0;

    pub fn new(value: f64) -> Result<Self, ValidationError> {
        if !(Self::MIN..=Self::MAX).contains(&value) || !value.is_finite() {
            return Err(ValidationError::OutOfRange {
                field: "confidence".into(),
                min: Self::MIN.to_string(),
                max: Self::MAX.to_string(),
                found: value.to_string(),
            });
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn get(self) -> f64 {
        self.0
    }
}

impl Default for Confidence {
    fn default() -> Self {
        Self(1.0)
    }
}

/// Quality score for a sample, clamped to [0.0, 1.0].
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Quality(pub f64);

impl Quality {
    pub const MIN: f64 = 0.0;
    pub const MAX: f64 = 1.0;

    pub fn new(value: f64) -> Result<Self, ValidationError> {
        if !(Self::MIN..=Self::MAX).contains(&value) || !value.is_finite() {
            return Err(ValidationError::OutOfRange {
                field: "quality".into(),
                min: Self::MIN.to_string(),
                max: Self::MAX.to_string(),
                found: value.to_string(),
            });
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn get(self) -> f64 {
        self.0
    }
}

impl Default for Quality {
    fn default() -> Self {
        Self(1.0)
    }
}

/// Priority value. Higher = more important for retention/rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Priority(pub u8);

impl Priority {
    pub const LOW: Self = Self(32);
    pub const NORMAL: Self = Self(128);
    pub const HIGH: Self = Self(192);
    pub const CRITICAL: Self = Self(255);

    #[must_use]
    pub const fn new(value: u8) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl Default for Priority {
    fn default() -> Self {
        Self::NORMAL
    }
}

/// Freeform key-value tag attached to entities or sources.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Tag {
    pub key: String,
    pub value: String,
}

impl Tag {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Result<Self, ValidationError> {
        let key = key.into();
        let value = value.into();

        if key.trim().is_empty() {
            return Err(ValidationError::EmptyField("tag.key".into()));
        }
        if value.trim().is_empty() {
            return Err(ValidationError::EmptyField("tag.value".into()));
        }

        Ok(Self { key, value })
    }
}
