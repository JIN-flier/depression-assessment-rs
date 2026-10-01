//! Versioned binary encoding for channel-major `f32` sample matrices.

use crate::{EntityKind, StorageError, StorageResult};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufReader, BufWriter, Read, Write},
    path::{Component, Path},
};

const MAGIC: &[u8; 8] = b"EEGSMP01";
const HEADER_BYTES: u64 = 8 + 4 + 8;

/// Writes to a new, uniquely named file and synchronizes it before returning.
/// The caller publishes the filename in redb only after this function succeeds.
pub(crate) fn write(path: &Path, samples: &[Vec<f32>]) -> StorageResult<()> {
    let result = (|| {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|error| StorageError::io(path, error))?;
        let mut writer = BufWriter::new(file);

        writer
            .write_all(MAGIC)
            .and_then(|()| writer.write_all(&(samples.len() as u32).to_le_bytes()))
            .and_then(|()| {
                writer.write_all(&(samples.first().map_or(0, Vec::len) as u64).to_le_bytes())
            })
            .map_err(|error| StorageError::io(path, error))?;

        for row in samples {
            for sample in row {
                writer
                    .write_all(&sample.to_le_bytes())
                    .map_err(|error| StorageError::io(path, error))?;
            }
        }
        writer
            .flush()
            .map_err(|error| StorageError::io(path, error))?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|error| StorageError::io(path, error))
    })();

    // A partial file must never be mistaken for a committed payload. Removing
    // it is best-effort because the original error is more useful to callers.
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}

pub(crate) fn read(
    path: &Path,
    recording_id: &str,
    expected_channels: usize,
    expected_samples: usize,
) -> StorageResult<Vec<Vec<f32>>> {
    let file = File::open(path).map_err(|error| StorageError::io(path, error))?;
    let file_len = file
        .metadata()
        .map_err(|error| StorageError::io(path, error))?
        .len();
    let mut reader = BufReader::new(file);

    let mut magic = [0_u8; 8];
    reader
        .read_exact(&mut magic)
        .map_err(|error| corrupt_or_io(path, recording_id, error))?;
    if &magic != MAGIC {
        return Err(corrupt(recording_id, "unsupported sample file format"));
    }

    let channels = read_u32(&mut reader, path, recording_id)? as usize;
    let sample_count = read_u64(&mut reader, path, recording_id)? as usize;
    if channels != expected_channels || sample_count != expected_samples {
        return Err(corrupt(
            recording_id,
            format!(
                "sample shape is {channels}x{sample_count}, metadata expects {expected_channels}x{expected_samples}"
            ),
        ));
    }

    let value_count = channels
        .checked_mul(sample_count)
        .ok_or_else(|| corrupt(recording_id, "sample dimensions overflow usize"))?;
    let payload_bytes = (value_count as u64)
        .checked_mul(4)
        .ok_or_else(|| corrupt(recording_id, "sample payload size overflows u64"))?;
    let expected_file_len = HEADER_BYTES
        .checked_add(payload_bytes)
        .ok_or_else(|| corrupt(recording_id, "sample file size overflows u64"))?;
    if file_len != expected_file_len {
        return Err(corrupt(
            recording_id,
            format!("sample file has {file_len} bytes, expected {expected_file_len}"),
        ));
    }

    let mut samples = Vec::with_capacity(channels);
    for _ in 0..channels {
        let mut row = Vec::with_capacity(sample_count);
        for _ in 0..sample_count {
            let mut bytes = [0_u8; 4];
            reader
                .read_exact(&mut bytes)
                .map_err(|error| corrupt_or_io(path, recording_id, error))?;
            row.push(f32::from_le_bytes(bytes));
        }
        samples.push(row);
    }
    Ok(samples)
}

/// Only a single normal path component is accepted from persisted metadata.
/// This turns database tampering into a corruption error instead of allowing a
/// payload path to escape its recording directory.
pub(crate) fn validate_file_name(file_name: &str, recording_id: &str) -> StorageResult<()> {
    let mut components = Path::new(file_name).components();
    let valid =
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();
    if valid {
        Ok(())
    } else {
        Err(corrupt(recording_id, "invalid sample payload filename"))
    }
}

fn read_u32(reader: &mut impl Read, path: &Path, id: &str) -> StorageResult<u32> {
    let mut bytes = [0_u8; 4];
    reader
        .read_exact(&mut bytes)
        .map_err(|error| corrupt_or_io(path, id, error))?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(reader: &mut impl Read, path: &Path, id: &str) -> StorageResult<u64> {
    let mut bytes = [0_u8; 8];
    reader
        .read_exact(&mut bytes)
        .map_err(|error| corrupt_or_io(path, id, error))?;
    Ok(u64::from_le_bytes(bytes))
}

fn corrupt(id: &str, reason: impl Into<String>) -> StorageError {
    StorageError::CorruptData {
        entity: EntityKind::Recording,
        id: id.to_owned(),
        reason: reason.into(),
    }
}

fn corrupt_or_io(path: &Path, id: &str, error: std::io::Error) -> StorageError {
    if error.kind() == std::io::ErrorKind::UnexpectedEof {
        corrupt(id, "sample file is truncated")
    } else {
        StorageError::io(path, error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn sample_file_round_trips_channel_major_values() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("samples.bin");
        let expected = vec![vec![1.0, -2.5], vec![3.25, 4.0]];

        write(&path, &expected).unwrap();
        let actual = read(&path, "recording-id", 2, 2).unwrap();

        assert_eq!(actual, expected);
    }

    #[test]
    fn sample_file_rejects_a_shape_mismatch() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("samples.bin");
        write(&path, &[vec![1.0, 2.0]]).unwrap();

        let error = read(&path, "recording-id", 2, 2).unwrap_err();
        assert!(matches!(error, StorageError::CorruptData { .. }));
    }
}
