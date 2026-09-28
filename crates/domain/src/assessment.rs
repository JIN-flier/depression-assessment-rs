//! Integrated assessment and reliability values produced by deterministic engines.

use crate::{
    AssessmentId, EvidenceSet, Metadata, Provenance, SubjectId, UnitInterval, VisitId, Warning,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Clinical wording level used by structured results and report validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    None,
    Minimal,
    Mild,
    Moderate,
    Severe,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReliabilityLevel {
    Low,
    Moderate,
    High,
}

/// Named reliability dimensions prescribed by the research plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReliabilityDimension {
    EegQuality,
    ModelConfidence,
    OodRisk,
    ScaleCompleteness,
    HistoricalConsistency,
}

/// Explainable output of the reliability engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReliabilityAssessment {
    pub overall: ReliabilityLevel,
    #[serde(default)]
    pub dimensions: BTreeMap<ReliabilityDimension, ReliabilityLevel>,
    #[serde(default)]
    pub reasons: Vec<String>,
}

impl ReliabilityAssessment {
    #[must_use]
    pub fn new(overall: ReliabilityLevel) -> Self {
        Self {
            overall,
            dimensions: BTreeMap::new(),
            reasons: Vec::new(),
        }
    }
}

/// Structured conclusion fixed before any LLM narration occurs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntegratedAssessment {
    pub id: AssessmentId,
    pub subject_id: SubjectId,
    pub visit_id: Option<VisitId>,
    pub severity: Severity,
    pub risk: Option<UnitInterval>,
    pub reliability: ReliabilityAssessment,
    pub evidence: EvidenceSet,
    #[serde(default)]
    pub warnings: Vec<Warning>,
    #[serde(default)]
    pub limitations: Vec<String>,
    /// Only a safety engine may set this to true for a strong conclusion.
    pub conclusion_allowed: bool,
    pub assessed_at: DateTime<Utc>,
    pub provenance: Provenance,
    #[serde(default)]
    pub metadata: Metadata,
}

impl IntegratedAssessment {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        id: AssessmentId,
        subject_id: SubjectId,
        visit_id: Option<VisitId>,
        severity: Severity,
        reliability: ReliabilityAssessment,
        evidence: EvidenceSet,
        assessed_at: DateTime<Utc>,
        provenance: Provenance,
    ) -> Self {
        Self {
            id,
            subject_id,
            visit_id,
            severity,
            risk: None,
            reliability,
            evidence,
            warnings: Vec::new(),
            limitations: Vec::new(),
            conclusion_allowed: false,
            assessed_at,
            provenance,
            metadata: Metadata::new(),
        }
    }
}
