//! P6 自有错误边界，调用者可以匹配错误类型，不必解析错误文本。
use thiserror::Error;

pub type FeatureResult<T> = Result<T, FeatureError>;

#[derive(Debug, Error)]
pub enum FeatureError {
    #[error("invalid feature configuration: {0}")]
    Configuration(String),
    #[error("invalid EEG recording: {0}")]
    InvalidRecording(String),
    #[error(transparent)]
    Domain(#[from] domain::DomainError),
    #[error("no EEG channels remain for feature extraction")]
    NoEegChannels,
    #[error("unsupported EEG unit {unit:?} on channel {channel}")]
    UnsupportedUnit { channel: String, unit: String },
    #[error("excluded channel does not exist: {0}")]
    UnknownChannel(String),
    #[error("Welch requires at least {minimum} samples; got {actual}")]
    TooShort { minimum: usize, actual: usize },
    #[error("range [{low_hz}, {high_hz}) exceeds Nyquist {nyquist_hz} Hz")]
    UnavailableRange {
        low_hz: f64,
        high_hz: f64,
        nyquist_hz: f64,
    },
    #[error("range [{low_hz}, {high_hz}) contains no bins at spacing {bin_width_hz} Hz")]
    UnresolvedRange {
        low_hz: f64,
        high_hz: f64,
        bin_width_hz: f64,
    },
    #[error("feature extraction budget exceeded: {0}")]
    LimitExceeded(&'static str),
    #[error("non-finite or overflowing numerical result in {0}")]
    Numerical(&'static str),
    #[error("unable to encode feature provenance: {0}")]
    Serialization(#[from] serde_json::Error),
}

pub(crate) fn configuration(message: impl Into<String>) -> FeatureError {
    FeatureError::Configuration(message.into())
}
