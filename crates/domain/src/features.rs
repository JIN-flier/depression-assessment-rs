//! Versioned EEG feature results.

use crate::{DomainError, Provenance, RecordingId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Standard frequency bands used by the V1 spectral pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrequencyBand {
    Delta,
    Theta,
    Alpha,
    Beta,
    Gamma,
}

/// Absolute and relative power for one frequency band.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BandPower {
    pub absolute: f64,
    pub relative: crate::UnitInterval,
}

/// Power spectral density and band power, keyed by channel label.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpectralFeatures {
    pub frequencies_hz: Vec<f64>,
    pub psd_by_channel: BTreeMap<String, Vec<f64>>,
    pub band_power_by_channel: BTreeMap<String, BTreeMap<FrequencyBand, BandPower>>,
}

impl SpectralFeatures {
    pub fn new(
        frequencies_hz: Vec<f64>,
        psd_by_channel: BTreeMap<String, Vec<f64>>,
        band_power_by_channel: BTreeMap<String, BTreeMap<FrequencyBand, BandPower>>,
    ) -> Result<Self, DomainError> {
        for (channel, psd) in &psd_by_channel {
            if psd.len() != frequencies_hz.len() {
                return Err(DomainError::PsdLengthMismatch {
                    channel: channel.clone(),
                    expected: frequencies_hz.len(),
                    actual: psd.len(),
                });
            }
        }
        validate_finite("frequencies_hz", frequencies_hz.iter().copied())?;
        for psd in psd_by_channel.values() {
            validate_finite("psd", psd.iter().copied())?;
        }
        for bands in band_power_by_channel.values() {
            validate_finite(
                "absolute_band_power",
                bands.values().map(|power| power.absolute),
            )?;
        }
        Ok(Self {
            frequencies_hz,
            psd_by_channel,
            band_power_by_channel,
        })
    }
}

/// Extensible named numerical features for later product phases.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct FeatureValues {
    #[serde(default)]
    pub values: BTreeMap<String, f64>,
}

/// Feature bundle whose optional groups allow V1 data to evolve through V3.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EegFeatures {
    pub recording_id: RecordingId,
    pub spectral: Option<SpectralFeatures>,
    pub spatial: Option<FeatureValues>,
    pub connectivity: Option<FeatureValues>,
    pub complexity: Option<FeatureValues>,
    pub provenance: Provenance,
}

impl EegFeatures {
    #[must_use]
    pub fn spectral(
        recording_id: RecordingId,
        spectral: SpectralFeatures,
        provenance: Provenance,
    ) -> Self {
        Self {
            recording_id,
            spectral: Some(spectral),
            spatial: None,
            connectivity: None,
            complexity: None,
            provenance,
        }
    }
}

fn validate_finite(
    field: &'static str,
    values: impl IntoIterator<Item = f64>,
) -> Result<(), DomainError> {
    for value in values {
        if !value.is_finite() {
            return Err(DomainError::NonFinite { field, value });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spectral_features_require_one_psd_value_per_frequency() {
        let result = SpectralFeatures::new(
            vec![1.0, 2.0],
            BTreeMap::from([("Fp1".into(), vec![0.5])]),
            BTreeMap::new(),
        );
        assert!(matches!(result, Err(DomainError::PsdLengthMismatch { .. })));
    }
}
