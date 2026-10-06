//! Import failures retain their layer and location; no malformed input panics.

use domain::DomainError;
use thiserror::Error;

pub type EegIoResult<T> = Result<T, EegIoError>;

#[derive(Debug, Error)]
pub enum EegIoError {
    #[error("EEG input I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid import configuration: {0}")]
    Configuration(String),
    #[error("invalid {format} input: {reason}")]
    InvalidData {
        format: &'static str,
        reason: String,
    },
    #[error("unsupported EEG input: {0}")]
    Unsupported(String),
    #[error("import exceeds limit: {0}")]
    LimitExceeded(&'static str),
    #[error("CSV parsing failed: {0}")]
    Csv(#[from] csv::Error),
    #[error("MAT parsing failed: {0}")]
    Mat(#[from] matfile::Error),
    #[error("imported recording violates domain invariant: {0}")]
    Domain(#[from] DomainError),
}

pub(crate) fn invalid(format: &'static str, reason: impl Into<String>) -> EegIoError {
    EegIoError::InvalidData {
        format,
        reason: reason.into(),
    }
}
