//! Normalized evidence exchanged by scale, EEG, model, and fusion modules.

use crate::{DomainError, Metadata, UnitInterval, types::required};
use serde::{Deserialize, Serialize};

/// Origin category of an evidence item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    Eeg,
    Scale,
    Model,
    Longitudinal,
}

/// Direction in which a finding moves the interpreted assessment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceDirection {
    Supports,
    Opposes,
    Neutral,
}

/// One traceable input to deterministic evidence fusion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceItem {
    pub kind: EvidenceKind,
    /// Identifier of the result or algorithm output that produced this item.
    pub source_id: String,
    pub finding: String,
    pub observed_value: Option<f64>,
    pub unit: Option<String>,
    pub direction: EvidenceDirection,
    /// Signed contribution assigned by a validated fusion model, if available.
    pub contribution: Option<f64>,
    pub confidence: Option<UnitInterval>,
    #[serde(default)]
    pub metadata: Metadata,
}

impl EvidenceItem {
    pub fn new(
        kind: EvidenceKind,
        source_id: impl Into<String>,
        finding: impl Into<String>,
        direction: EvidenceDirection,
    ) -> Result<Self, DomainError> {
        Ok(Self {
            kind,
            source_id: required("evidence.source_id", source_id)?,
            finding: required("evidence.finding", finding)?,
            observed_value: None,
            unit: None,
            direction,
            contribution: None,
            confidence: None,
            metadata: Metadata::new(),
        })
    }
}

/// Evidence separated by modality so missing modalities remain explicit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct EvidenceSet {
    #[serde(default)]
    pub eeg_evidence: Vec<EvidenceItem>,
    #[serde(default)]
    pub scale_evidence: Vec<EvidenceItem>,
    #[serde(default)]
    pub model_evidence: Vec<EvidenceItem>,
    #[serde(default)]
    pub longitudinal_evidence: Vec<EvidenceItem>,
}

impl EvidenceSet {
    /// Adds an item to the collection matching its declared origin.
    pub fn push(&mut self, item: EvidenceItem) {
        match item.kind {
            EvidenceKind::Eeg => self.eeg_evidence.push(item),
            EvidenceKind::Scale => self.scale_evidence.push(item),
            EvidenceKind::Model => self.model_evidence.push(item),
            EvidenceKind::Longitudinal => self.longitudinal_evidence.push(item),
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.eeg_evidence.len()
            + self.scale_evidence.len()
            + self.model_evidence.len()
            + self.longitudinal_evidence.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_is_routed_by_modality() {
        let mut set = EvidenceSet::default();
        set.push(
            EvidenceItem::new(
                EvidenceKind::Scale,
                "phq9-result",
                "elevated score",
                EvidenceDirection::Supports,
            )
            .unwrap(),
        );
        assert_eq!(set.scale_evidence.len(), 1);
        assert_eq!(set.len(), 1);
    }
}
