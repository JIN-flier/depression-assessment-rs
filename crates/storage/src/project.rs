//! redb-backed implementation of the storage repository contracts.

use crate::{
    EntityKind, RecordingRepository, StorageError, StorageResult, StoredRecordingMetadata,
    SubjectRepository, sample_file,
};
use domain::{EegRecording, RecordingId, Subject, SubjectId};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

pub const DATABASE_FILE_NAME: &str = "project.redb";
pub const RECORDINGS_DIRECTORY_NAME: &str = "recordings";

const SUBJECTS: TableDefinition<&str, &[u8]> = TableDefinition::new("subjects");
const RECORDINGS: TableDefinition<&str, &[u8]> = TableDefinition::new("recordings");

/// A clonable handle to one assessment project on disk.
///
/// Clones share the same database handle; redb serializes writes while allowing
/// concurrent reads. Every public operation uses a separate transaction so a
/// failed operation cannot leave partially written database rows.
#[derive(Clone)]
pub struct ProjectStorage {
    inner: Arc<Inner>,
}

struct Inner {
    database: Database,
    recordings_directory: PathBuf,
}

impl ProjectStorage {
    /// Opens an existing project or creates the directory, database, and tables
    /// needed by a new one.
    pub fn open(project_directory: impl AsRef<Path>) -> StorageResult<Self> {
        let project_directory = project_directory.as_ref();
        fs::create_dir_all(project_directory)
            .map_err(|error| StorageError::io(project_directory, error))?;

        let recordings_directory = project_directory.join(RECORDINGS_DIRECTORY_NAME);
        fs::create_dir_all(&recordings_directory)
            .map_err(|error| StorageError::io(&recordings_directory, error))?;

        let database_path = project_directory.join(DATABASE_FILE_NAME);
        let database = Database::create(&database_path).map_err(StorageError::database)?;

        // redb creates tables lazily. Initializing both here means later read
        // transactions can always open them, including for an empty project.
        let write = database.begin_write().map_err(StorageError::database)?;
        write.open_table(SUBJECTS).map_err(StorageError::database)?;
        write
            .open_table(RECORDINGS)
            .map_err(StorageError::database)?;
        write.commit().map_err(StorageError::database)?;

        Ok(Self {
            inner: Arc::new(Inner {
                database,
                recordings_directory,
            }),
        })
    }

    fn recording_directory(&self, id: RecordingId) -> PathBuf {
        self.inner.recordings_directory.join(id.to_string())
    }

    fn sample_path(&self, metadata: &StoredRecordingMetadata) -> StorageResult<PathBuf> {
        let id = metadata.id.to_string();
        sample_file::validate_file_name(metadata.sample_file_name(), &id)?;
        Ok(self
            .recording_directory(metadata.id)
            .join(metadata.sample_file_name()))
    }

    fn write_new_sample_payload(
        &self,
        recording: &EegRecording,
    ) -> StorageResult<(String, PathBuf)> {
        let directory = self.recording_directory(recording.id);
        fs::create_dir_all(&directory).map_err(|error| StorageError::io(&directory, error))?;

        // A versioned filename makes updates atomic from the database's point
        // of view: readers see either the old committed payload or the new one.
        let file_name = format!("samples-{}.bin", RecordingId::new());
        let path = directory.join(&file_name);
        sample_file::write(&path, &recording.samples)?;
        Ok((file_name, path))
    }

    fn recording_metadata_in_write(
        table: &redb::Table<'_, &str, &[u8]>,
        id: RecordingId,
    ) -> StorageResult<Option<StoredRecordingMetadata>> {
        let key = id.to_string();
        table
            .get(key.as_str())
            .map_err(StorageError::database)?
            .map(|value| serde_json::from_slice(value.value()).map_err(StorageError::from))
            .transpose()
    }

    fn subject_exists_in_write(
        table: &redb::Table<'_, &str, &[u8]>,
        id: SubjectId,
    ) -> StorageResult<bool> {
        let key = id.to_string();
        table
            .get(key.as_str())
            .map(|value| value.is_some())
            .map_err(StorageError::database)
    }
}

impl SubjectRepository for ProjectStorage {
    fn create_subject(&self, subject: &Subject) -> StorageResult<()> {
        let key = subject.id.to_string();
        let value = serde_json::to_vec(subject)?;
        let write = self
            .inner
            .database
            .begin_write()
            .map_err(StorageError::database)?;
        {
            let mut table = write.open_table(SUBJECTS).map_err(StorageError::database)?;
            if table
                .get(key.as_str())
                .map_err(StorageError::database)?
                .is_some()
            {
                return Err(StorageError::AlreadyExists {
                    entity: EntityKind::Subject,
                    id: key,
                });
            }
            table
                .insert(key.as_str(), value.as_slice())
                .map_err(StorageError::database)?;
        }
        write.commit().map_err(StorageError::database)
    }

    fn get_subject(&self, id: SubjectId) -> StorageResult<Option<Subject>> {
        let key = id.to_string();
        let read = self
            .inner
            .database
            .begin_read()
            .map_err(StorageError::database)?;
        let table = read.open_table(SUBJECTS).map_err(StorageError::database)?;
        table
            .get(key.as_str())
            .map_err(StorageError::database)?
            .map(|value| serde_json::from_slice(value.value()).map_err(StorageError::from))
            .transpose()
    }

    fn list_subjects(&self) -> StorageResult<Vec<Subject>> {
        let read = self
            .inner
            .database
            .begin_read()
            .map_err(StorageError::database)?;
        let table = read.open_table(SUBJECTS).map_err(StorageError::database)?;
        table
            .iter()
            .map_err(StorageError::database)?
            .map(|entry| {
                let (_, value) = entry.map_err(StorageError::database)?;
                serde_json::from_slice(value.value()).map_err(StorageError::from)
            })
            .collect()
    }

    fn update_subject(&self, subject: &Subject) -> StorageResult<()> {
        let key = subject.id.to_string();
        let value = serde_json::to_vec(subject)?;
        let write = self
            .inner
            .database
            .begin_write()
            .map_err(StorageError::database)?;
        {
            let mut table = write.open_table(SUBJECTS).map_err(StorageError::database)?;
            if table
                .get(key.as_str())
                .map_err(StorageError::database)?
                .is_none()
            {
                return Err(StorageError::NotFound {
                    entity: EntityKind::Subject,
                    id: key,
                });
            }
            table
                .insert(key.as_str(), value.as_slice())
                .map_err(StorageError::database)?;
        }
        write.commit().map_err(StorageError::database)
    }

    fn delete_subject(&self, id: SubjectId) -> StorageResult<()> {
        let key = id.to_string();
        let write = self
            .inner
            .database
            .begin_write()
            .map_err(StorageError::database)?;
        {
            let recordings = write
                .open_table(RECORDINGS)
                .map_err(StorageError::database)?;
            for entry in recordings.iter().map_err(StorageError::database)? {
                let (_, value) = entry.map_err(StorageError::database)?;
                let metadata: StoredRecordingMetadata = serde_json::from_slice(value.value())?;
                if metadata.subject_id == id {
                    return Err(StorageError::SubjectHasRecordings { subject_id: key });
                }
            }
            drop(recordings);

            let mut subjects = write.open_table(SUBJECTS).map_err(StorageError::database)?;
            if subjects
                .remove(key.as_str())
                .map_err(StorageError::database)?
                .is_none()
            {
                return Err(StorageError::NotFound {
                    entity: EntityKind::Subject,
                    id: key,
                });
            }
        }
        write.commit().map_err(StorageError::database)
    }
}

impl RecordingRepository for ProjectStorage {
    fn create_recording(&self, recording: &EegRecording) -> StorageResult<()> {
        let (file_name, sample_path) = self.write_new_sample_payload(recording)?;
        let result = (|| {
            let metadata = StoredRecordingMetadata::from_recording(recording, file_name);
            let key = recording.id.to_string();
            let value = serde_json::to_vec(&metadata)?;
            let write = self
                .inner
                .database
                .begin_write()
                .map_err(StorageError::database)?;
            {
                let subjects = write.open_table(SUBJECTS).map_err(StorageError::database)?;
                if !Self::subject_exists_in_write(&subjects, recording.subject_id)? {
                    return Err(StorageError::MissingSubject {
                        recording_id: key,
                        subject_id: recording.subject_id.to_string(),
                    });
                }
                drop(subjects);

                let mut recordings = write
                    .open_table(RECORDINGS)
                    .map_err(StorageError::database)?;
                if recordings
                    .get(key.as_str())
                    .map_err(StorageError::database)?
                    .is_some()
                {
                    return Err(StorageError::AlreadyExists {
                        entity: EntityKind::Recording,
                        id: key,
                    });
                }
                recordings
                    .insert(key.as_str(), value.as_slice())
                    .map_err(StorageError::database)?;
            }
            write.commit().map_err(StorageError::database)
        })();

        if result.is_err() {
            let _ = fs::remove_file(&sample_path);
        }
        result
    }

    fn get_recording(&self, id: RecordingId) -> StorageResult<Option<EegRecording>> {
        let Some(metadata) = self.get_recording_metadata(id)? else {
            return Ok(None);
        };
        let id_text = id.to_string();
        let samples = sample_file::read(
            &self.sample_path(&metadata)?,
            &id_text,
            metadata.channels.len(),
            metadata.sample_count,
        )?;
        let mut recording = EegRecording::new(
            metadata.id,
            metadata.subject_id,
            metadata.sampling_rate_hz,
            metadata.channels,
            samples,
            metadata.recording_state,
            metadata.metadata,
        )
        .map_err(|error| StorageError::CorruptData {
            entity: EntityKind::Recording,
            id: id_text,
            reason: error.to_string(),
        })?;
        recording.processing_history = metadata.processing_history;
        Ok(Some(recording))
    }

    fn get_recording_metadata(
        &self,
        id: RecordingId,
    ) -> StorageResult<Option<StoredRecordingMetadata>> {
        let key = id.to_string();
        let read = self
            .inner
            .database
            .begin_read()
            .map_err(StorageError::database)?;
        let table = read
            .open_table(RECORDINGS)
            .map_err(StorageError::database)?;
        table
            .get(key.as_str())
            .map_err(StorageError::database)?
            .map(|value| serde_json::from_slice(value.value()).map_err(StorageError::from))
            .transpose()
    }

    fn list_recording_metadata(
        &self,
        subject_id: SubjectId,
    ) -> StorageResult<Vec<StoredRecordingMetadata>> {
        let read = self
            .inner
            .database
            .begin_read()
            .map_err(StorageError::database)?;
        let table = read
            .open_table(RECORDINGS)
            .map_err(StorageError::database)?;
        let mut result = Vec::new();
        for entry in table.iter().map_err(StorageError::database)? {
            let (_, value) = entry.map_err(StorageError::database)?;
            let metadata: StoredRecordingMetadata = serde_json::from_slice(value.value())?;
            if metadata.subject_id == subject_id {
                result.push(metadata);
            }
        }
        Ok(result)
    }

    fn update_recording(&self, recording: &EegRecording) -> StorageResult<()> {
        let (file_name, sample_path) = self.write_new_sample_payload(recording)?;
        let result = (|| {
            let metadata = StoredRecordingMetadata::from_recording(recording, file_name);
            let key = recording.id.to_string();
            let value = serde_json::to_vec(&metadata)?;
            let write = self
                .inner
                .database
                .begin_write()
                .map_err(StorageError::database)?;
            let old_metadata;
            {
                let subjects = write.open_table(SUBJECTS).map_err(StorageError::database)?;
                if !Self::subject_exists_in_write(&subjects, recording.subject_id)? {
                    return Err(StorageError::MissingSubject {
                        recording_id: key,
                        subject_id: recording.subject_id.to_string(),
                    });
                }
                drop(subjects);

                let mut recordings = write
                    .open_table(RECORDINGS)
                    .map_err(StorageError::database)?;
                old_metadata = Self::recording_metadata_in_write(&recordings, recording.id)?
                    .ok_or_else(|| StorageError::NotFound {
                        entity: EntityKind::Recording,
                        id: key.clone(),
                    })?;
                recordings
                    .insert(key.as_str(), value.as_slice())
                    .map_err(StorageError::database)?;
            }
            write.commit().map_err(StorageError::database)?;
            Ok(old_metadata)
        })();

        match result {
            Ok(old_metadata) => {
                // Once redb points at the new payload, the old version is no
                // longer observable and may be removed safely.
                if let Ok(old_path) = self.sample_path(&old_metadata) {
                    let _ = fs::remove_file(old_path);
                }
                Ok(())
            }
            Err(error) => {
                let _ = fs::remove_file(&sample_path);
                Err(error)
            }
        }
    }

    fn delete_recording(&self, id: RecordingId) -> StorageResult<()> {
        let key = id.to_string();
        let write = self
            .inner
            .database
            .begin_write()
            .map_err(StorageError::database)?;
        {
            let mut table = write
                .open_table(RECORDINGS)
                .map_err(StorageError::database)?;
            if table
                .remove(key.as_str())
                .map_err(StorageError::database)?
                .is_none()
            {
                return Err(StorageError::NotFound {
                    entity: EntityKind::Recording,
                    id: key,
                });
            }
        }
        write.commit().map_err(StorageError::database)?;

        let directory = self.recording_directory(id);
        match fs::remove_dir_all(&directory) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(StorageError::io(directory, error)),
        }
    }
}
