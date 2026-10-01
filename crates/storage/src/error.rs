//! Error types shared by repository interfaces and implementations.

use std::{io, path::PathBuf};
use thiserror::Error;

/// Persistent entity categories used in actionable storage errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityKind {
    Subject,
    Recording,
}

impl std::fmt::Display for EntityKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Subject => formatter.write_str("subject"),
            Self::Recording => formatter.write_str("recording"),
        }
    }
}

/// Failures produced by a storage repository.
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("{entity} {id} already exists")]
    AlreadyExists { entity: EntityKind, id: String },

    #[error("{entity} {id} was not found")]
    NotFound { entity: EntityKind, id: String },

    #[error("recording {recording_id} references missing subject {subject_id}")]
    MissingSubject {
        recording_id: String,
        subject_id: String,
    },

    #[error("subject {subject_id} still owns one or more recordings")]
    SubjectHasRecordings { subject_id: String },

    #[error("database operation failed: {0}")]
    Database(String),

    #[error("could not encode or decode persisted JSON: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("I/O operation failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("persisted {entity} {id} is corrupt: {reason}")]
    CorruptData {
        entity: EntityKind,
        id: String,
        reason: String,
    },
}

impl StorageError {
    pub(crate) fn database(error: impl std::fmt::Display) -> Self {
        Self::Database(error.to_string())
    }

    pub(crate) fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

/// Convenience result used by all storage contracts.
pub type StorageResult<T> = Result<T, StorageError>;
