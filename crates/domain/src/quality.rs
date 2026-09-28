//! Structured output of EEG signal-quality assessment.

use crate::{DomainError, Provenance, RecordingId, UnitInterval, Warning};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Quality measurements for one channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelQuality {
    pub score: UnitInterval,
    pub missing_fraction: UnitInterval,
    pub noisy_fraction: UnitInterval,
    pub power_line_interference: UnitInterval,
    #[serde(default)]
    pub warnings: Vec<Warning>,
}

/// Result of a quality algorithm for one recording.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignalQuality {
    pub recording_id: RecordingId,
    pub overall_score: UnitInterval,
    #[serde(default)]
    pub bad_channels: Vec<String>,
    pub channel_quality: BTreeMap<String, ChannelQuality>,
    #[serde(default)]
    pub warnings: Vec<Warning>,
    pub provenance: Provenance,
}

impl SignalQuality {
    pub fn new(
        recording_id: RecordingId,
        overall_score: UnitInterval,
        bad_channels: Vec<String>,
        channel_quality: BTreeMap<String, ChannelQuality>,
        provenance: Provenance,
    ) -> Result<Self, DomainError> {
        let mut unique = BTreeSet::new();
        for channel in &bad_channels {
            if !channel_quality.contains_key(channel) {
                return Err(DomainError::UnknownBadChannel {
                    channel: channel.clone(),
                });
            }
            if !unique.insert(channel) {
                return Err(DomainError::DuplicateChannel {
                    label: channel.clone(),
                });
            }
        }
        Ok(Self {
            recording_id,
            overall_score,
            bad_channels,
            channel_quality,
            warnings: Vec::new(),
            provenance,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn provenance() -> Provenance {
        Provenance::new("0.1.0", "quality", "1", Utc::now()).unwrap()
    }

    #[test]
    fn bad_channels_must_exist_in_channel_results() {
        let result = SignalQuality::new(
            RecordingId::new(),
            UnitInterval::ONE,
            vec!["Fp1".into()],
            BTreeMap::new(),
            provenance(),
        );
        assert!(matches!(result, Err(DomainError::UnknownBadChannel { .. })));
    }
}
