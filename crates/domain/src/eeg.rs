//! Canonical EEG recording exchanged by import and processing crates.

use crate::{DomainError, Metadata, RecordingId, SubjectId, types::required};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Physical meaning of a channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    Eeg,
    Eog,
    Ecg,
    Emg,
    Trigger,
    Other,
}

/// Description of one row of the channel-major sample matrix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    pub label: String,
    pub kind: ChannelKind,
    /// Unit in which samples are represented, normally `uV` for EEG.
    pub unit: String,
}

impl Channel {
    pub fn new(
        label: impl Into<String>,
        kind: ChannelKind,
        unit: impl Into<String>,
    ) -> Result<Self, DomainError> {
        Ok(Self {
            label: required("channel.label", label)?,
            kind,
            unit: required("channel.unit", unit)?,
        })
    }
}

/// Lifecycle state of the samples currently carried by a recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingState {
    Raw,
    Normalized,
    Preprocessed,
}

/// Acquisition metadata needed by storage, compatibility checks, and reports.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RecordingMetadata {
    pub recorded_at: Option<DateTime<Utc>>,
    pub device: Option<String>,
    pub electrode_system: Option<String>,
    pub source_format: Option<String>,
    pub notes: Option<String>,
    #[serde(default)]
    pub extra: Metadata,
}

/// One deterministic processing operation applied to a recording.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessingStep {
    pub operation: String,
    pub algorithm_version: String,
    #[serde(default)]
    pub parameters: Metadata,
    pub applied_at: DateTime<Utc>,
}

impl ProcessingStep {
    pub fn new(
        operation: impl Into<String>,
        algorithm_version: impl Into<String>,
        applied_at: DateTime<Utc>,
    ) -> Result<Self, DomainError> {
        Ok(Self {
            operation: required("processing_step.operation", operation)?,
            algorithm_version: required("processing_step.algorithm_version", algorithm_version)?,
            parameters: Metadata::new(),
            applied_at,
        })
    }
}

/// Canonical channel-major EEG data model.
///
/// `samples[channel_index][sample_index]` corresponds to `channels[channel_index]`.
/// Construction validates the shape, labels, sampling rate, and all samples so
/// downstream algorithms do not each need to repeat those checks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EegRecording {
    pub id: RecordingId,
    pub subject_id: SubjectId,
    pub sampling_rate_hz: f64,
    pub channels: Vec<Channel>,
    pub samples: Vec<Vec<f32>>,
    pub recording_state: RecordingState,
    pub duration_seconds: f64,
    #[serde(default)]
    pub processing_history: Vec<ProcessingStep>,
    pub metadata: RecordingMetadata,
}

impl EegRecording {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: RecordingId,
        subject_id: SubjectId,
        sampling_rate_hz: f64,
        channels: Vec<Channel>,
        samples: Vec<Vec<f32>>,
        recording_state: RecordingState,
        metadata: RecordingMetadata,
    ) -> Result<Self, DomainError> {
        validate_sampling_rate(sampling_rate_hz)?;
        validate_channels_and_samples(&channels, &samples)?;
        let duration_seconds = samples[0].len() as f64 / sampling_rate_hz;

        Ok(Self {
            id,
            subject_id,
            sampling_rate_hz,
            channels,
            samples,
            recording_state,
            duration_seconds,
            processing_history: Vec::new(),
            metadata,
        })
    }

    #[must_use]
    pub fn sample_count(&self) -> usize {
        self.samples.first().map_or(0, Vec::len)
    }

    #[must_use]
    pub fn eeg_channel_count(&self) -> usize {
        self.channels
            .iter()
            .filter(|channel| channel.kind == ChannelKind::Eeg)
            .count()
    }
}

fn validate_sampling_rate(value: f64) -> Result<(), DomainError> {
    if !value.is_finite() {
        return Err(DomainError::NonFinite {
            field: "sampling_rate_hz",
            value,
        });
    }
    if value <= 0.0 {
        return Err(DomainError::OutOfRange {
            field: "sampling_rate_hz",
            min: f64::MIN_POSITIVE,
            max: f64::MAX,
            value,
        });
    }
    Ok(())
}

fn validate_channels_and_samples(
    channels: &[Channel],
    samples: &[Vec<f32>],
) -> Result<(), DomainError> {
    if channels.is_empty() {
        return Err(DomainError::NoChannels);
    }
    if channels.len() != samples.len() {
        return Err(DomainError::ChannelSampleCountMismatch {
            channels: channels.len(),
            sample_rows: samples.len(),
        });
    }

    let mut labels = BTreeSet::new();
    for channel in channels {
        if !labels.insert(channel.label.as_str()) {
            return Err(DomainError::DuplicateChannel {
                label: channel.label.clone(),
            });
        }
    }

    let expected = samples[0].len();
    for (channel, row) in channels.iter().zip(samples) {
        if row.len() != expected {
            return Err(DomainError::UnequalSampleCount {
                channel: channel.label.clone(),
                expected,
                actual: row.len(),
            });
        }
        if let Some(sample_index) = row.iter().position(|sample| !sample.is_finite()) {
            return Err(DomainError::NonFiniteSample {
                channel: channel.label.clone(),
                sample_index,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(label: &str) -> Channel {
        Channel::new(label, ChannelKind::Eeg, "uV").unwrap()
    }

    #[test]
    fn recording_computes_duration_from_shape() {
        let recording = EegRecording::new(
            RecordingId::new(),
            SubjectId::new(),
            250.0,
            vec![channel("Fp1"), channel("Fp2")],
            vec![vec![0.0; 500], vec![0.0; 500]],
            RecordingState::Raw,
            RecordingMetadata::default(),
        )
        .unwrap();

        assert_eq!(recording.duration_seconds, 2.0);
        assert_eq!(recording.sample_count(), 500);
        assert_eq!(recording.eeg_channel_count(), 2);
    }

    #[test]
    fn recording_rejects_ragged_sample_matrix() {
        let error = EegRecording::new(
            RecordingId::new(),
            SubjectId::new(),
            250.0,
            vec![channel("Fp1"), channel("Fp2")],
            vec![vec![0.0; 5], vec![0.0; 4]],
            RecordingState::Raw,
            RecordingMetadata::default(),
        )
        .unwrap_err();

        assert!(matches!(error, DomainError::UnequalSampleCount { .. }));
    }

    #[test]
    fn recording_rejects_duplicate_labels_and_non_finite_samples() {
        let duplicate = EegRecording::new(
            RecordingId::new(),
            SubjectId::new(),
            250.0,
            vec![channel("Fp1"), channel("Fp1")],
            vec![vec![0.0], vec![0.0]],
            RecordingState::Raw,
            RecordingMetadata::default(),
        );
        assert!(matches!(
            duplicate,
            Err(DomainError::DuplicateChannel { .. })
        ));

        let non_finite = EegRecording::new(
            RecordingId::new(),
            SubjectId::new(),
            250.0,
            vec![channel("Fp1")],
            vec![vec![f32::NAN]],
            RecordingState::Raw,
            RecordingMetadata::default(),
        );
        assert!(matches!(
            non_finite,
            Err(DomainError::NonFiniteSample { .. })
        ));
    }
}
