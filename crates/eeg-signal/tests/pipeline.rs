//! Public API contracts, replay and defensive boundary validation.
mod support;
use domain::{ChannelKind, DomainError, ProcessingStep, RecordingState};
use eeg_signal::{
    EegProcessor, FilterPhase, PipelineConfig, ProcessingLimits, SignalError, SignalPipeline,
    SignalStep,
};
use support::*;

#[test]
fn pipeline_replays_recipe_and_serialized_history_without_changing_input() {
    let mut input = recording(
        500.0,
        vec![sine(500.0, 10.0, 1000), sine(500.0, 20.0, 1000)],
    );
    input.metadata.device = Some("synthetic-amplifier".into());
    input
        .processing_history
        .push(ProcessingStep::new("upstream_normalize", "v1", context().applied_at).unwrap());
    input.recording_state = RecordingState::Normalized;
    let original = input.clone();
    let processor: Box<dyn EegProcessor> = Box::new(pipeline(vec![
        SignalStep::DcRemoval {},
        SignalStep::bandpass(1.0, 45.0),
        SignalStep::notch(50.0),
        SignalStep::resample(250.0),
    ]));
    let output = processor.process(&input, &context()).unwrap();
    assert_eq!(input, original);
    assert_eq!(output.id, input.id);
    assert_eq!(output.subject_id, input.subject_id);
    assert_eq!(output.channels, input.channels);
    assert_eq!(output.metadata, input.metadata);
    assert_eq!(output.recording_state, RecordingState::Preprocessed);
    assert_eq!(output.sampling_rate_hz, 250.0);
    assert_eq!(output.duration_seconds, 2.0);
    assert_eq!(output.processing_history[0], input.processing_history[0]);
    assert_eq!(
        output
            .processing_history
            .iter()
            .map(|s| s.operation.as_str())
            .collect::<Vec<_>>(),
        [
            "upstream_normalize",
            "dc_removal",
            "bandpass",
            "notch",
            "resample"
        ]
    );
    for step in &output.processing_history[1..] {
        assert!(!step.algorithm_version.is_empty());
        assert_eq!(step.applied_at, context().applied_at);
        assert_eq!(step.parameters["software_version"], "test-application-1.0");
        assert_eq!(
            step.parameters["eeg_signal_version"],
            env!("CARGO_PKG_VERSION")
        );
    }
    let resample = &output.processing_history[4].parameters;
    assert_eq!(resample["from_hz"], "500");
    assert_eq!(resample["to_hz"], "250");
    assert_eq!(resample["input_samples"], "1000");
    assert_eq!(resample["output_samples"], "500");

    // History embeds exact serde recipes, independent of UI or a hand-built prompt.
    let replay_steps = output.processing_history[1..]
        .iter()
        .map(|step| serde_json::from_str::<SignalStep>(&step.parameters["configuration"]).unwrap())
        .collect();
    let replay = pipeline(replay_steps).process(&input, &context()).unwrap();
    assert_eq!(replay, output);
    let decoded =
        serde_json::from_str::<domain::EegRecording>(&serde_json::to_string(&output).unwrap())
            .unwrap();
    assert_eq!(decoded, output);
}

#[test]
fn config_json_is_round_trippable_and_unknown_parameters_are_rejected() {
    let original = pipeline(vec![
        SignalStep::bandpass(1.0, 45.0),
        SignalStep::resample(250.0),
    ]);
    let json = serde_json::to_string(original.config()).unwrap();
    let decoded = SignalPipeline::new(serde_json::from_str(&json).unwrap()).unwrap();
    assert_eq!(decoded.config(), original.config());
    assert!(serde_json::from_str::<PipelineConfig>(r#"{"steps":[],"typo":1}"#).is_err());
    assert!(serde_json::from_str::<SignalStep>(r#"{"type":"dc_removal","typo":1}"#).is_err());
    assert!(
        serde_json::from_str::<SignalStep>(
            r#"{"type":"notch","frequency_hz":50,"q":30,"phase":"typo"}"#
        )
        .is_err()
    );
}

#[test]
fn empty_pipeline_is_identity_including_lifecycle_and_existing_history() {
    let mut input = recording(250.0, vec![sine(250.0, 10.0, 100)]);
    // Public fields can contain a slightly rounded persisted duration. An empty
    // recipe still preserves accepted input exactly, rather than rewriting it.
    input.duration_seconds += 1e-12;
    assert_eq!(pipeline(vec![]).process(&input, &context()).unwrap(), input);
}

#[test]
fn immutable_pipeline_can_be_reused_across_threads() {
    let processor = pipeline(vec![SignalStep::DcRemoval {}, SignalStep::notch(50.0)]);
    let input = recording(250.0, vec![sine(250.0, 10.0, 1000)]);
    let expected = processor.process(&input, &context()).unwrap();
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..4)
            .map(|_| scope.spawn(|| processor.process(&input, &context()).unwrap()))
            .collect();
        for worker in workers {
            assert_eq!(worker.join().unwrap(), expected);
        }
    });
}

#[test]
fn configuration_rejects_non_finite_unordered_and_out_of_range_parameters() {
    let invalid = vec![
        SignalStep::bandpass(0.0, 45.0),
        SignalStep::bandpass(45.0, 1.0),
        SignalStep::bandpass(1.0, f64::INFINITY),
        SignalStep::notch(f64::NAN),
        SignalStep::Notch {
            frequency_hz: 50.0,
            q: 0.0,
            phase: FilterPhase::Causal,
        },
        SignalStep::Notch {
            frequency_hz: 50.0,
            q: f64::NAN,
            phase: FilterPhase::Causal,
        },
        SignalStep::Notch {
            frequency_hz: 50.0,
            q: 10001.0,
            phase: FilterPhase::Causal,
        },
        SignalStep::resample(0.0),
        SignalStep::resample(-1.0),
        SignalStep::resample(f64::INFINITY),
        SignalStep::Resample {
            target_rate_hz: 100.0,
            half_width: 0,
            rolloff: 0.9,
        },
        SignalStep::Resample {
            target_rate_hz: 100.0,
            half_width: 32,
            rolloff: 1.0,
        },
        SignalStep::Resample {
            target_rate_hz: 100.0,
            half_width: 32,
            rolloff: f64::NAN,
        },
    ];
    for step in invalid {
        assert!(matches!(
            SignalPipeline::new(PipelineConfig {
                steps: vec![step],
                ..PipelineConfig::default()
            }),
            Err(SignalError::Configuration(_))
        ));
    }
}

#[test]
fn validates_filter_nyquist_against_intermediate_rate_and_fails_atomically() {
    let input = recording(250.0, vec![sine(250.0, 10.0, 1000)]);
    let original = input.clone();
    for steps in [
        vec![SignalStep::notch(125.0)],
        vec![SignalStep::bandpass(1.0, 126.0)],
        vec![
            SignalStep::DcRemoval {},
            SignalStep::resample(80.0),
            SignalStep::notch(50.0),
        ],
    ] {
        assert!(matches!(
            pipeline(steps).process(&input, &context()),
            Err(SignalError::Configuration(_))
        ));
        assert_eq!(input, original);
    }
    // A recipe may explicitly increase the sampling rate before a valid filter.
    let output = pipeline(vec![SignalStep::resample(500.0), SignalStep::notch(150.0)])
        .process(&input, &context())
        .unwrap();
    assert_eq!(output.sample_count(), 2000);
}

#[test]
fn rejects_numerically_degenerate_filter_coefficients() {
    let input = recording(250.0, vec![vec![1.0; 100]]);
    assert!(matches!(
        pipeline(vec![SignalStep::bandpass(1e-20, 45.0)]).process(&input, &context()),
        Err(SignalError::Configuration(_))
    ));
}

#[test]
fn zero_phase_requires_padding_but_causal_supports_short_records() {
    for count in [1, 24] {
        let input = recording(250.0, vec![vec![2.0; count]]);
        assert!(matches!(
            pipeline(vec![SignalStep::notch(50.0)]).process(&input, &context()),
            Err(SignalError::TooShort { minimum: 25, .. })
        ));
        let output = pipeline(vec![SignalStep::Notch {
            frequency_hz: 50.0,
            q: 30.0,
            phase: FilterPhase::Causal,
        }])
        .process(&input, &context())
        .unwrap();
        assert!(output.samples[0].iter().all(|&x| (x - 2.0).abs() < 1e-6));
    }
    let input = recording(250.0, vec![vec![2.0; 25]]);
    assert!(
        pipeline(vec![SignalStep::notch(50.0)])
            .process(&input, &context())
            .is_ok()
    );
}

#[test]
fn checks_short_records_created_by_resampling_before_following_filter() {
    let input = recording(250.0, vec![vec![1.0; 100]]);
    assert!(matches!(
        pipeline(vec![SignalStep::resample(50.0), SignalStep::notch(10.0)])
            .process(&input, &context()),
        Err(SignalError::TooShort { actual: 20, .. })
    ));
}

#[test]
fn malformed_public_domain_fields_never_panic_or_enter_dsp() {
    let input = recording(250.0, vec![vec![1.0; 100], vec![2.0; 100]]);
    let processor = pipeline(vec![SignalStep::DcRemoval {}]);
    let mut bad = input.clone();
    bad.samples[1].pop();
    assert!(matches!(
        processor.process(&bad, &context()),
        Err(SignalError::Domain(DomainError::UnequalSampleCount { .. }))
    ));
    let mut bad = input.clone();
    bad.samples.pop();
    assert!(matches!(
        processor.process(&bad, &context()),
        Err(SignalError::Domain(
            DomainError::ChannelSampleCountMismatch { .. }
        ))
    ));
    let mut bad = input.clone();
    bad.samples[0][0] = f32::NAN;
    assert!(matches!(
        processor.process(&bad, &context()),
        Err(SignalError::Domain(DomainError::NonFiniteSample { .. }))
    ));
    let mut bad = input.clone();
    bad.channels[1] = bad.channels[0].clone();
    assert!(matches!(
        processor.process(&bad, &context()),
        Err(SignalError::Domain(DomainError::DuplicateChannel { .. }))
    ));
    let mut bad = input.clone();
    bad.channels[0].label.clear();
    assert!(matches!(
        processor.process(&bad, &context()),
        Err(SignalError::Domain(DomainError::EmptyField { .. }))
    ));
    for rate in [0.0, f64::NAN, f64::INFINITY] {
        let mut bad = input.clone();
        bad.sampling_rate_hz = rate;
        assert!(processor.process(&bad, &context()).is_err());
    }
    let mut bad = input.clone();
    bad.duration_seconds = 99.0;
    assert!(matches!(
        processor.process(&bad, &context()),
        Err(SignalError::InvalidRecording(_))
    ));
    let mut bad = input.clone();
    bad.duration_seconds = f64::NAN;
    assert!(matches!(
        processor.process(&bad, &context()),
        Err(SignalError::InvalidRecording(_))
    ));
    let mut bad = input.clone();
    bad.channels.clear();
    bad.samples.clear();
    assert!(matches!(
        processor.process(&bad, &context()),
        Err(SignalError::InvalidRecording(_))
    ));
    let mut bad = input;
    for row in &mut bad.samples {
        row.clear();
    }
    bad.duration_seconds = 0.0;
    assert!(matches!(
        processor.process(&bad, &context()),
        Err(SignalError::InvalidRecording(_))
    ));
}

#[test]
fn budget_guards_output_shape_kernel_radius_and_work() {
    let input = recording(250.0, vec![vec![1.0; 100]]);
    let cases = [
        (
            vec![],
            ProcessingLimits {
                max_total_samples: 99,
                ..ProcessingLimits::default()
            },
        ),
        (
            vec![SignalStep::resample(1000.0)],
            ProcessingLimits {
                max_total_samples: 100,
                ..ProcessingLimits::default()
            },
        ),
        (
            vec![SignalStep::resample(50.0)],
            ProcessingLimits {
                max_kernel_radius: 100,
                ..ProcessingLimits::default()
            },
        ),
        (
            vec![SignalStep::resample(125.0)],
            ProcessingLimits {
                max_kernel_evaluations: 10,
                ..ProcessingLimits::default()
            },
        ),
        (
            vec![SignalStep::resample(f64::MAX)],
            ProcessingLimits::default(),
        ),
    ];
    for (steps, limits) in cases {
        assert!(matches!(
            SignalPipeline::new(PipelineConfig { steps, limits })
                .unwrap()
                .process(&input, &context()),
            Err(SignalError::LimitExceeded(_))
        ));
    }
    assert!(matches!(
        SignalPipeline::new(PipelineConfig {
            limits: ProcessingLimits {
                max_total_samples: 0,
                ..ProcessingLimits::default()
            },
            ..PipelineConfig::default()
        }),
        Err(SignalError::Configuration(_))
    ));
}

#[test]
fn resampling_rejects_empty_output_and_non_finite_duration() {
    let input = recording(250.0, vec![vec![1.0]]);
    assert!(matches!(
        pipeline(vec![SignalStep::resample(1.0)]).process(&input, &context()),
        Err(SignalError::Configuration(_))
    ));
    let mut input = recording(1.0, vec![vec![1.0]]);
    input.sampling_rate_hz = f64::MIN_POSITIVE;
    input.duration_seconds = 1.0 / f64::MIN_POSITIVE;
    // Extreme upsampling must be rejected by shape budgets without allocation.
    assert!(matches!(
        pipeline(vec![SignalStep::resample(1.0)]).process(&input, &context()),
        Err(SignalError::LimitExceeded(_))
    ));
    let input = recording(1e-308, vec![vec![1.0]]);
    assert!(matches!(
        pipeline(vec![SignalStep::resample(5.1e-309)]).process(&input, &context()),
        Err(SignalError::Configuration(_))
    ));
}

#[test]
fn identity_resampling_has_no_kernel_work_and_tiny_duration_validation_is_relative() {
    let input = recording(250.0, vec![vec![1.0; 100]]);
    let processor = SignalPipeline::new(PipelineConfig {
        steps: vec![SignalStep::resample(250.0)],
        limits: ProcessingLimits {
            max_kernel_radius: 1,
            max_kernel_evaluations: 1,
            ..ProcessingLimits::default()
        },
    })
    .unwrap();
    assert_eq!(
        processor.process(&input, &context()).unwrap().samples,
        input.samples
    );
    let mut tiny = recording(1e12, vec![vec![1.0]]);
    tiny.duration_seconds = 0.0;
    assert!(matches!(
        pipeline(vec![]).process(&tiny, &context()),
        Err(SignalError::InvalidRecording(_))
    ));
}

#[test]
fn operations_reject_absent_eeg_and_invalid_execution_context() {
    let mut input = recording(250.0, vec![vec![1.0; 100]]);
    input.channels[0].kind = ChannelKind::Trigger;
    assert!(matches!(
        pipeline(vec![SignalStep::DcRemoval {}]).process(&input, &context()),
        Err(SignalError::Configuration(_))
    ));
    let mut bad_context = context();
    bad_context.software_version.clear();
    assert!(matches!(
        pipeline(vec![]).process(&input, &bad_context),
        Err(SignalError::Configuration(_))
    ));
}

#[test]
fn numerical_overflow_is_an_error_without_partial_result() {
    let input = recording(250.0, vec![vec![f32::MAX, f32::MAX, -f32::MAX]]);
    let saved = input.clone();
    assert!(matches!(
        pipeline(vec![SignalStep::DcRemoval {}]).process(&input, &context()),
        Err(SignalError::Numerical {
            operation: "dc_removal"
        })
    ));
    assert_eq!(input, saved);
}
