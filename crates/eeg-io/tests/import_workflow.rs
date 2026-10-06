//! P3 contract tests: identical physical values across formats, and imported
//! recordings survive P2 persistence without a runtime dependency on storage.

mod support;

use chrono::{TimeZone, Utc};
use domain::{Sex, Subject};
use eeg_io::{CsvOptions, CsvReader, EdfReader, EegIoError, EegReader};
use std::io::{self, Read};
use storage::{ProjectStorage, RecordingRepository, SubjectRepository};
use support::*;
use tempfile::tempdir;

const GOLDEN_CSV: &[u8] = include_bytes!("data/golden.csv");
const GOLDEN_EDF: &[u8] = include_bytes!("data/golden.edf");
const GOLDEN_MAT: &[u8] = include_bytes!("data/golden.mat");
const GOLDEN_COMPRESSED_MAT: &[u8] = include_bytes!("data/golden-compressed.mat");

#[test]
fn golden_formats_produce_identical_domain_samples() {
    let ctx = context();
    let readers: Vec<(Box<dyn EegReader>, &[u8])> = vec![
        (Box::new(CsvReader::new(CsvOptions::new(2.0))), GOLDEN_CSV),
        (Box::new(EdfReader::default()), GOLDEN_EDF),
        (Box::new(mat_reader()), GOLDEN_MAT),
        (Box::new(mat_reader()), GOLDEN_COMPRESSED_MAT),
    ];
    for (reader, mut bytes) in readers {
        let recording = reader.read(&mut bytes, &ctx).unwrap();
        assert_eq!(
            recording.samples,
            vec![vec![1.0, 2.0, 3.0, 4.0], vec![-1.0, -2.0, -3.0, -4.0]]
        );
        assert_eq!(recording.channels, channels());
        assert_eq!(recording.sampling_rate_hz, 2.0);
        assert_eq!(recording.duration_seconds, 2.0);
        assert_eq!(recording.id, ctx.recording_id);
        assert_eq!(recording.subject_id, ctx.subject_id);
    }
}

#[test]
fn imported_files_round_trip_through_storage_across_reopen() {
    let readers: Vec<(Box<dyn EegReader>, &[u8], &str)> = vec![
        (
            Box::new(CsvReader::new(CsvOptions::new(2.0))),
            GOLDEN_CSV,
            "csv",
        ),
        (Box::new(EdfReader::default()), GOLDEN_EDF, "edf"),
        (Box::new(mat_reader()), GOLDEN_COMPRESSED_MAT, "mat"),
    ];
    for (reader, bytes, suffix) in readers {
        let directory = tempdir().unwrap();
        let path = directory.path().join(format!("input.{suffix}"));
        std::fs::write(&path, bytes).unwrap();
        let ctx = context();
        let recording = reader.read_path(&path, &ctx).unwrap();
        let subject = Subject::new(
            ctx.subject_id,
            None,
            Sex::Unknown,
            Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        )
        .unwrap();
        {
            let storage = ProjectStorage::open(directory.path()).unwrap();
            storage.create_subject(&subject).unwrap();
            storage.create_recording(&recording).unwrap();
        }
        let storage = ProjectStorage::open(directory.path()).unwrap();
        assert_eq!(
            storage.get_recording(recording.id).unwrap(),
            Some(recording.clone())
        );
        assert_eq!(
            storage
                .get_recording_metadata(recording.id)
                .unwrap()
                .unwrap()
                .sample_count,
            4
        );
    }
}

struct FailedInput;
impl Read for FailedInput {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("synthetic failure"))
    }
}

#[test]
fn adapters_propagate_io_failures_and_do_not_require_a_file_path() {
    let readers: Vec<Box<dyn EegReader>> = vec![
        Box::new(CsvReader::new(CsvOptions::new(2.0))),
        Box::new(EdfReader::default()),
        Box::new(mat_reader()),
    ];
    for reader in readers {
        assert!(matches!(
            reader.read(&mut FailedInput, &context()),
            Err(EegIoError::Io(_))
        ));
    }
}

#[test]
fn edf_import_does_not_copy_patient_and_recording_identifiers() {
    let mut bytes = GOLDEN_EDF.to_vec();
    put_field(&mut bytes, 8, 80, "private-patient-name");
    put_field(&mut bytes, 88, 80, "private-investigator-name");
    let recording = EdfReader::default()
        .read(&mut bytes.as_slice(), &context())
        .unwrap();
    let text = format!("{:?}", recording.metadata);
    assert!(!text.contains("private-patient-name"));
    assert!(!text.contains("private-investigator-name"));
    assert!(recording.metadata.recorded_at.is_none());
    assert_eq!(recording.metadata.extra["edf.start_date_local"], "01.01.26");
}
