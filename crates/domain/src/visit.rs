//! Visit aggregate linking persisted outputs without coupling to storage.

use crate::{
    AssessmentId, Metadata, ModelResultId, RecordingId, ScaleResultId, SubjectId, VisitId,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A longitudinal assessment time point for one subject.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Visit {
    pub id: VisitId,
    pub subject_id: SubjectId,
    pub occurred_at: DateTime<Utc>,
    #[serde(default)]
    pub recording_ids: Vec<RecordingId>,
    #[serde(default)]
    pub scale_result_ids: Vec<ScaleResultId>,
    #[serde(default)]
    pub model_result_ids: Vec<ModelResultId>,
    pub assessment_id: Option<AssessmentId>,
    #[serde(default)]
    pub metadata: Metadata,
}

impl Visit {
    #[must_use]
    pub fn new(id: VisitId, subject_id: SubjectId, occurred_at: DateTime<Utc>) -> Self {
        Self {
            id,
            subject_id,
            occurred_at,
            recording_ids: Vec::new(),
            scale_result_ids: Vec::new(),
            model_result_ids: Vec::new(),
            assessment_id: None,
            metadata: Metadata::new(),
        }
    }
}
