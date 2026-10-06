//! P3 -> P4 -> P2 integration runs headlessly; IO/storage are dev dependencies only.
mod support;
use domain::{Sex, Subject};
use eeg_io::{CsvOptions, CsvReader, EegReader, ImportContext};
use eeg_signal::{FilterPhase, SignalStep};
use serde::Deserialize;
use storage::{ProjectStorage, RecordingRepository, SubjectRepository};
use support::*;

#[test]
fn csv_import_processing_and_storage_preserve_provenance_across_reopen() {
    let mut csv = String::from("Fp1,Fp2\n");
    for (a, b) in sine(500.0, 10.0, 1000).iter().zip(sine(500.0, 20.0, 1000)) {
        csv.push_str(&format!("{},{}\n", a + 10.0, b - 20.0));
    }
    let import = ImportContext::new(domain::RecordingId::new(), domain::SubjectId::new());
    let raw = CsvReader::new(CsvOptions::new(500.0))
        .read(&mut csv.as_bytes(), &import)
        .unwrap();
    let processed = pipeline(vec![
        SignalStep::DcRemoval {},
        SignalStep::bandpass(1.0, 45.0),
        SignalStep::notch(50.0),
        SignalStep::resample(250.0),
    ])
    .process(&raw, &context())
    .unwrap();
    assert_eq!(processed.metadata.source_format.as_deref(), Some("csv"));
    assert_eq!(processed.metadata.extra, raw.metadata.extra);
    assert_eq!(processed.channels[0].unit, "uV");
    assert!(raw.processing_history.is_empty());
    assert_eq!(processed.processing_history.len(), 4);
    let directory = tempfile::tempdir().unwrap();
    {
        let storage = ProjectStorage::open(directory.path()).unwrap();
        storage
            .create_subject(
                &Subject::new(import.subject_id, None, Sex::Unknown, context().applied_at).unwrap(),
            )
            .unwrap();
        storage.create_recording(&raw).unwrap();
        storage.update_recording(&processed).unwrap();
    }
    let storage = ProjectStorage::open(directory.path()).unwrap();
    assert_eq!(storage.get_recording(raw.id).unwrap().unwrap(), processed);
    let metadata = storage.get_recording_metadata(raw.id).unwrap().unwrap();
    assert_eq!(metadata.sample_count, 500);
    assert_eq!(metadata.processing_history, processed.processing_history);
}

#[test]
fn analytic_notch_impulse_golden_is_stable() {
    #[derive(Deserialize)]
    struct Golden {
        input: Vec<f32>,
        expected: Vec<f32>,
    }
    let fixture: Golden = serde_json::from_str(include_str!("data/notch-impulse.json")).unwrap();
    let input = recording(200.0, vec![fixture.input]);
    let output = pipeline(vec![SignalStep::Notch {
        frequency_hz: 50.0,
        q: 2.0,
        phase: FilterPhase::Causal,
    }])
    .process(&input, &context())
    .unwrap();
    for (actual, expected) in output.samples[0].iter().zip(fixture.expected) {
        assert!((actual - expected).abs() < 1e-7);
    }
}
