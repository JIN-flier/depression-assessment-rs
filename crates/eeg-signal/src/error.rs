//! Errors local to the signal boundary; callers can inspect them without parsing text.
use thiserror::Error;

pub type SignalResult<T> = Result<T, SignalError>;

#[derive(Debug, Error)]
pub enum SignalError {
    #[error("invalid signal configuration: {0}")]
    Configuration(String),
    #[error("invalid EEG recording: {0}")]
    InvalidRecording(String),
    #[error(transparent)]
    Domain(#[from] domain::DomainError),
    #[error("{operation} requires at least {minimum} samples; got {actual}")]
    TooShort {
        operation: &'static str,
        minimum: usize,
        actual: usize,
    },
    #[error("signal processing budget exceeded: {0}")]
    LimitExceeded(&'static str),
    #[error("non-finite or overflowing result in {operation}")]
    Numerical { operation: &'static str },
    #[error("unable to encode processing parameters: {0}")]
    Serialization(#[from] serde_json::Error),
}

pub(crate) fn configuration(message: impl Into<String>) -> SignalError {
    SignalError::Configuration(message.into())
}

/// DSP uses f64, but the domain stores f32. Never silently clamp overflow:
/// doing so would turn a failed computation into plausible EEG.
pub(crate) fn sample(value: f64, operation: &'static str) -> SignalResult<f32> {
    let output = value as f32;
    if !value.is_finite() || !output.is_finite() {
        return Err(SignalError::Numerical { operation });
    }
    Ok(output)
}
