//! Database-independent CRUD contracts.

use crate::{StorageResult, StoredRecordingMetadata};
use domain::{EegRecording, RecordingId, Subject, SubjectId};

/// Persistence operations for anonymized subjects.
///
/// Create and update are separate on purpose: accidentally overwriting an
/// existing subject is reported rather than silently accepted.
pub trait SubjectRepository: Send + Sync {
    fn create_subject(&self, subject: &Subject) -> StorageResult<()>;
    fn get_subject(&self, id: SubjectId) -> StorageResult<Option<Subject>>;
    fn list_subjects(&self) -> StorageResult<Vec<Subject>>;
    fn update_subject(&self, subject: &Subject) -> StorageResult<()>;
    fn delete_subject(&self, id: SubjectId) -> StorageResult<()>;
}

/// Persistence operations for EEG recordings.
///
/// Metadata-only reads never touch the binary sample file. Full reads restore
/// the canonical domain object, keeping storage details out of downstream EEG
/// processing crates.
pub trait RecordingRepository: Send + Sync {
    fn create_recording(&self, recording: &EegRecording) -> StorageResult<()>;
    fn get_recording(&self, id: RecordingId) -> StorageResult<Option<EegRecording>>;
    fn get_recording_metadata(
        &self,
        id: RecordingId,
    ) -> StorageResult<Option<StoredRecordingMetadata>>;
    fn list_recording_metadata(
        &self,
        subject_id: SubjectId,
    ) -> StorageResult<Vec<StoredRecordingMetadata>>;
    fn update_recording(&self, recording: &EegRecording) -> StorageResult<()>;
    fn delete_recording(&self, id: RecordingId) -> StorageResult<()>;
}
