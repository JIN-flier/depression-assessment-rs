//! Persistent project storage for subjects and EEG recordings.
//!
//! The crate deliberately exposes repository traits in addition to the redb
//! implementation. Application and UI code can therefore depend on the CRUD
//! contracts without depending on a concrete database. redb stores searchable
//! metadata, while the much larger sample matrices live in recording-specific
//! files below the project directory.

#![forbid(unsafe_code)]

mod error;
mod model;
mod project;
mod repository;
mod sample_file;

pub use error::{EntityKind, StorageError, StorageResult};
pub use model::StoredRecordingMetadata;
pub use project::{DATABASE_FILE_NAME, ProjectStorage, RECORDINGS_DIRECTORY_NAME};
pub use repository::{RecordingRepository, SubjectRepository};
