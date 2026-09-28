//! Contract test covering the data flow between independently implemented crates.

use chrono::{TimeZone, Utc};
use domain::*;
use std::collections::BTreeMap;

#[test]
fn v1_to_v3_domain_contract_round_trips_as_json() {
    let now = Utc.with_ymd_and_hms(2026, 9, 26, 8, 0, 0).unwrap();
    let subject = Subject::new(SubjectId::new(), Some(32), Sex::Female, now).unwrap();
    let recording = EegRecording::new(
        RecordingId::new(),
        subject.id,
        250.0,
        vec![Channel::new("Fp1", ChannelKind::Eeg, "uV").unwrap()],
        vec![vec![0.0; 500]],
        RecordingState::Raw,
        RecordingMetadata {
            recorded_at: Some(now),
            device: Some("research-amplifier".into()),
            electrode_system: Some("10-20".into()),
            ..RecordingMetadata::default()
        },
    )
    .unwrap();

    let provenance = Provenance::new(env!("CARGO_PKG_VERSION"), "welch", "welch-v1", now).unwrap();
    let spectral = SpectralFeatures::new(
        vec![1.0, 2.0],
        BTreeMap::from([("Fp1".into(), vec![0.1, 0.2])]),
        BTreeMap::from([(
            "Fp1".into(),
            BTreeMap::from([(
                FrequencyBand::Alpha,
                BandPower {
                    absolute: 2.4,
                    relative: UnitInterval::new("relative_power", 0.4).unwrap(),
                },
            )]),
        )]),
    )
    .unwrap();
    let features = EegFeatures::spectral(recording.id, spectral, provenance.clone());
    assert_eq!(features.recording_id, recording.id);

    let mut evidence = EvidenceSet::default();
    evidence.push(
        EvidenceItem::new(
            EvidenceKind::Eeg,
            recording.id.to_string(),
            "alpha relative power available",
            EvidenceDirection::Neutral,
        )
        .unwrap(),
    );
    let assessment = IntegratedAssessment::new(
        AssessmentId::new(),
        subject.id,
        None,
        Severity::Unknown,
        ReliabilityAssessment::new(ReliabilityLevel::Low),
        evidence,
        now,
        provenance,
    );
    let mut context = ReportContext::new(SubjectSummary::from(&subject), assessment).unwrap();
    context.eeg_features = Some(EegFeatureSummary {
        recording_id: recording.id,
        values: BTreeMap::from([("alpha_relative_power".into(), 0.4)]),
        notes: vec![],
    });

    let json = serde_json::to_string(&context).unwrap();
    let decoded: ReportContext = serde_json::from_str(&json).unwrap();

    assert_eq!(decoded, context);
    assert!(!json.contains("research-amplifier"));
    assert!(!json.contains("samples"));
    assert!(!decoded.integrated_assessment.conclusion_allowed);
}
