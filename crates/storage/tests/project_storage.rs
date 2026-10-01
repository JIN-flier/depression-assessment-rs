use chrono::{TimeZone, Utc};
use domain::{
    Channel, ChannelKind, EegRecording, RecordingId, RecordingMetadata, RecordingState, Sex,
    Subject, SubjectId,
};
use std::fs;
use storage::{
    DATABASE_FILE_NAME, EntityKind, ProjectStorage, RECORDINGS_DIRECTORY_NAME, RecordingRepository,
    StorageError, SubjectRepository,
};
use tempfile::tempdir;

fn subject() -> Subject {
    Subject::new(
        SubjectId::new(),
        Some(29),
        Sex::Unknown,
        Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap(),
    )
    .unwrap()
}

fn recording(subject_id: SubjectId) -> EegRecording {
    EegRecording::new(
        RecordingId::new(),
        subject_id,
        2.0,
        vec![
            Channel::new("Fp1", ChannelKind::Eeg, "uV").unwrap(),
            Channel::new("Fp2", ChannelKind::Eeg, "uV").unwrap(),
        ],
        vec![vec![1.0, 2.0, 3.0, 4.0], vec![-1.0, -2.0, -3.0, -4.0]],
        RecordingState::Raw,
        RecordingMetadata::default(),
    )
    .unwrap()
}

#[test]
fn subject_crud_persists_across_reopen() {
    let directory = tempdir().unwrap();
    let original = subject();
    {
        let storage = ProjectStorage::open(directory.path()).unwrap();
        storage.create_subject(&original).unwrap();
        assert_eq!(
            storage.get_subject(original.id).unwrap(),
            Some(original.clone())
        );

        let mut updated = original.clone();
        updated.age = Some(30);
        storage.update_subject(&updated).unwrap();
        assert_eq!(storage.list_subjects().unwrap(), vec![updated]);
    }

    let reopened = ProjectStorage::open(directory.path()).unwrap();
    assert_eq!(
        reopened.get_subject(original.id).unwrap().unwrap().age,
        Some(30)
    );
    reopened.delete_subject(original.id).unwrap();
    assert_eq!(reopened.get_subject(original.id).unwrap(), None);
}

#[test]
fn create_and_update_report_explicit_crud_errors() {
    let directory = tempdir().unwrap();
    let storage = ProjectStorage::open(directory.path()).unwrap();
    let value = subject();

    storage.create_subject(&value).unwrap();
    assert!(matches!(
        storage.create_subject(&value),
        Err(StorageError::AlreadyExists {
            entity: EntityKind::Subject,
            ..
        })
    ));

    let missing = subject();
    assert!(matches!(
        storage.update_subject(&missing),
        Err(StorageError::NotFound {
            entity: EntityKind::Subject,
            ..
        })
    ));
}

#[test]
fn recording_metadata_and_samples_use_separate_storage() {
    let directory = tempdir().unwrap();
    let owner = subject();
    let value = recording(owner.id);
    {
        let storage = ProjectStorage::open(directory.path()).unwrap();
        storage.create_subject(&owner).unwrap();
        storage.create_recording(&value).unwrap();
    }

    // A fresh database handle proves both redb metadata and the external sample
    // payload survive beyond the lifetime of the writer.
    let storage = ProjectStorage::open(directory.path()).unwrap();
    let metadata = storage.get_recording_metadata(value.id).unwrap().unwrap();
    assert_eq!(metadata.sample_count, 4);
    assert_eq!(metadata.channels, value.channels);
    assert_eq!(
        storage.get_recording(value.id).unwrap(),
        Some(value.clone())
    );

    let recording_directory = directory
        .path()
        .join(RECORDINGS_DIRECTORY_NAME)
        .join(value.id.to_string());
    assert_eq!(fs::read_dir(recording_directory).unwrap().count(), 1);
    assert!(directory.path().join(DATABASE_FILE_NAME).is_file());
}

#[test]
fn recording_update_replaces_samples_and_metadata() {
    let directory = tempdir().unwrap();
    let storage = ProjectStorage::open(directory.path()).unwrap();
    let owner = subject();
    let mut value = recording(owner.id);
    storage.create_subject(&owner).unwrap();
    storage.create_recording(&value).unwrap();

    value.samples = vec![vec![10.0, 20.0], vec![30.0, 40.0]];
    value.duration_seconds = 1.0;
    value.metadata.device = Some("updated-device".to_owned());
    storage.update_recording(&value).unwrap();

    assert_eq!(
        storage.get_recording(value.id).unwrap(),
        Some(value.clone())
    );
    let payload_count = fs::read_dir(
        directory
            .path()
            .join(RECORDINGS_DIRECTORY_NAME)
            .join(value.id.to_string()),
    )
    .unwrap()
    .count();
    assert_eq!(payload_count, 1, "the superseded sample payload is removed");
}

#[test]
fn recording_requires_an_existing_subject_and_rolls_back_its_file() {
    let directory = tempdir().unwrap();
    let storage = ProjectStorage::open(directory.path()).unwrap();
    let value = recording(SubjectId::new());

    assert!(matches!(
        storage.create_recording(&value),
        Err(StorageError::MissingSubject { .. })
    ));
    let recording_directory = directory
        .path()
        .join(RECORDINGS_DIRECTORY_NAME)
        .join(value.id.to_string());
    assert_eq!(fs::read_dir(recording_directory).unwrap().count(), 0);
}

#[test]
fn subject_delete_is_restricted_until_recordings_are_deleted() {
    let directory = tempdir().unwrap();
    let storage = ProjectStorage::open(directory.path()).unwrap();
    let owner = subject();
    let value = recording(owner.id);
    storage.create_subject(&owner).unwrap();
    storage.create_recording(&value).unwrap();

    assert!(matches!(
        storage.delete_subject(owner.id),
        Err(StorageError::SubjectHasRecordings { .. })
    ));
    storage.delete_recording(value.id).unwrap();
    storage.delete_subject(owner.id).unwrap();
}

#[test]
fn recording_lists_are_scoped_to_the_requested_subject() {
    let directory = tempdir().unwrap();
    let storage = ProjectStorage::open(directory.path()).unwrap();
    let first = subject();
    let second = subject();
    storage.create_subject(&first).unwrap();
    storage.create_subject(&second).unwrap();
    storage.create_recording(&recording(first.id)).unwrap();
    storage.create_recording(&recording(second.id)).unwrap();

    let first_recordings = storage.list_recording_metadata(first.id).unwrap();
    assert_eq!(first_recordings.len(), 1);
    assert_eq!(first_recordings[0].subject_id, first.id);
}
