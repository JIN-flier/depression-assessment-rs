//! Shared validation and canonicalization, independent of file syntax.

use crate::{EegIoError, EegIoResult, error::invalid};
use domain::{
    Channel, ChannelKind, EegRecording, RecordingId, RecordingMetadata, RecordingState, SubjectId,
};
use std::{collections::BTreeSet, io::Read};

/// Resource budgets bound encoded bytes and decoded matrix dimensions,
/// including the uncompressed payload of compressed MAT elements.
#[derive(Debug, Clone, Copy)]
pub struct ImportLimits {
    pub max_input_bytes: usize,
    pub max_channels: usize,
    pub max_samples_per_channel: usize,
    pub max_total_samples: usize,
}

impl Default for ImportLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 256 * 1024 * 1024,
            max_channels: 1024,
            max_samples_per_channel: 10_000_000,
            max_total_samples: 64_000_000,
        }
    }
}

impl ImportLimits {
    pub(crate) fn shape(&self, channels: usize, samples: usize) -> EegIoResult<()> {
        if channels == 0 || samples == 0 {
            return Err(invalid(
                "EEG",
                "recording must contain channels and samples",
            ));
        }
        if channels > self.max_channels {
            return Err(EegIoError::LimitExceeded("channel count"));
        }
        if samples > self.max_samples_per_channel {
            return Err(EegIoError::LimitExceeded("samples per channel"));
        }
        if channels
            .checked_mul(samples)
            .is_none_or(|n| n > self.max_total_samples)
        {
            return Err(EegIoError::LimitExceeded("total samples"));
        }
        Ok(())
    }
}

/// Application-owned IDs are never inferred from identifying EEG file fields.
/// Caller metadata is retained except source format and importer provenance.
#[derive(Debug, Clone)]
pub struct ImportContext {
    pub recording_id: RecordingId,
    pub subject_id: SubjectId,
    pub metadata: RecordingMetadata,
    pub limits: ImportLimits,
}

impl ImportContext {
    pub fn new(recording_id: RecordingId, subject_id: SubjectId) -> Self {
        Self {
            recording_id,
            subject_id,
            metadata: RecordingMetadata::default(),
            limits: ImportLimits::default(),
        }
    }
}

pub(crate) fn bounded_bytes(input: &mut dyn Read, limit: usize) -> EegIoResult<Vec<u8>> {
    let mut bytes = Vec::new();
    input
        .take((limit as u64).saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(EegIoError::LimitExceeded("input/decompressed bytes"));
    }
    Ok(bytes)
}

pub(crate) fn sampling_rate(rate: f64) -> EegIoResult<()> {
    if !rate.is_finite() || rate <= 0.0 {
        return Err(EegIoError::Configuration(
            "sampling rate must be finite and positive".into(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_channels(channels: &[Channel]) -> EegIoResult<()> {
    let mut labels = BTreeSet::new();
    for channel in channels {
        Channel::new(&channel.label, channel.kind.clone(), &channel.unit)?;
        if !labels.insert(channel.label.trim()) {
            return Err(EegIoError::Configuration(format!(
                "duplicate channel: {}",
                channel.label
            )));
        }
    }
    Ok(())
}

/// EEG units are never guessed. Non-EEG channels may carry trigger codes or
/// temperatures; these are preserved rather than interpreted as voltages.
pub(crate) fn canonical_channel(channel: &Channel) -> EegIoResult<(Channel, f64)> {
    let factor = match channel.unit.trim() {
        "V" => 1_000_000.0,
        "mV" => 1_000.0,
        "uV" | "µV" | "μV" => 1.0,
        "nV" => 0.001,
        other if channel.kind == ChannelKind::Eeg => {
            return Err(EegIoError::Unsupported(format!("EEG unit {other:?}")));
        }
        _ => return Ok((channel.clone(), 1.0)),
    };
    Ok((
        Channel::new(channel.label.trim(), channel.kind.clone(), "uV")?,
        factor,
    ))
}

pub(crate) fn sample(
    value: f64,
    factor: f64,
    format: &'static str,
    location: impl std::fmt::Display,
) -> EegIoResult<f32> {
    let result = (value * factor) as f32;
    if !value.is_finite() || !result.is_finite() {
        return Err(invalid(
            format,
            format!("non-finite or overflowing amplitude at {location}"),
        ));
    }
    Ok(result)
}

pub(crate) fn finish(
    context: &ImportContext,
    format: &str,
    rate: f64,
    channels: Vec<Channel>,
    samples: Vec<Vec<f32>>,
    mut metadata: RecordingMetadata,
) -> EegIoResult<EegRecording> {
    sampling_rate(rate)?;
    validate_channels(&channels)?;
    context
        .limits
        .shape(channels.len(), samples.first().map_or(0, Vec::len))?;
    let duration = samples.first().map_or(0, Vec::len) as f64 / rate;
    if !duration.is_finite() {
        return Err(invalid(
            "EEG",
            "sample count and sampling rate produce non-finite duration",
        ));
    }
    metadata.source_format = Some(format.into());
    metadata
        .extra
        .insert("eeg_io.version".into(), env!("CARGO_PKG_VERSION").into());
    metadata
        .extra
        .insert("eeg_io.amplitude_conversion".into(), "voltage_to_uV".into());
    // Calibration is decoding, not a P4 processing step. Acquisition prefilters
    // remain metadata; the signal lifecycle is still Raw.
    Ok(EegRecording::new(
        context.recording_id,
        context.subject_id,
        rate,
        channels,
        samples,
        RecordingState::Raw,
        metadata,
    )?)
}

/// Preserve the input schema separately from canonical channels, so unit
/// conversion can be reproduced after storing only the resulting recording.
pub(crate) fn source_channel_metadata(metadata: &mut RecordingMetadata, channels: &[Channel]) {
    for (index, channel) in channels.iter().enumerate() {
        metadata.extra.insert(
            format!("eeg_io.channel.{index}.source_unit"),
            channel.unit.clone(),
        );
    }
}
