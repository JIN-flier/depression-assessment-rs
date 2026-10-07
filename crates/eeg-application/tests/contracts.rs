mod support;
use eeg_application::*;
use eeg_io::{MatLayout, MatSamplingRate};
use support::*;

#[test]
fn form_validation_rejects_invalid_numbers_without_silent_defaults() {
    for age in ["NaN", "-1", "126", "3.5"] {
        assert!(
            SubjectForm {
                age: age.into(),
                sex_index: 0,
                notes: String::new()
            }
            .parse()
            .is_err()
        );
    }
    assert_eq!(
        SubjectForm {
            age: "".into(),
            sex_index: 0,
            notes: String::new()
        }
        .parse()
        .unwrap()
        .age,
        None
    );
    let mut form = ProcessingForm {
        dc: true,
        bandpass: true,
        low_hz: "0.5".into(),
        high_hz: "30".into(),
        notch: false,
        notch_hz: "bad but disabled".into(),
        resample: false,
        target_hz: "bad but disabled".into(),
        exclude_bad_channels: false,
    };
    assert!(form.parse().is_ok());
    for value in ["NaN", "inf", "0", "-1", "abc", "31"] {
        form.low_hz = value.into();
        assert!(form.parse().is_err());
    }
    for view in [
        ViewRequest {
            amplitude_uv: 0.0,
            ..Default::default()
        },
        ViewRequest {
            start_seconds: -1.0,
            ..Default::default()
        },
        ViewRequest {
            window_seconds: f64::NAN,
            ..Default::default()
        },
        ViewRequest {
            channels: Some(vec!["Cz".into(); 257]),
            ..Default::default()
        },
    ] {
        assert!(view.validate().is_err());
    }
}

#[test]
fn import_form_maps_mat_layout_and_rate_variable_and_guards_headerless_csv() {
    let mut form = ImportForm {
        format_index: 2,
        sampling_rate: "ignored".into(),
        unit: "uV".into(),
        csv_headers: false,
        labels: "Fp1, Fp2".into(),
        mat_variable: "eeg".into(),
        mat_rate_variable: "fs".into(),
        mat_samples_by_channels: true,
    };
    let ImportFormat::Mat(options) = form.parse().unwrap() else {
        panic!()
    };
    assert!(matches!(options.layout, MatLayout::SamplesByChannels));
    assert!(matches!(options.sampling_rate, MatSamplingRate::Variable(ref name) if name == "fs"));
    form.format_index = 1;
    form.sampling_rate = "256".into();
    form.labels.clear();
    assert!(form.parse().is_err());
    form.csv_headers = true;
    assert!(form.parse().is_ok());
    form.format_index = 0;
    form.sampling_rate = "unused".into();
    assert!(form.parse().is_ok());
}

#[test]
fn out_of_order_commands_and_failed_import_do_not_create_recordings() {
    let mut service = ApplicationService::default();
    for command in [
        AppCommand::SaveProcessed,
        AppCommand::AssessQuality(QualityConfig::default()),
        AppCommand::Analyze(AnalysisRequest::default()),
        AppCommand::SelectSubject(SubjectId::new()),
        AppCommand::LoadRecording(RecordingId::new()),
    ] {
        assert!(service.execute(command, &mut |_| {}).is_err());
    }
    let project = TestProject::new();
    let mut service = setup(&project);
    assert!(
        service
            .execute(
                AppCommand::ImportEeg {
                    path: project.0.join("missing.edf"),
                    format: ImportFormat::Edf,
                    metadata: RecordingMetadata::default()
                },
                &mut |_| {}
            )
            .is_err()
    );
    assert!(service.snapshot().recordings.is_empty());
    assert!(service.snapshot().raw.is_none());
}

#[test]
fn application_imports_all_three_p3_formats_through_same_command_boundary() {
    let project = TestProject::new();
    let mut service = setup(&project);
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../eeg-io/tests/data");
    let edf = run(
        &mut service,
        AppCommand::ImportEeg {
            path: base.join("golden.edf"),
            format: ImportFormat::Edf,
            metadata: RecordingMetadata::default(),
        },
    );
    assert_eq!(edf.raw.unwrap().channels.len(), 2);
    let mat = ImportForm {
        format_index: 2,
        sampling_rate: "4".into(),
        unit: "uV".into(),
        csv_headers: true,
        labels: "Fp1,Fp2".into(),
        mat_variable: "eeg".into(),
        mat_rate_variable: "fs".into(),
        mat_samples_by_channels: false,
    }
    .parse()
    .unwrap();
    let result = service.execute(
        AppCommand::ImportEeg {
            path: base.join("golden.mat"),
            format: mat,
            metadata: RecordingMetadata::default(),
        },
        &mut |_| {},
    );
    assert!(result.is_ok(), "{result:?}");
    let csv = import_csv(&mut service, base.join("golden.csv"), 4.0);
    assert_eq!(csv.recordings.len(), 3);
}
