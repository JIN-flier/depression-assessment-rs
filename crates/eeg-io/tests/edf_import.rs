mod support;

use domain::ChannelKind;
use eeg_io::{EdfReader, EegIoError, EegReader};
use support::*;

fn read(bytes: &[u8]) -> Result<domain::EegRecording, EegIoError> {
    EdfReader::default().read(&mut &*bytes, &context())
}

#[test]
fn edf_calibrates_offset_gain_voltage_units_and_record_order() {
    let signal = TestSignal {
        label: "Fp1",
        unit: "mV",
        physical_min: -1.0,
        physical_max: 3.0,
        digital_min: -100,
        digital_max: 100,
        count: 3,
    };
    let bytes = edf(&[signal], 2, &[-100, 0, 100, 50, -50, 0], "");
    let value = read(&bytes).unwrap();
    assert_eq!(
        value.samples[0],
        vec![-1000.0, 1000.0, 3000.0, 2000.0, 0.0, 1000.0]
    );
    assert_eq!(value.sampling_rate_hz, 3.0);
    assert_eq!(value.duration_seconds, 2.0);
    assert_eq!(value.channels[0].unit, "uV");
}

#[test]
fn edf_supports_negative_physical_slope() {
    let mut signal = TestSignal::eeg("Fp1", 2);
    signal.physical_min = 32768.0;
    signal.physical_max = -32767.0;
    assert_eq!(
        read(&edf(&[signal], 1, &[1, -2], "")).unwrap().samples[0],
        vec![-1.0, 2.0]
    );
}

#[test]
fn edf_unknown_record_count_is_derived_from_exact_payload() {
    let value = read(&edf(&[TestSignal::eeg("Fp1", 2)], -1, &[1, 2, 3, 4], "")).unwrap();
    assert_eq!(value.samples[0], vec![1.0, 2.0, 3.0, 4.0]);
}

#[test]
fn edf_rejects_mixed_rates_and_accepts_explicit_subset() {
    let bytes = edf(
        &[TestSignal::eeg("Fp1", 2), TestSignal::eeg("Fp2", 1)],
        1,
        &[1, 2, 3],
        "",
    );
    assert!(matches!(read(&bytes), Err(EegIoError::Unsupported(_))));
    let reader = EdfReader {
        channel_indices: Some(vec![1]),
        ..Default::default()
    };
    let value = reader.read(&mut bytes.as_slice(), &context()).unwrap();
    assert_eq!(value.samples, vec![vec![3.0]]);
    assert_eq!(value.sampling_rate_hz, 1.0);
}

#[test]
fn edf_preserves_requested_channel_order_and_kind_override() {
    let bytes = edf(
        &[TestSignal::eeg("Fp1", 1), TestSignal::eeg("Fp2", 1)],
        1,
        &[1, 2],
        "",
    );
    let reader = EdfReader {
        channel_indices: Some(vec![1, 0]),
        channel_kinds: [(1, ChannelKind::Eog)].into(),
    };
    let value = reader.read(&mut bytes.as_slice(), &context()).unwrap();
    assert_eq!(value.samples, vec![vec![2.0], vec![1.0]]);
    assert_eq!(value.channels[0].kind, ChannelKind::Eog);
    assert_eq!(value.metadata.extra["edf.channel.0.source_index"], "1");
}

#[test]
fn edf_rejects_invalid_selection_and_overrides() {
    let bytes = edf(&[TestSignal::eeg("Fp1", 1)], 1, &[1], "");
    for indices in [vec![], vec![1], vec![0, 0]] {
        assert!(
            EdfReader {
                channel_indices: Some(indices),
                ..Default::default()
            }
            .read(&mut bytes.as_slice(), &context())
            .is_err()
        );
    }
    assert!(
        EdfReader {
            channel_kinds: [(1, ChannelKind::Eeg)].into(),
            ..Default::default()
        }
        .read(&mut bytes.as_slice(), &context())
        .is_err()
    );
}

#[test]
fn edf_rejects_corrupt_headers_and_payloads() {
    let original = edf(&[TestSignal::eeg("Fp1", 2)], 1, &[1, 2], "");
    for (offset, width, text) in [
        (184, 8, "256"),
        (236, 8, "2"),
        (236, 8, "-2"),
        (244, 8, "0"),
        (244, 8, "NaN"),
        (252, 4, "0"),
        (256 + 216, 8, "0"),
        (256 + 104, 8, "32767"),
        (256 + 120, 8, "32767"),
        (256 + 96, 8, "unknown"),
    ] {
        let mut bytes = original.clone();
        put_field(&mut bytes, offset, width, text);
        assert!(read(&bytes).is_err(), "offset={offset}, text={text}");
    }
    for length in [0, 255, 511, original.len() - 1] {
        assert!(read(&original[..length]).is_err());
    }
    let mut extra = original.clone();
    extra.push(0);
    assert!(read(&extra).is_err());
    let mut non_ascii = original.clone();
    non_ascii[256] = 0xff;
    assert!(read(&non_ascii).is_err());
}

#[test]
fn edf_rejects_samples_outside_declared_adc_range_and_duplicate_labels() {
    let mut signal = TestSignal::eeg("Fp1", 1);
    signal.digital_min = -10;
    signal.digital_max = 10;
    assert!(read(&edf(&[signal], 1, &[11], "")).is_err());
    assert!(
        read(&edf(
            &[TestSignal::eeg("Fp1", 1), TestSignal::eeg("Fp1", 1)],
            1,
            &[1, 2],
            ""
        ))
        .is_err()
    );
}

fn continuous_edf() -> Vec<u8> {
    let mut bytes = edf(
        &[
            TestSignal::eeg("Fp1", 2),
            TestSignal::eeg("EDF Annotations", 8),
        ],
        2,
        &[1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 3, 4, 0, 0, 0, 0, 0, 0, 0, 0],
        "EDF+C",
    );
    for record in 0..2 {
        let offset = 768 + record * 20 + 4;
        let tal = format!("+{record}.25\x14\x14\0");
        bytes[offset..offset + tal.len()].copy_from_slice(tal.as_bytes());
    }
    // EDF+ annotation calibration fields are permitted to be blank.
    for offset in [96, 104, 112, 120, 128] {
        put_field(&mut bytes, 256 + 2 * offset + 8, 8, "");
    }
    bytes
}

#[test]
fn edf_plus_omits_annotations_and_checks_contiguous_timekeeping() {
    let bytes = continuous_edf();
    let value = read(&bytes).unwrap();
    assert_eq!(value.samples, vec![vec![1.0, 2.0, 3.0, 4.0]]);
    assert_eq!(value.metadata.extra["edf.annotation_signals_omitted"], "1");
    assert_eq!(
        value.metadata.extra["edf.first_record_offset_seconds"],
        "0.25"
    );
    let mut interrupted = bytes;
    interrupted[768 + 20 + 5] = b'2';
    assert!(read(&interrupted).is_err());
}

#[test]
fn edf_rejects_discontinuous_plus_and_zero_records() {
    assert!(matches!(
        read(&edf(&[TestSignal::eeg("Fp1", 1)], 1, &[0], "EDF+D")),
        Err(EegIoError::Unsupported(_))
    ));
    assert!(read(&edf(&[TestSignal::eeg("Fp1", 1)], 0, &[], "")).is_err());
    assert!(read(&edf(&[TestSignal::eeg("Fp1", 1)], 1, &[0], "EDF+C")).is_err());
}

#[test]
fn edf_enforces_resource_limits_before_sample_allocation() {
    let bytes = edf(&[TestSignal::eeg("Fp1", 2)], 2, &[1, 2, 3, 4], "");
    let mut ctx = context();
    ctx.limits.max_samples_per_channel = 3;
    assert!(matches!(
        EdfReader::default().read(&mut bytes.as_slice(), &ctx),
        Err(EegIoError::LimitExceeded(_))
    ));
}
