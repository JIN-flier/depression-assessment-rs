//! 模块边界、异常输入、预算与回放契约。构造输入后主动破坏公开字段，
//! 确保 API 不依赖“所有调用者都使用领域构造函数”的假设。
mod support;
use domain::{ChannelKind, DomainError};
use eeg_features::{
    EegFeatureExtractor, FeatureConfig, FeatureError, FeatureExtraction, SpectralAudit,
    SpectralExtractor,
};
use std::sync::Arc;
use support::*;

#[test]
fn domain_and_detailed_result_roundtrip_and_replay_without_mutating_input() {
    let input = recording(256.0, vec![sine(256.0, 10.0, 20.0, 1024)]);
    let original = input.clone();
    let first = extractor().extract_detailed(&input, &context()).unwrap();
    let decoded: FeatureExtraction =
        serde_json::from_str(&serde_json::to_string(&first).unwrap()).unwrap();
    assert_eq!(decoded, first);
    let p = &first.features.provenance;
    assert_eq!(p.generated_at, context().generated_at);
    assert_eq!(p.software_version, context().software_version);
    assert_eq!(p.algorithm_version, "p6-welch-v1");
    let audit: SpectralAudit = serde_json::from_str(&p.parameters["audit"]).unwrap();
    assert_eq!(audit, first.audit);
    let config: FeatureConfig = serde_json::from_str(&p.parameters["configuration"]).unwrap();
    assert_eq!(
        SpectralExtractor::new(config)
            .unwrap()
            .extract_detailed(&input, &context())
            .unwrap(),
        first
    );
    assert_eq!(
        extractor().extract(&input, &context()).unwrap(),
        first.features
    );
    assert_eq!(input, original);
    assert!(
        first.features.spatial.is_none()
            && first.features.connectivity.is_none()
            && first.features.complexity.is_none()
    );
}

#[test]
fn trait_is_object_safe_and_parallel_calls_share_no_mutable_state() {
    let service: Arc<dyn EegFeatureExtractor> = Arc::new(extractor());
    let input = Arc::new(recording(256.0, vec![sine(256.0, 10.0, 20.0, 1024)]));
    let expected = service.extract(&input, &context()).unwrap();
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let service = service.clone();
            let input = input.clone();
            std::thread::spawn(move || service.extract(&input, &context()).unwrap())
        })
        .collect();
    for handle in handles {
        assert_eq!(handle.join().unwrap(), expected);
    }
}

#[test]
fn only_eeg_and_explicitly_selected_channels_are_computed() {
    let mut input = recording(
        256.0,
        vec![
            vec![0.0; 1024],
            sine(256.0, 10.0, 20.0, 1024),
            vec![1.0; 1024],
        ],
    );
    input.channels[2].kind = ChannelKind::Trigger;
    input.channels[2].unit = "code".into();
    let config = FeatureConfig {
        excluded_channels: vec!["Ch0".into()],
        ..Default::default()
    };
    let result = SpectralExtractor::new(config)
        .unwrap()
        .extract_detailed(&input, &context())
        .unwrap();
    assert_eq!(result.audit.included_channels, ["Ch1"]);
    assert_eq!(result.audit.excluded_channels, ["Ch0", "Ch2"]);
    assert_eq!(result.features.spectral.unwrap().psd_by_channel.len(), 1);
    let config = FeatureConfig {
        excluded_channels: vec!["missing".into()],
        ..Default::default()
    };
    assert!(matches!(
        SpectralExtractor::new(config)
            .unwrap()
            .extract(&input, &context()),
        Err(FeatureError::UnknownChannel(_))
    ));
    let config = FeatureConfig {
        excluded_channels: vec!["Ch0".into(), "Ch1".into()],
        ..Default::default()
    };
    assert!(matches!(
        SpectralExtractor::new(config)
            .unwrap()
            .extract(&input, &context()),
        Err(FeatureError::NoEegChannels)
    ));
}

#[test]
fn malformed_recordings_return_typed_errors() {
    let valid = recording(256.0, vec![vec![0.0; 1024], vec![0.0; 1024]]);
    let check = |input| extractor().extract(&input, &context()).unwrap_err();
    let mut input = valid.clone();
    input.samples.pop();
    assert!(matches!(
        check(input),
        FeatureError::Domain(DomainError::ChannelSampleCountMismatch { .. })
    ));
    let mut input = valid.clone();
    input.samples[1].pop();
    assert!(matches!(
        check(input),
        FeatureError::Domain(DomainError::UnequalSampleCount { .. })
    ));
    let mut input = valid.clone();
    input.channels[1].label = "Ch0".into();
    assert!(matches!(
        check(input),
        FeatureError::Domain(DomainError::DuplicateChannel { .. })
    ));
    let mut input = valid.clone();
    input.channels[0].label = " ".into();
    assert!(matches!(
        check(input),
        FeatureError::Domain(DomainError::EmptyField { .. })
    ));
    let mut input = valid.clone();
    input.channels[0].unit = "count".into();
    assert!(matches!(check(input), FeatureError::UnsupportedUnit { .. }));
    let mut input = valid.clone();
    input.channels.clear();
    input.samples.clear();
    assert!(matches!(
        check(input),
        FeatureError::Domain(DomainError::NoChannels)
    ));
    let mut input = valid.clone();
    input
        .channels
        .iter_mut()
        .for_each(|c| c.kind = ChannelKind::Eog);
    assert!(matches!(check(input), FeatureError::NoEegChannels));
    let mut input = valid.clone();
    input.samples.iter_mut().for_each(Vec::clear);
    input.duration_seconds = 0.0;
    assert!(matches!(check(input), FeatureError::InvalidRecording(_)));
    for value in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let mut input = valid.clone();
        input.sampling_rate_hz = value;
        assert!(matches!(check(input), FeatureError::InvalidRecording(_)));
    }
    for value in [0.0, -1.0, 1.0, f64::NAN, f64::INFINITY] {
        let mut input = valid.clone();
        input.duration_seconds = value;
        assert!(matches!(check(input), FeatureError::InvalidRecording(_)));
    }
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut input = valid.clone();
        input.samples[0][11] = value;
        assert!(matches!(
            check(input),
            FeatureError::Domain(DomainError::NonFiniteSample {
                sample_index: 11,
                ..
            })
        ));
    }
    let mut ctx = context();
    ctx.software_version = " ".into();
    assert!(matches!(
        extractor().extract(&valid, &ctx),
        Err(FeatureError::Domain(DomainError::EmptyField { .. }))
    ));
}

#[test]
fn short_recording_low_rate_unresolved_band_and_undersized_fft_are_explicit_errors() {
    assert!(matches!(
        extractor().extract(&recording(256.0, vec![vec![0.0; 511]]), &context()),
        Err(FeatureError::TooShort {
            minimum: 512,
            actual: 511
        })
    ));
    assert!(matches!(
        extractor().extract(&recording(32.0, vec![vec![0.0; 128]]), &context()),
        Err(FeatureError::UnavailableRange { .. })
    ));
    let mut config = FeatureConfig::default();
    config.welch.fft_size = Some(128);
    assert!(matches!(
        SpectralExtractor::new(config)
            .unwrap()
            .extract(&recording(256.0, vec![vec![0.0; 512]]), &context()),
        Err(FeatureError::Configuration(_))
    ));
    let mut config = FeatureConfig::default();
    config.bands[0].range.high_hz = 0.6;
    config.bands[0].range.low_hz = 0.55;
    assert!(matches!(
        SpectralExtractor::new(config)
            .unwrap()
            .extract(&recording(256.0, vec![vec![0.0; 512]]), &context()),
        Err(FeatureError::UnresolvedRange { .. })
    ));
}

#[test]
fn unknown_config_fields_and_invalid_numerical_choices_are_rejected() {
    let default = serde_json::to_value(FeatureConfig::default()).unwrap();
    for path in ["", "welch", "limits", "relative_power_range", "bands"] {
        let mut json = default.clone();
        let object = match path {
            "" => &mut json,
            "bands" => &mut json["bands"][0],
            _ => &mut json[path],
        };
        object["typo"] = serde_json::json!(1);
        assert!(
            serde_json::from_value::<FeatureConfig>(json).is_err(),
            "{path}"
        );
    }
    let mut invalid = Vec::new();
    for value in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let mut c = FeatureConfig::default();
        c.welch.window_seconds = value;
        invalid.push(c);
    }
    for value in [-0.1, 1.0, f64::NAN, f64::INFINITY] {
        let mut c = FeatureConfig::default();
        c.welch.overlap_fraction = value;
        invalid.push(c);
    }
    for n in [0, 1, 2] {
        let mut c = FeatureConfig::default();
        c.welch.fft_size = Some(n);
        invalid.push(c);
    }
    let mut c = FeatureConfig::default();
    c.bands.clear();
    invalid.push(c);
    let mut c = FeatureConfig::default();
    c.bands.push(c.bands[0].clone());
    invalid.push(c);
    let mut c = FeatureConfig::default();
    c.bands[0].range.high_hz = 5.0;
    invalid.push(c);
    let mut c = FeatureConfig::default();
    c.bands[0].range.low_hz = -1.0;
    invalid.push(c);
    let mut c = FeatureConfig::default();
    c.bands[0].range.high_hz = f64::NAN;
    invalid.push(c);
    let mut c = FeatureConfig::default();
    c.relative_power_range.high_hz = 20.0;
    invalid.push(c);
    invalid.push(FeatureConfig {
        excluded_channels: vec!["Ch0".into(), "Ch0".into()],
        ..Default::default()
    });
    invalid.push(FeatureConfig {
        excluded_channels: vec![" ".into()],
        ..Default::default()
    });
    let mut c = FeatureConfig::default();
    c.limits.max_windows = 0;
    invalid.push(c);
    for config in invalid {
        assert!(matches!(
            SpectralExtractor::new(config),
            Err(FeatureError::Configuration(_))
        ));
    }
}

#[test]
fn resource_budgets_fail_before_fft_and_extreme_window_products_do_not_panic() {
    let input = recording(256.0, vec![vec![0.0; 1024], vec![0.0; 1024]]);
    let defaults = FeatureConfig::default();
    for limits in [
        eeg_features::FeatureLimits {
            max_total_samples: 2047,
            ..defaults.limits
        },
        eeg_features::FeatureLimits {
            max_fft_size: 511,
            ..defaults.limits
        },
        eeg_features::FeatureLimits {
            max_windows: 5,
            ..defaults.limits
        },
        eeg_features::FeatureLimits {
            max_output_values: 513,
            ..defaults.limits
        },
        eeg_features::FeatureLimits {
            max_fft_operations: 100,
            ..defaults.limits
        },
    ] {
        let config = FeatureConfig {
            limits,
            ..Default::default()
        };
        assert!(matches!(
            SpectralExtractor::new(config)
                .unwrap()
                .extract(&input, &context()),
            Err(FeatureError::LimitExceeded(_))
        ));
    }
    let mut config = FeatureConfig::default();
    config.welch.window_seconds = f64::MAX;
    assert!(matches!(
        SpectralExtractor::new(config)
            .unwrap()
            .extract(&input, &context()),
        Err(FeatureError::LimitExceeded(_))
    ));
    let mut config = FeatureConfig::default();
    config.welch.window_seconds = 0.0001;
    assert!(matches!(
        SpectralExtractor::new(config)
            .unwrap()
            .extract(&input, &context()),
        Err(FeatureError::Configuration(_))
    ));
}
