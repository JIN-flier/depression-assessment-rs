//! Model inference, compatibility, uncertainty, and explainability data.

use crate::{DomainError, Metadata, ModelResultId, RecordingId, UnitInterval, types::required};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Result of checking current EEG data against a model's declared inputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ModelCompatibility {
    Compatible,
    Incompatible { reasons: Vec<String> },
}

impl ModelCompatibility {
    #[must_use]
    pub fn is_compatible(&self) -> bool {
        matches!(self, Self::Compatible)
    }
}

/// Importance assigned to a named channel, band, or region.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Importance {
    pub name: String,
    pub value: f64,
}

/// Structured explainability output; no visualization implementation leaks in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ExplainabilityResult {
    #[serde(default)]
    pub channel_importance: Vec<Importance>,
    #[serde(default)]
    pub frequency_importance: Vec<Importance>,
    #[serde(default)]
    pub region_importance: Vec<Importance>,
    #[serde(default)]
    pub metadata: Metadata,
}

/// Versioned output of one inference run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelResult {
    pub id: ModelResultId,
    pub recording_id: RecordingId,
    pub model_id: String,
    pub model_version: String,
    pub prediction: String,
    pub probability: UnitInterval,
    pub calibrated_probability: Option<UnitInterval>,
    pub entropy: Option<f64>,
    pub ood_score: Option<f64>,
    pub compatibility: ModelCompatibility,
    pub explainability: Option<ExplainabilityResult>,
    pub generated_at: DateTime<Utc>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

impl ModelResult {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: ModelResultId,
        recording_id: RecordingId,
        model_id: impl Into<String>,
        model_version: impl Into<String>,
        prediction: impl Into<String>,
        probability: UnitInterval,
        compatibility: ModelCompatibility,
        generated_at: DateTime<Utc>,
    ) -> Result<Self, DomainError> {
        Ok(Self {
            id,
            recording_id,
            model_id: required("model.model_id", model_id)?,
            model_version: required("model.model_version", model_version)?,
            prediction: required("model.prediction", prediction)?,
            probability,
            calibrated_probability: None,
            entropy: None,
            ood_score: None,
            compatibility,
            explainability: None,
            generated_at,
            metadata: Metadata::new(),
        })
    }

    /// Adds uncertainty values after validating they are finite and non-negative.
    pub fn with_uncertainty(mut self, entropy: f64, ood_score: f64) -> Result<Self, DomainError> {
        for (field, value) in [("model.entropy", entropy), ("model.ood_score", ood_score)] {
            if !value.is_finite() {
                return Err(DomainError::NonFinite { field, value });
            }
            if value < 0.0 {
                return Err(DomainError::OutOfRange {
                    field,
                    min: 0.0,
                    max: f64::MAX,
                    value,
                });
            }
        }
        self.entropy = Some(entropy);
        self.ood_score = Some(ood_score);
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incompatible_models_are_explicitly_blocked() {
        let compatibility = ModelCompatibility::Incompatible {
            reasons: vec!["sampling rate mismatch".into()],
        };
        assert!(!compatibility.is_compatible());
    }

    #[test]
    fn uncertainty_rejects_negative_values() {
        let model = ModelResult::new(
            ModelResultId::new(),
            RecordingId::new(),
            "eegnet",
            "1.0.0",
            "elevated",
            UnitInterval::new("probability", 0.7).unwrap(),
            ModelCompatibility::Compatible,
            Utc::now(),
        )
        .unwrap();
        assert!(model.with_uncertainty(-0.1, 0.2).is_err());
    }
}
