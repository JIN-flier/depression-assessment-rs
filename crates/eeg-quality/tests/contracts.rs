//! Public boundary, reproducibility and scoring-policy tests.
mod support;
use domain::DomainError;
use eeg_quality::{
    QualityAnalyzer, QualityAssessment, QualityConfig, QualityError, SignalQualityEvaluator,
};
use std::sync::Arc;
use support::*;

#[test]
fn domain_output_and_detailed_audit_roundtrip_with_identical_replay() {
    let input = recording(256.0, vec![sine(256.0, 10.0, 20.0, 1024)]);
    let context = context();
    let first = analyzer().assess_detailed(&input, &context).unwrap();
    let decoded: QualityAssessment =
        serde_json::from_str(&serde_json::to_string(&first).unwrap()).unwrap();
    assert_eq!(decoded, first);
    let provenance = &first.signal_quality.provenance;
    assert_eq!(provenance.generated_at, context.generated_at);
    assert_eq!(provenance.software_version, context.software_version);
    assert_eq!(provenance.algorithm_version, "p5-qc-v2");
    assert_eq!(
        provenance.parameters["spectral_method"],
        "nonoverlap-periodic-hann-detrend-constant-rustfft-v1"
    );
    assert_eq!(provenance.parameters["window_samples"], "512");
    let config: QualityConfig =
        serde_json::from_str(&provenance.parameters["configuration"]).unwrap();
    let replay = QualityAnalyzer::new(config)
        .unwrap()
        .assess_detailed(&input, &context)
        .unwrap();
    assert_eq!(replay, first);
    assert_eq!(
        analyzer().assess(&input, &context).unwrap(),
        first.signal_quality
    );
    let measurements: std::collections::BTreeMap<String, eeg_quality::ChannelMeasurements> =
        serde_json::from_str(&provenance.parameters["measurements"]).unwrap();
    assert_eq!(measurements, first.measurements);
}

#[test]
fn trait_injection_and_parallel_calls_share_no_mutable_state() {
    let analyzer: Arc<dyn SignalQualityEvaluator> = Arc::new(analyzer());
    let input = Arc::new(recording(256.0, vec![sine(256.0, 10.0, 20.0, 1024)]));
    let expected = analyzer.assess(&input, &context()).unwrap();
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let analyzer = analyzer.clone();
            let input = input.clone();
            std::thread::spawn(move || analyzer.assess(&input, &context()).unwrap())
        })
        .collect();
    for handle in handles {
        assert_eq!(handle.join().unwrap(), expected);
    }
}

#[test]
fn epoch_noise_score_uses_weakest_dimension_and_overall_uses_all_eeg_channels() {
    let mut noisy = sine(256.0, 10.0, 20.0, 1024);
    noisy[0..128].fill(123.0); // Exactly 1/8 of the channel is flat/noisy.
    let input = recording(256.0, vec![sine(256.0, 10.0, 20.0, 1024), noisy]);
    let result = QualityAnalyzer::new(time_only())
        .unwrap()
        .assess(&input, &context())
        .unwrap();
    let expected = 1.0 - 0.125 / (2.0 * 0.2);
    close(result.channel_quality["Ch1"].score.get(), expected, 1e-12);
    close(result.overall_score.get(), (1.0 + expected) / 2.0, 1e-12);
    assert!(result.bad_channels.is_empty());
}

#[test]
fn scoring_can_change_without_changing_detector_measurements() {
    let input = recording(256.0, vec![sine(256.0, 50.0, 20.0, 1024)]);
    let baseline = analyzer().assess_detailed(&input, &context()).unwrap();
    let mut config = QualityConfig::default();
    config.scoring.line_power_fraction_limit = 1.0;
    config.scoring.high_frequency_power_fraction_limit = 1.0;
    config.scoring.bad_channel_score_threshold = 0.4;
    config.scoring.unavailable_score_cap = 0.3;
    let changed = QualityAnalyzer::new(config)
        .unwrap()
        .assess_detailed(&input, &context())
        .unwrap();
    assert_eq!(changed.measurements, baseline.measurements);
    assert!(changed.signal_quality.overall_score > baseline.signal_quality.overall_score);
}

#[test]
fn single_finite_sample_is_insufficient_even_when_spectra_are_disabled() {
    let input = recording(256.0, vec![vec![20.0]]);
    let result = QualityAnalyzer::new(time_only())
        .unwrap()
        .assess(&input, &context())
        .unwrap();
    assert!(has_warning(
        &result.channel_quality["Ch0"],
        "INSUFFICIENT_FINITE_SAMPLES"
    ));
    assert_eq!(result.overall_score.get(), 0.5);
    assert_eq!(result.bad_channels, ["Ch0"]);
}

#[test]
fn malformed_public_fields_return_typed_errors_without_panics() {
    let valid = recording(256.0, vec![sine(256.0, 10.0, 20.0, 1024), vec![0.0; 1024]]);
    let check = |input| analyzer().assess(&input, &context()).unwrap_err();
    let mut input = valid.clone();
    input.samples.pop();
    assert!(matches!(
        check(input),
        QualityError::Domain(DomainError::ChannelSampleCountMismatch { .. })
    ));
    let mut input = valid.clone();
    input.samples[1].pop();
    assert!(matches!(
        check(input),
        QualityError::Domain(DomainError::UnequalSampleCount { .. })
    ));
    let mut input = valid.clone();
    input.channels[1].label = input.channels[0].label.clone();
    assert!(matches!(
        check(input),
        QualityError::Domain(DomainError::DuplicateChannel { .. })
    ));
    let mut input = valid.clone();
    input.channels[0].label = " ".into();
    assert!(matches!(
        check(input),
        QualityError::Domain(DomainError::EmptyField { .. })
    ));
    let mut input = valid.clone();
    input.channels[0].unit = "counts".into();
    assert!(matches!(check(input), QualityError::UnsupportedUnit { .. }));
    for value in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let mut input = valid.clone();
        input.sampling_rate_hz = value;
        assert!(matches!(check(input), QualityError::InvalidRecording(_)));
    }
    for value in [0.0, f64::NAN, f64::INFINITY] {
        let mut input = valid.clone();
        input.duration_seconds = value;
        assert!(matches!(check(input), QualityError::InvalidRecording(_)));
    }
    let mut input = valid.clone();
    input.channels.clear();
    input.samples.clear();
    assert!(matches!(
        check(input),
        QualityError::Domain(DomainError::NoChannels)
    ));
    let mut input = valid.clone();
    input.samples.iter_mut().for_each(Vec::clear);
    input.duration_seconds = 0.0;
    assert!(matches!(check(input), QualityError::InvalidRecording(_)));
    let mut input = valid;
    input
        .channels
        .iter_mut()
        .for_each(|channel| channel.kind = domain::ChannelKind::Eog);
    assert!(matches!(check(input), QualityError::NoEegChannels));
}

#[test]
fn serialized_config_rejects_unknown_fields_and_constructor_rejects_invalid_values() {
    let default = serde_json::to_value(QualityConfig::default()).unwrap();
    for path in ["", "flatline", "variance", "spectral", "scoring", "limits"] {
        let mut json = default.clone();
        let object = if path.is_empty() {
            &mut json
        } else {
            &mut json[path]
        };
        object["typo"] = serde_json::json!(1);
        assert!(
            serde_json::from_value::<QualityConfig>(json).is_err(),
            "path {path}"
        );
    }
    for (path, key, value) in [
        ("", "window_seconds", 0.0),
        ("", "extreme_amplitude_uv", -1.0),
        ("flatline", "tolerance_uv", -1.0),
        ("flatline", "minimum_duration_seconds", 0.0),
        ("variance", "maximum_uv2", 0.001),
        ("spectral", "minimum_hz", 0.0),
        ("spectral", "line_half_width_hz", 100.0),
        ("spectral", "high_frequency_start_hz", 0.5),
        ("scoring", "missing_fraction_limit", 0.0),
        ("scoring", "noisy_fraction_limit", 1.1),
        ("scoring", "unavailable_score_cap", 0.6),
    ] {
        let mut json = default.clone();
        let object = if path.is_empty() {
            &mut json
        } else {
            &mut json[path]
        };
        object[key] = serde_json::json!(value);
        let config = serde_json::from_value(json).unwrap();
        assert!(
            matches!(
                QualityAnalyzer::new(config),
                Err(QualityError::Configuration(_))
            ),
            "{path}.{key}"
        );
    }
    let config = QualityConfig {
        window_seconds: f64::NAN,
        ..QualityConfig::default()
    };
    assert!(QualityAnalyzer::new(config).is_err());
    let mut config = QualityConfig::default();
    config.spectral.line_frequency_hz = Some(f64::INFINITY);
    assert!(QualityAnalyzer::new(config).is_err());
    let mut context = context();
    context.software_version = " ".into();
    assert!(
        analyzer()
            .assess(&recording(256.0, vec![vec![0.0; 1024]]), &context)
            .is_err()
    );
}

#[test]
fn budgets_and_extreme_epoch_products_fail_before_computation() {
    let input = recording(256.0, vec![vec![0.0; 1024], vec![0.0; 1024]]);
    for limits in [
        eeg_quality::QualityLimits {
            max_total_samples: 1000,
            ..QualityConfig::default().limits
        },
        eeg_quality::QualityLimits {
            max_fft_size: 256,
            ..QualityConfig::default().limits
        },
        eeg_quality::QualityLimits {
            max_windows: 3,
            ..QualityConfig::default().limits
        },
    ] {
        let config = QualityConfig {
            limits,
            ..QualityConfig::default()
        };
        assert!(matches!(
            QualityAnalyzer::new(config)
                .unwrap()
                .assess(&input, &context()),
            Err(QualityError::LimitExceeded(_))
        ));
    }
    let config = QualityConfig {
        window_seconds: f64::MAX,
        ..QualityConfig::default()
    };
    assert!(matches!(
        QualityAnalyzer::new(config)
            .unwrap()
            .assess(&input, &context()),
        Err(QualityError::LimitExceeded(_))
    ));
    let config = QualityConfig {
        window_seconds: 0.001,
        ..QualityConfig::default()
    };
    assert!(matches!(
        QualityAnalyzer::new(config)
            .unwrap()
            .assess(&input, &context()),
        Err(QualityError::Configuration(_))
    ));
    let config = QualityConfig {
        flatline: eeg_quality::FlatlineConfig {
            minimum_duration_seconds: f64::MAX,
            tolerance_uv: 0.01,
        },
        ..time_only()
    };
    let result = QualityAnalyzer::new(config)
        .unwrap()
        .assess_detailed(&input, &context())
        .unwrap();
    assert_eq!(result.measurements["Ch0"].flatline_fraction.get(), 0.0);

    // Relative duration validation must still reject a stale/zero duration
    // when the actual recording duration is below any common absolute epsilon.
    let analyzer = QualityAnalyzer::new(QualityConfig {
        window_seconds: 8e-12,
        ..time_only()
    })
    .unwrap();
    let mut tiny = recording(1e12, vec![vec![20.0; 8]]);
    analyzer.assess(&tiny, &context()).unwrap();
    tiny.duration_seconds = 0.0;
    assert!(matches!(
        analyzer.assess(&tiny, &context()),
        Err(QualityError::InvalidRecording(_))
    ));
    tiny.duration_seconds = 4e-12;
    assert!(matches!(
        analyzer.assess(&tiny, &context()),
        Err(QualityError::InvalidRecording(_))
    ));
}
