//! Data-driven questionnaire results shared with the future scale engine.

use crate::{
    DomainError, Metadata, ScaleResultId, SubjectId, UnitInterval, VisitId, types::required,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Evaluated result of one scale administration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScaleResult {
    pub id: ScaleResultId,
    pub subject_id: SubjectId,
    pub visit_id: Option<VisitId>,
    pub scale_id: String,
    pub version: String,
    pub total_score: f64,
    #[serde(default)]
    pub dimension_scores: BTreeMap<String, f64>,
    pub completed_at: DateTime<Utc>,
    pub completeness: UnitInterval,
    #[serde(default)]
    pub metadata: Metadata,
}

impl ScaleResult {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: ScaleResultId,
        subject_id: SubjectId,
        visit_id: Option<VisitId>,
        scale_id: impl Into<String>,
        version: impl Into<String>,
        total_score: f64,
        completed_at: DateTime<Utc>,
        completeness: UnitInterval,
    ) -> Result<Self, DomainError> {
        if !total_score.is_finite() {
            return Err(DomainError::NonFinite {
                field: "scale.total_score",
                value: total_score,
            });
        }
        Ok(Self {
            id,
            subject_id,
            visit_id,
            scale_id: required("scale.scale_id", scale_id)?,
            version: required("scale.version", version)?,
            total_score,
            dimension_scores: BTreeMap::new(),
            completed_at,
            completeness,
            metadata: Metadata::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_identity_and_version_are_required() {
        let result = ScaleResult::new(
            ScaleResultId::new(),
            SubjectId::new(),
            None,
            " ",
            "1.0",
            5.0,
            Utc::now(),
            UnitInterval::ONE,
        );
        assert!(matches!(result, Err(DomainError::EmptyField { .. })));
    }
}
