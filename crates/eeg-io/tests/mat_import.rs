mod support;

use eeg_io::{EegIoError, EegReader, MatLayout, MatSamplingRate};
use support::*;

fn read(bytes: &[u8]) -> Result<domain::EegRecording, EegIoError> {
    mat_reader().read(&mut &*bytes, &context())
}

#[test]
fn mat_reads_non_square_column_major_matrix_in_both_endiannesses() {
    for little in [true, false] {
        let value = read(&data_mat(
            &[2, 3],
            &[1.0, -1.0, 2.0, -2.0, 3.0, -3.0],
            little,
            false,
        ))
        .unwrap();
        assert_eq!(
            value.samples,
            vec![vec![1.0, 2.0, 3.0], vec![-1.0, -2.0, -3.0]]
        );
        assert_eq!(value.sampling_rate_hz, 2.0);
        assert_eq!(value.duration_seconds, 1.5);
    }
}

#[test]
fn mat_explicit_samples_by_channels_and_supplied_rate() {
    let mut reader = mat_reader();
    reader.options.layout = MatLayout::SamplesByChannels;
    reader.options.sampling_rate = MatSamplingRate::Hertz(3.0);
    reader.options.channels[0].unit = "mV".into();
    let bytes = data_mat(
        &[3, 2],
        &[0.001, 0.002, 0.003, -1.0, -2.0, -3.0],
        true,
        false,
    );
    let value = reader.read(&mut bytes.as_slice(), &context()).unwrap();
    assert_eq!(
        value.samples,
        vec![vec![1.0, 2.0, 3.0], vec![-1.0, -2.0, -3.0]]
    );
    assert_eq!(value.sampling_rate_hz, 3.0);
    assert_eq!(value.duration_seconds, 1.0);
}

#[test]
fn mat_supports_compressed_and_mixed_elements() {
    for little in [true, false] {
        let arrays = [
            matrix("eeg", &[2, 2], &[1.0, -1.0, 2.0, -2.0], little, false),
            matrix("fs", &[1, 1], &[2.0], little, false),
        ];
        let compressed = mat(&arrays, little, true);
        assert_eq!(
            read(&compressed).unwrap().samples,
            vec![vec![1.0, 2.0], vec![-1.0, -2.0]]
        );
        let mut mixed = mat(&arrays[..1], little, false);
        mixed.extend_from_slice(&mat(&arrays[1..], little, true)[128..]);
        assert!(read(&mixed).is_ok());
    }
}

#[test]
fn mat_reads_all_real_numeric_storage_types() {
    // The class number describes MATLAB's logical numeric class; miTYPE in the
    // real payload describes its actual binary storage, not the same enum.
    let types = [
        (8, 1, 1),
        (9, 2, 1),
        (10, 3, 2),
        (11, 4, 2),
        (12, 5, 4),
        (13, 6, 4),
        (7, 7, 4),
        (6, 9, 8),
        (14, 12, 8),
        (15, 13, 8),
    ];
    for (class, kind, width) in types {
        let mut body = element(6, &[word(class, true), word(0, true)].concat(), true);
        body.extend(element(5, &[word(2, true), word(1, true)].concat(), true));
        body.extend(element(1, b"eeg", true));
        let values: Vec<u8> = match kind {
            7 => [1.0f32, 2.0].iter().flat_map(|v| v.to_le_bytes()).collect(),
            9 => [1.0f64, 2.0].iter().flat_map(|v| v.to_le_bytes()).collect(),
            _ => [1u64, 2]
                .iter()
                .flat_map(|v| v.to_le_bytes()[..width].to_vec())
                .collect(),
        };
        body.extend(element(kind, &values, true));
        let bytes = mat(
            &[
                element(14, &body, true),
                matrix("fs", &[1, 1], &[2.0], true, false),
            ],
            true,
            false,
        );
        let value = read(&bytes).unwrap_or_else(|error| panic!("class={class}: {error}"));
        assert_eq!(value.samples, vec![vec![1.0], vec![2.0]], "class={class}");
    }
}

#[test]
fn mat_supports_small_data_element_variable_names() {
    let mut body = element(6, &[word(6, true), word(0, true)].concat(), true);
    body.extend(element(5, &[word(2, true), word(1, true)].concat(), true));
    body.extend(word((3 << 16) | 1, true));
    body.extend(*b"eeg\0");
    body.extend(element(
        9,
        &[1.0f64.to_le_bytes(), 2.0f64.to_le_bytes()].concat(),
        true,
    ));
    let bytes = mat(
        &[
            element(14, &body, true),
            matrix("fs", &[1, 1], &[2.0], true, false),
        ],
        true,
        false,
    );
    assert_eq!(read(&bytes).unwrap().samples, vec![vec![1.0], vec![2.0]]);
}

#[test]
fn mat_signed_int32_dependency_workaround_preserves_negative_samples() {
    for little in [true, false] {
        let mut body = element(6, &[word(12, little), word(0, little)].concat(), little);
        body.extend(element(
            5,
            &[word(2, little), word(2, little)].concat(),
            little,
        ));
        body.extend(element(1, b"eeg", little));
        let values: Vec<_> = [-1i32, 2, -3, 4]
            .iter()
            .flat_map(|&v| word(v as u32, little))
            .collect();
        body.extend(element(5, &values, little));
        let bytes = mat(&[element(14, &body, little)], little, true);
        let mut reader = mat_reader();
        reader.options.sampling_rate = MatSamplingRate::Hertz(2.0);
        assert_eq!(
            reader
                .read(&mut bytes.as_slice(), &context())
                .unwrap()
                .samples,
            vec![vec![-1.0, -3.0], vec![2.0, 4.0]]
        );
    }
}

#[test]
fn mat_rejects_missing_duplicate_and_invalid_rate_variables() {
    let eeg = matrix("eeg", &[2, 1], &[1.0, 2.0], true, false);
    assert!(read(&mat(std::slice::from_ref(&eeg), true, false)).is_err());
    let fs = matrix("fs", &[1, 1], &[2.0], true, false);
    assert!(read(&mat(&[eeg.clone(), eeg.clone(), fs], true, false)).is_err());
    for rate in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(
            read(&mat(
                &[eeg.clone(), matrix("fs", &[1, 1], &[rate], true, false)],
                true,
                false
            ))
            .is_err()
        );
    }
    assert!(
        read(&mat(
            &[eeg, matrix("fs", &[1, 2], &[1.0, 2.0], true, false)],
            true,
            false
        ))
        .is_err()
    );
}

#[test]
fn mat_rejects_complex_nonfinite_overflowing_and_non_2d_eeg() {
    assert!(matches!(
        read(&data_mat(&[2, 1], &[1.0, 2.0], true, true)),
        Err(EegIoError::Unsupported(_))
    ));
    for bad in [f64::NAN, f64::INFINITY, 1e100] {
        assert!(read(&data_mat(&[2, 1], &[bad, 2.0], true, false)).is_err());
    }
    assert!(read(&data_mat(&[2, 1, 1], &[1.0, 2.0], true, false)).is_err());
    assert!(read(&data_mat(&[1, 2], &[1.0, 2.0], true, false)).is_err());
    assert!(read(&data_mat(&[2, 0], &[], true, false)).is_err());
}

#[test]
fn mat_rejects_header_truncation_trailing_garbage_and_payload_shape_mismatch() {
    let bytes = data_mat(&[2, 2], &[1.0, 2.0, 3.0, 4.0], true, false);
    for length in [0, 127, bytes.len() - 1] {
        assert!(read(&bytes[..length]).is_err());
    }
    let mut trailing = bytes.clone();
    trailing.extend([1, 2, 3]);
    assert!(read(&trailing).is_err());
    assert!(read(&data_mat(&[2, 2], &[1.0, 2.0], true, false)).is_err());
    assert!(read(&data_mat(&[-1, 2], &[1.0, 2.0], true, false)).is_err());
    assert!(read(&data_mat(&[65536, 65536, 0], &[], true, false)).is_err());
}

#[test]
fn mat_rejects_unsupported_hdf5_and_nested_compression() {
    assert!(matches!(
        read(b"MATLAB 7.3 MAT-file"),
        Err(EegIoError::Unsupported(_))
    ));
    assert!(matches!(
        read(b"\x89HDF\r\n\x1a\n"),
        Err(EegIoError::Unsupported(_))
    ));
    let compressed = mat(
        &[matrix("eeg", &[2, 1], &[1.0, 2.0], true, false)],
        true,
        true,
    );
    let nested = mat(&[compressed[128..].to_vec()], true, true);
    assert!(read(&nested).is_err());
}

#[test]
fn mat_enforces_decoded_shape_and_compressed_expansion_limits() {
    let bytes = data_mat(&[2, 2], &[1.0, 2.0, 3.0, 4.0], true, false);
    let mut ctx = context();
    ctx.limits.max_total_samples = 3;
    assert!(matches!(
        mat_reader().read(&mut bytes.as_slice(), &ctx),
        Err(EegIoError::LimitExceeded(_))
    ));
    let bomb = mat(
        &[matrix("eeg", &[2, 2000], &vec![0.0; 4000], true, false)],
        true,
        true,
    );
    ctx.limits.max_input_bytes = 1000;
    assert!(bomb.len() < 1000);
    assert!(matches!(
        mat_reader().read(&mut bomb.as_slice(), &ctx),
        Err(EegIoError::LimitExceeded(_))
    ));
}

#[test]
fn mat_rejects_bad_numeric_tail_even_if_requested_variable_precedes_it() {
    let mut reader = mat_reader();
    reader.options.sampling_rate = MatSamplingRate::Hertz(2.0);
    let good = matrix("eeg", &[2, 1], &[1.0, 2.0], true, false);
    let mut incompatible = matrix("other", &[1, 1], &[3.0], true, false);
    // Change class to single but leave miDOUBLE payload (same valid framing).
    incompatible[16..20].copy_from_slice(&word(7, true));
    let bytes = mat(&[good, incompatible], true, false);
    assert!(reader.read(&mut bytes.as_slice(), &context()).is_err());
}
