//! Privacy-minimized structured input to report narrators.

use crate::{
    DomainError, IntegratedAssessment, Metadata, ModelResult, RecordingId, ScaleResult,
    SignalQuality, Subject, SubjectId,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Subject fields needed to narrate a report, excluding direct identifiers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubjectSummary {
    pub subject_id: SubjectId,
    pub age: Option<u8>,
    pub sex: crate::Sex,
}

impl From<&Subject> for SubjectSummary {
    fn from(subject: &Subject) -> Self {
        Self {
            subject_id: subject.id,
            age: subject.age,
            sex: subject.sex.clone(),
        }
    }
}

/// Compact EEG values appropriate for report input; raw samples stay local.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EegFeatureSummary {
    pub recording_id: RecordingId,
    #[serde(default)]
    pub values: BTreeMap<String, f64>,
    #[serde(default)]
    pub notes: Vec<String>,
}

/// The only aggregate that should be passed to an LLM report narrator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReportContext {
    pub subject: SubjectSummary,
    pub signal_quality: Option<SignalQuality>,
    pub eeg_features: Option<EegFeatureSummary>,
    #[serde(default)]
    pub scale_results: Vec<ScaleResult>,
    #[serde(default)]
    pub model_results: Vec<ModelResult>,
    pub integrated_assessment: IntegratedAssessment,
    #[serde(default)]
    pub limitations: Vec<String>,
    #[serde(default)]
    pub metadata: Metadata,
}

impl ReportContext {
    pub fn new(
        subject: SubjectSummary,
        integrated_assessment: IntegratedAssessment,
    ) -> Result<Self, DomainError> {
        if subject.subject_id != integrated_assessment.subject_id {
            return Err(DomainError::EntityMismatch {
                entity: "integrated_assessment",
                expected: subject.subject_id.to_string(),
                actual: integrated_assessment.subject_id.to_string(),
            });
        }
        Ok(Self {
            subject,
            signal_quality: None,
            eeg_features: None,
            scale_results: Vec::new(),
            model_results: Vec::new(),
            integrated_assessment,
            limitations: Vec::new(),
            metadata: Metadata::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AssessmentId, EvidenceSet, Provenance, ReliabilityAssessment, ReliabilityLevel, Severity,
    };
    use chrono::Utc;

    fn assessment(subject_id: SubjectId) -> IntegratedAssessment {
        IntegratedAssessment::new(
            AssessmentId::new(),
            subject_id,
            None,
            Severity::Unknown,
            ReliabilityAssessment::new(ReliabilityLevel::Low),
            EvidenceSet::default(),
            Utc::now(),
            Provenance::new("0.1.0", "fusion", "1", Utc::now()).unwrap(),
        )
    }

    #[test]
    fn report_context_rejects_cross_subject_assessment() {
        let summary = SubjectSummary {
            subject_id: SubjectId::new(),
            age: None,
            sex: crate::Sex::Unknown,
        };
        let result = ReportContext::new(summary, assessment(SubjectId::new()));
        assert!(matches!(result, Err(DomainError::EntityMismatch { .. })));
    }

    #[test]
    fn report_context_json_contains_no_raw_eeg_samples() {
        let subject_id = SubjectId::new();
        let context = ReportContext::new(
            SubjectSummary {
                subject_id,
                age: Some(30),
                sex: crate::Sex::Unknown,
            },
            assessment(subject_id),
        )
        .unwrap();
        let json = serde_json::to_string(&context).unwrap();
        assert!(!json.contains("samples"));
    }
}
