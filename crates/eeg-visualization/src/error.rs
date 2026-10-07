//! 可匹配的错误边界，避免应用层解析文本或在图像失败时崩溃。
use thiserror::Error;

pub type VisualizationResult<T> = Result<T, VisualizationError>;

#[derive(Debug, Error)]
pub enum VisualizationError {
    #[error("invalid visualization configuration: {0}")]
    Configuration(String),
    #[error("invalid visualization input: {0}")]
    InvalidInput(String),
    #[error("channel is unavailable: {0}")]
    UnknownChannel(String),
    #[error("unsupported signal unit {unit:?} on channel {channel}")]
    UnsupportedUnit { channel: String, unit: String },
    #[error("requested display range contains no samples or bins")]
    EmptyRange,
    #[error("spectral features are unavailable")]
    MissingSpectral,
    #[error("band {band:?} is unavailable on channel {channel}")]
    MissingBand {
        channel: String,
        band: domain::FrequencyBand,
    },
    #[error("electrode position is unavailable: {0}")]
    MissingPosition(String),
    #[error("topomap needs at least three distinct, non-collinear electrode positions")]
    InsufficientGeometry,
    #[error("visualization resource budget exceeded: {0}")]
    LimitExceeded(&'static str),
    #[error("non-finite or overflowing display value: {0}")]
    Numerical(&'static str),
}

pub(crate) fn invalid(message: impl Into<String>) -> VisualizationError {
    VisualizationError::InvalidInput(message.into())
}

pub(crate) fn configuration(message: impl Into<String>) -> VisualizationError {
    VisualizationError::Configuration(message.into())
}
