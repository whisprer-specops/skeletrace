// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Minimal world scaffold and spatial helpers.
//!
//! This module intentionally stays small. It provides enough WGS-84 math
//! to anchor sparse entities, but does not model terrain, tiles, or a
//! fully populated globe.

use serde::{Deserialize, Serialize};

use crate::types::ValidationError;

/// WGS-84 geographic position. Altitude is optional because most
/// OSINT nodes only have a 2D fix.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GeoCoord {
    pub lat: f64,
    pub lon: f64,
    pub alt: Option<f64>,
}

impl GeoCoord {
    pub fn new(lat: f64, lon: f64, alt: Option<f64>) -> Result<Self, ValidationError> {
        if !lat.is_finite() || !(-90.0..=90.0).contains(&lat) {
            return Err(ValidationError::OutOfRange {
                field: "lat".into(),
                min: "-90".into(),
                max: "90".into(),
                found: lat.to_string(),
            });
        }
        if !lon.is_finite() || !(-180.0..=180.0).contains(&lon) {
            return Err(ValidationError::OutOfRange {
                field: "lon".into(),
                min: "-180".into(),
                max: "180".into(),
                found: lon.to_string(),
            });
        }
        if let Some(alt) = alt {
            if !alt.is_finite() {
                return Err(ValidationError::InvalidState(
                    "altitude must be finite".into(),
                ));
            }
        }
        Ok(Self { lat, lon, alt })
    }

    #[must_use]
    pub fn normalized_lon(self) -> Self {
        let mut lon = self.lon;
        while lon > 180.0 {
            lon -= 360.0;
        }
        while lon < -180.0 {
            lon += 360.0;
        }
        Self { lon, ..self }
    }

    #[must_use]
    pub fn to_ecef(self, ellipsoid: Ellipsoid) -> CartesianCoord {
        let lat = self.lat.to_radians();
        let lon = self.lon.to_radians();
        let alt = self.alt.unwrap_or(0.0);

        let sin_lat = lat.sin();
        let cos_lat = lat.cos();
        let cos_lon = lon.cos();
        let sin_lon = lon.sin();

        let n = ellipsoid.semi_major / (1.0 - ellipsoid.e_sq * sin_lat * sin_lat).sqrt();

        CartesianCoord {
            x: (n + alt) * cos_lat * cos_lon,
            y: (n + alt) * cos_lat * sin_lon,
            z: (n * (1.0 - ellipsoid.e_sq) + alt) * sin_lat,
        }
    }

    pub fn from_ecef(cart: CartesianCoord, ellipsoid: Ellipsoid) -> Result<Self, ValidationError> {
        if !cart.x.is_finite() || !cart.y.is_finite() || !cart.z.is_finite() {
            return Err(ValidationError::InvalidState(
                "ECEF coordinates must be finite".into(),
            ));
        }

        let a = ellipsoid.semi_major;
        let b = ellipsoid.semi_minor;
        let e_sq = ellipsoid.e_sq;
        let ep_sq = (a * a - b * b) / (b * b);
        let p = (cart.x * cart.x + cart.y * cart.y).sqrt();

        let theta = (cart.z * a).atan2(p * b);
        let sin_theta = theta.sin();
        let cos_theta = theta.cos();

        let lat = (cart.z + ep_sq * b * sin_theta.powi(3)).atan2(p - e_sq * a * cos_theta.powi(3));
        let lon = cart.y.atan2(cart.x);

        let sin_lat = lat.sin();
        let n = a / (1.0 - e_sq * sin_lat * sin_lat).sqrt();
        let alt = p / lat.cos() - n;

        Self::new(lat.to_degrees(), lon.to_degrees(), Some(alt))
    }
}

/// Earth-centred, Earth-fixed (ECEF) Cartesian position in metres.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CartesianCoord {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl CartesianCoord {
    #[must_use]
    pub fn distance_to(self, other: Self) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        let dz = self.z - other.z;
        (dx * dx + dy * dy + dz * dz).sqrt()
    }
}

/// WGS-84 ellipsoid constants.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Ellipsoid {
    pub semi_major: f64,
    pub semi_minor: f64,
    pub flattening: f64,
    pub e_sq: f64,
}

pub const WGS84: Ellipsoid = Ellipsoid {
    semi_major: 6_378_137.0,
    semi_minor: 6_356_752.314_245_179,
    flattening: 1.0 / 298.257_223_563,
    e_sq: 0.006_694_379_990_141_316,
};

/// Axis-aligned geographic bounding box.
/// This simple representation is not antimeridian-aware by design.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GeoBBox {
    pub min_lat: f64,
    pub min_lon: f64,
    pub max_lat: f64,
    pub max_lon: f64,
}

impl GeoBBox {
    pub fn new(
        min_lat: f64,
        min_lon: f64,
        max_lat: f64,
        max_lon: f64,
    ) -> Result<Self, ValidationError> {
        let bbox = Self {
            min_lat,
            min_lon,
            max_lat,
            max_lon,
        };
        bbox.validate()?;
        Ok(bbox)
    }

    pub fn validate(self) -> Result<(), ValidationError> {
        for (field, value, min, max) in [
            ("min_lat", self.min_lat, -90.0, 90.0),
            ("max_lat", self.max_lat, -90.0, 90.0),
            ("min_lon", self.min_lon, -180.0, 180.0),
            ("max_lon", self.max_lon, -180.0, 180.0),
        ] {
            if !value.is_finite() || !(min..=max).contains(&value) {
                return Err(ValidationError::OutOfRange {
                    field: field.into(),
                    min: min.to_string(),
                    max: max.to_string(),
                    found: value.to_string(),
                });
            }
        }

        if self.min_lat > self.max_lat {
            return Err(ValidationError::InvalidWindow {
                start_field: "min_lat".into(),
                end_field: "max_lat".into(),
            });
        }
        if self.min_lon > self.max_lon {
            return Err(ValidationError::InvalidWindow {
                start_field: "min_lon".into(),
                end_field: "max_lon".into(),
            });
        }
        Ok(())
    }

    #[must_use]
    pub fn contains(self, coord: GeoCoord) -> bool {
        coord.lat >= self.min_lat
            && coord.lat <= self.max_lat
            && coord.lon >= self.min_lon
            && coord.lon <= self.max_lon
    }

    #[must_use]
    pub fn intersects(self, other: Self) -> bool {
        !(self.max_lat < other.min_lat
            || self.min_lat > other.max_lat
            || self.max_lon < other.min_lon
            || self.min_lon > other.max_lon)
    }

    #[must_use]
    pub fn center(self) -> GeoCoord {
        GeoCoord {
            lat: (self.min_lat + self.max_lat) / 2.0,
            lon: (self.min_lon + self.max_lon) / 2.0,
            alt: None,
        }
    }

    #[must_use]
    pub fn width_deg(self) -> f64 {
        self.max_lon - self.min_lon
    }

    #[must_use]
    pub fn height_deg(self) -> f64 {
        self.max_lat - self.min_lat
    }

    #[must_use]
    pub fn expanded_to_include(self, coord: GeoCoord) -> Self {
        Self {
            min_lat: self.min_lat.min(coord.lat),
            min_lon: self.min_lon.min(coord.lon),
            max_lat: self.max_lat.max(coord.lat),
            max_lon: self.max_lon.max(coord.lon),
        }
    }
}
