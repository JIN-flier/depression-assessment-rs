//! Typed errors at the quality boundary. Missing samples are measurements;
//! malformed shape, unsupported units and invalid configuration are errors.
use thiserror::Error;

pub type QualityResult<T> = Result<T, QualityError>;

#[derive(Debug, Error)]
pub enum QualityError {
    #[error("invalid quality configuration: {0}")]
    Configuration(String),
    #[error("invalid EEG recording: {0}")]
    InvalidRecording(String),
    #[error("quality assessment requires at least one EEG channel")]
    NoEegChannels,
    #[error("unsupported amplitude unit {unit:?} for EEG channel {channel}")]
    UnsupportedUnit { channel: String, unit: String },
    #[error("quality processing budget exceeded: {0}")]
    LimitExceeded(&'static str),
    #[error(transparent)]
    Domain(#[from] domain::DomainError),
    #[error("unable to encode quality provenance: {0}")]
    Serialization(#[from] serde_json::Error),
}

pub(crate) fn configuration(message: impl Into<String>) -> QualityError {
    QualityError::Configuration(message.into())
}
