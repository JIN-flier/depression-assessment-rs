mod support;

use domain::{Channel, ChannelKind, RecordingState};
use eeg_io::{CsvOptions, CsvReader, EegIoError, EegReader};
use support::*;

fn read(text: &str) -> Result<domain::EegRecording, EegIoError> {
    CsvReader::new(CsvOptions::new(2.0)).read(&mut text.as_bytes(), &context())
}

#[test]
fn csv_transposes_sample_rows_and_preserves_caller_identity() {
    let mut ctx = context();
    ctx.metadata.device = Some("amplifier".into());
    ctx.metadata
        .extra
        .insert("operator_note".into(), "synthetic".into());
    let value = CsvReader::new(CsvOptions::new(2.0))
        .read(&mut "Fp1,Fp2\n1,-1\n2,-2\n".as_bytes(), &ctx)
        .unwrap();
    assert_eq!(value.samples, vec![vec![1.0, 2.0], vec![-1.0, -2.0]]);
    assert_eq!(value.id, ctx.recording_id);
    assert_eq!(value.subject_id, ctx.subject_id);
    assert_eq!(value.duration_seconds, 1.0);
    assert_eq!(value.channels, channels());
    assert_eq!(value.metadata.device, ctx.metadata.device);
    assert_eq!(value.metadata.extra["operator_note"], "synthetic");
    assert_eq!(value.metadata.source_format.as_deref(), Some("csv"));
    assert_eq!(value.recording_state, RecordingState::Raw);
    assert!(value.processing_history.is_empty());
}

#[test]
fn csv_handles_quoted_headers_bom_whitespace_and_crlf() {
    let value = read("\u{feff}\"Fp1\",\"Fp2\"\r\n 1 , -2 \r\n 3e0 , 4.5 \r\n").unwrap();
    assert_eq!(value.samples, vec![vec![1.0, 3.0], vec![-2.0, 4.5]]);
    assert_eq!(value.channels, channels());
}

#[test]
fn csv_headerless_custom_delimiter_and_non_eeg_units() {
    let mut options = CsvOptions::new(2.0);
    options.has_headers = false;
    options.delimiter = b';';
    options.channels = vec![
        Channel::new("Fp1", ChannelKind::Eeg, "mV").unwrap(),
        Channel::new("event", ChannelKind::Trigger, "code").unwrap(),
    ];
    let value = CsvReader::new(options)
        .read(&mut "0.001;7\n-0.002;8".as_bytes(), &context())
        .unwrap();
    assert_eq!(value.samples, vec![vec![1.0, -2.0], vec![7.0, 8.0]]);
    assert_eq!(value.channels[0].unit, "uV");
    assert_eq!(value.metadata.extra["eeg_io.channel.0.source_unit"], "mV");
    assert_eq!(value.channels[1].unit, "code");
}

#[test]
fn csv_rejects_sampling_rate_that_overflows_recording_duration() {
    assert!(
        CsvReader::new(CsvOptions::new(1e-320))
            .read(&mut "Fp1\n1\n".as_bytes(), &context())
            .is_err()
    );
}

#[test]
fn csv_converts_voltage_units_without_changing_polarity() {
    for (unit, input, expected) in [
        ("V", "-0.000002", -2.0),
        ("mV", "0.003", 3.0),
        ("uV", "4", 4.0),
        ("µV", "5", 5.0),
        ("μV", "6", 6.0),
        ("nV", "7000", 7.0),
    ] {
        let mut options = CsvOptions::new(1.0);
        options.unit = unit.into();
        let value = CsvReader::new(options)
            .read(&mut format!("Fp1\n{input}\n").as_bytes(), &context())
            .unwrap();
        assert_eq!(value.samples, vec![vec![expected]], "{unit}");
        assert_eq!(value.channels[0].unit, "uV");
    }
}

#[test]
fn csv_rejects_missing_malformed_ragged_and_nonfinite_samples() {
    for text in [
        "Fp1,Fp2\n1,\n",
        "Fp1,Fp2\n1\n",
        "Fp1,Fp2\n1,2,3\n",
        "Fp1\nhello\n",
        "Fp1\nNaN\n",
        "Fp1\ninf\n",
        "Fp1\n1e100\n",
    ] {
        assert!(read(text).is_err(), "{text:?}");
    }
}

#[test]
fn csv_rejects_empty_data_duplicate_and_blank_channels() {
    for text in ["", "Fp1,Fp2\n", "Fp1,Fp1\n1,2\n", ",Fp2\n1,2\n"] {
        assert!(read(text).is_err());
    }
}

#[test]
fn csv_rejects_invalid_schema_and_sampling_rate() {
    for rate in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(matches!(
            CsvReader::new(CsvOptions::new(rate)).read(&mut "Fp1\n1\n".as_bytes(), &context()),
            Err(EegIoError::Configuration(_))
        ));
    }
    let mut options = CsvOptions::new(1.0);
    options.channels = channels();
    assert!(
        CsvReader::new(options.clone())
            .read(&mut "Fp2,Fp1\n1,2\n".as_bytes(), &context())
            .is_err()
    );
    options.has_headers = false;
    options.channels.clear();
    assert!(matches!(
        CsvReader::new(options).read(&mut "1,2".as_bytes(), &context()),
        Err(EegIoError::Configuration(_))
    ));
}

#[test]
fn csv_enforces_each_resource_budget() {
    let input = "Fp1,Fp2\n1,2\n3,4\n";
    for field in 0..4 {
        let mut ctx = context();
        match field {
            0 => ctx.limits.max_input_bytes = input.len() - 1,
            1 => ctx.limits.max_channels = 1,
            2 => ctx.limits.max_samples_per_channel = 1,
            _ => ctx.limits.max_total_samples = 3,
        }
        assert!(matches!(
            CsvReader::new(CsvOptions::new(1.0)).read(&mut input.as_bytes(), &ctx),
            Err(EegIoError::LimitExceeded(_))
        ));
    }
}
