//! Compact recording representation stored inside redb.

use domain::{
    Channel, EegRecording, ProcessingStep, RecordingId, RecordingMetadata, RecordingState,
    SubjectId,
};
use serde::{Deserialize, Serialize};

/// Recording data that can be listed without reading a potentially large
/// sample payload.
///
/// `sample_file_name` is intentionally private: callers should not couple
/// themselves to the current filesystem layout. The repository uses it to
/// reconstruct a full [`EegRecording`] when requested.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredRecordingMetadata {
    pub id: RecordingId,
    pub subject_id: SubjectId,
    pub sampling_rate_hz: f64,
    pub channels: Vec<Channel>,
    pub sample_count: usize,
    pub recording_state: RecordingState,
    pub duration_seconds: f64,
    pub processing_history: Vec<ProcessingStep>,
    pub metadata: RecordingMetadata,
    sample_file_name: String,
}

impl StoredRecordingMetadata {
    pub(crate) fn from_recording(recording: &EegRecording, sample_file_name: String) -> Self {
        Self {
            id: recording.id,
            subject_id: recording.subject_id,
            sampling_rate_hz: recording.sampling_rate_hz,
            channels: recording.channels.clone(),
            sample_count: recording.sample_count(),
            recording_state: recording.recording_state,
            duration_seconds: recording.duration_seconds,
            processing_history: recording.processing_history.clone(),
            metadata: recording.metadata.clone(),
            sample_file_name,
        }
    }

    pub(crate) fn sample_file_name(&self) -> &str {
        &self.sample_file_name
    }
}
