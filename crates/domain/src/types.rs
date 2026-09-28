//! Small value objects shared by the domain modules.

use crate::DomainError;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Extensible metadata with deterministic key ordering when serialized.
pub type Metadata = BTreeMap<String, String>;

/// A finite value constrained to the inclusive interval `[0, 1]`.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "f64", into = "f64")]
pub struct UnitInterval(f64);

impl UnitInterval {
    pub const ZERO: Self = Self(0.0);
    pub const ONE: Self = Self(1.0);

    pub fn new(field: &'static str, value: f64) -> Result<Self, DomainError> {
        if !value.is_finite() {
            return Err(DomainError::NonFinite { field, value });
        }
        if !(0.0..=1.0).contains(&value) {
            return Err(DomainError::OutOfRange {
                field,
                min: 0.0,
                max: 1.0,
                value,
            });
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }
}

impl TryFrom<f64> for UnitInterval {
    type Error = DomainError;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new("unit interval", value)
    }
}

impl From<UnitInterval> for f64 {
    fn from(value: UnitInterval) -> Self {
        value.get()
    }
}

/// Captures versions, parameters, and time required to reproduce a result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    pub software_version: String,
    pub algorithm: String,
    pub algorithm_version: String,
    #[serde(default)]
    pub parameters: Metadata,
    pub generated_at: DateTime<Utc>,
}

impl Provenance {
    pub fn new(
        software_version: impl Into<String>,
        algorithm: impl Into<String>,
        algorithm_version: impl Into<String>,
        generated_at: DateTime<Utc>,
    ) -> Result<Self, DomainError> {
        let software_version = required("software_version", software_version)?;
        let algorithm = required("algorithm", algorithm)?;
        let algorithm_version = required("algorithm_version", algorithm_version)?;
        Ok(Self {
            software_version,
            algorithm,
            algorithm_version,
            parameters: Metadata::new(),
            generated_at,
        })
    }
}

/// A structured warning produced by a deterministic subsystem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Warning {
    pub code: String,
    pub message: String,
}

impl Warning {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Result<Self, DomainError> {
        Ok(Self {
            code: required("warning.code", code)?,
            message: required("warning.message", message)?,
        })
    }
}

pub(crate) fn required(
    field: &'static str,
    value: impl Into<String>,
) -> Result<String, DomainError> {
    let value = value.into();
    if value.trim().is_empty() {
        Err(DomainError::EmptyField { field })
    } else {
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_interval_rejects_invalid_numbers() {
        assert!(UnitInterval::new("score", -0.1).is_err());
        assert!(UnitInterval::new("score", 1.1).is_err());
        assert!(UnitInterval::new("score", f64::NAN).is_err());
        assert_eq!(UnitInterval::new("score", 0.25).unwrap().get(), 0.25);
    }

    #[test]
    fn deserialization_cannot_bypass_unit_interval_validation() {
        assert!(serde_json::from_str::<UnitInterval>("1.2").is_err());
    }
}
