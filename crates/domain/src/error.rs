//! Errors raised while constructing or validating domain values.

use thiserror::Error;

/// A violation of a domain invariant.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum DomainError {
    #[error("{field} must not be empty")]
    EmptyField { field: &'static str },

    #[error("{field} must be finite, got {value}")]
    NonFinite { field: &'static str, value: f64 },

    #[error("{field} must be in [{min}, {max}], got {value}")]
    OutOfRange {
        field: &'static str,
        min: f64,
        max: f64,
        value: f64,
    },

    #[error("age must be at most {max}, got {value}")]
    InvalidAge { max: u8, value: u8 },

    #[error("EEG recording must contain at least one channel")]
    NoChannels,

    #[error("duplicate EEG channel label: {label}")]
    DuplicateChannel { label: String },

    #[error("channel count ({channels}) does not match sample rows ({sample_rows})")]
    ChannelSampleCountMismatch { channels: usize, sample_rows: usize },

    #[error(
        "channel {channel} has {actual} samples; expected {expected} samples for every channel"
    )]
    UnequalSampleCount {
        channel: String,
        expected: usize,
        actual: usize,
    },

    #[error("channel {channel} contains a non-finite sample at index {sample_index}")]
    NonFiniteSample {
        channel: String,
        sample_index: usize,
    },

    #[error("spectral PSD for channel {channel} has {actual} values; expected {expected}")]
    PsdLengthMismatch {
        channel: String,
        expected: usize,
        actual: usize,
    },

    #[error("unknown bad channel: {channel}")]
    UnknownBadChannel { channel: String },

    #[error("{entity} belongs to {actual}, expected {expected}")]
    EntityMismatch {
        entity: &'static str,
        expected: String,
        actual: String,
    },
}
