//! Synthetic EEG acceptance tests: physical behavior, not implementation mirroring.
mod support;
use domain::ChannelKind;
use eeg_signal::{FilterPhase, SignalStep};
use std::f64::consts::PI;
use support::*;

#[test]
fn dc_removal_centers_each_channel_and_preserves_ac_offsets() {
    let input = recording(250.0, vec![vec![10.0, 12.0, 14.0], vec![-8.0, -4.0, 0.0]]);
    let output = pipeline(vec![SignalStep::DcRemoval {}])
        .process(&input, &context())
        .unwrap();
    assert_eq!(output.samples, [vec![-2.0, 0.0, 2.0], vec![-4.0, 0.0, 4.0]]);
    assert_eq!(input.samples[0], [10.0, 12.0, 14.0]);
}

#[test]
fn bandpass_gain_matches_independent_bilinear_butterworth_response() {
    // Analog second-order Butterworth magnitude evaluated with bilinear frequency
    // warping. This reference is independent of RBJ coefficients/recurrence.
    let rate = 250.0;
    for phase in [FilterPhase::Causal, FilterPhase::ZeroPhase] {
        for frequency in [1.0, 5.0, 10.0, 40.0, 80.0] {
            let input = recording(rate, vec![sine(rate, frequency, 5000)]);
            let step = SignalStep::Bandpass {
                low_hz: 5.0,
                high_hz: 40.0,
                phase,
            };
            let output = pipeline(vec![step]).process(&input, &context()).unwrap();
            let ratio = |cutoff: f64| (PI * frequency / rate).tan() / (PI * cutoff / rate).tan();
            let denominator = |x: f64| ((1.0 - x * x).powi(2) + 2.0 * x * x).sqrt();
            let hp = ratio(5.0).powi(2) / denominator(ratio(5.0));
            let lp = 1.0 / denominator(ratio(40.0));
            let expected = (hp * lp).powi(if phase == FilterPhase::ZeroPhase {
                2
            } else {
                1
            });
            let measured = amplitude(&output.samples[0], rate, frequency, 2000, 4000);
            assert!(
                (measured - expected).abs() < 0.0001,
                "{phase:?}, {frequency}Hz: measured={measured}, expected={expected}"
            );
        }
    }
}

#[test]
fn zero_phase_filter_retains_phase_and_channel_state_is_independent() {
    let row = sine(250.0, 10.0, 5000);
    let input = recording(250.0, vec![row.clone(), vec![0.0; row.len()], row]);
    let output = pipeline(vec![SignalStep::bandpass(2.0, 40.0)])
        .process(&input, &context())
        .unwrap();
    assert_eq!(output.samples[0], output.samples[2]);
    assert!(output.samples[1].iter().all(|&x| x == 0.0));
    let (sin, cos) = component(&output.samples[0], 250.0, 10.0, 2000, 4000);
    assert!(sin > 0.99);
    assert!(cos.abs() < 0.00001);
}

#[test]
fn notch_suppresses_50_and_60_hz_without_removing_alpha() {
    for line in [50.0, 60.0] {
        let alpha = sine(250.0, 10.0, 5000);
        let noise = sine(250.0, line, 5000);
        let input = recording(
            250.0,
            vec![alpha.iter().zip(noise).map(|(&a, b)| a + b).collect()],
        );
        let output = pipeline(vec![SignalStep::notch(line)])
            .process(&input, &context())
            .unwrap();
        assert!(amplitude(&output.samples[0], 250.0, line, 2000, 4000) < 0.001);
        assert!(amplitude(&output.samples[0], 250.0, 10.0, 2000, 4000) > 0.99);
    }
}

#[test]
fn filtering_uses_only_eeg_channels_and_keeps_ancillary_data_exact() {
    let mut input = recording(250.0, vec![vec![42.0; 200], vec![1.0; 200], vec![2.0; 200]]);
    input.channels[1].kind = ChannelKind::Trigger;
    input.channels[2].kind = ChannelKind::Eog;
    let output = pipeline(vec![
        SignalStep::DcRemoval {},
        SignalStep::bandpass(1.0, 45.0),
        SignalStep::notch(50.0),
    ])
    .process(&input, &context())
    .unwrap();
    assert!(output.samples[0].iter().all(|&x| x.abs() < 1e-6));
    assert_eq!(output.samples[1..], input.samples[1..]);
}

#[test]
fn downsampling_prevents_high_frequency_alias_and_preserves_alpha_phase() {
    let alpha = sine(250.0, 10.0, 2500);
    let noise = sine(250.0, 80.0, 2500);
    let input = recording(
        250.0,
        vec![alpha.iter().zip(noise).map(|(&a, b)| a + b).collect()],
    );
    let output = pipeline(vec![SignalStep::resample(100.0)])
        .process(&input, &context())
        .unwrap();
    assert_eq!(output.sample_count(), 1000);
    // A naive decimator would alias the 80Hz noise to 20Hz at unit amplitude.
    assert!(amplitude(&output.samples[0], 100.0, 20.0, 200, 800) < 0.001);
    let (sin, cos) = component(&output.samples[0], 100.0, 10.0, 200, 800);
    assert!((sin - 1.0).abs() < 0.001);
    assert!(cos.abs() < 0.001);
}

#[test]
fn upsampling_interpolates_analog_and_holds_trigger_codes() {
    let input = recording(100.0, vec![sine(100.0, 10.0, 1000)]);
    let output = pipeline(vec![SignalStep::resample(250.0)])
        .process(&input, &context())
        .unwrap();
    assert_eq!(output.sample_count(), 2500);
    let (sin, cos) = component(&output.samples[0], 250.0, 10.0, 500, 2000);
    assert!((sin - 1.0).abs() < 0.001 && cos.abs() < 0.001);
    let mut input = recording(2.0, vec![vec![0.0, 1.0, 0.0, 2.0]]);
    input.channels[0].kind = ChannelKind::Trigger;
    let output = pipeline(vec![SignalStep::resample(4.0)])
        .process(&input, &context())
        .unwrap();
    assert_eq!(output.samples[0], [0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 2.0, 2.0]);
}

#[test]
fn arbitrary_rate_rounds_count_and_maintains_domain_duration() {
    let input = recording(250.0, vec![vec![3.25; 101], vec![-7.0; 101]]);
    let output = pipeline(vec![SignalStep::resample(128.0)])
        .process(&input, &context())
        .unwrap();
    assert_eq!(output.sample_count(), 52);
    assert_eq!(output.duration_seconds, 52.0 / 128.0);
    assert!((output.duration_seconds - input.duration_seconds).abs() <= 0.5 / 128.0);
    assert!(output.samples[0].iter().all(|&x| (x - 3.25).abs() < 1e-6));
    assert!(output.samples[1].iter().all(|&x| (x + 7.0).abs() < 1e-6));
}

#[test]
fn single_sample_resampling_reflects_constant_and_same_rate_is_exact_identity() {
    let input = recording(100.0, vec![vec![7.0]]);
    let output = pipeline(vec![SignalStep::resample(200.0)])
        .process(&input, &context())
        .unwrap();
    assert_eq!(output.samples[0], [7.0, 7.0]);
    let input = recording(250.0, vec![sine(250.0, 80.0, 101)]);
    let output = pipeline(vec![SignalStep::resample(250.0)])
        .process(&input, &context())
        .unwrap();
    assert_eq!(output.samples, input.samples);
    assert_eq!(output.processing_history[0].parameters["identity"], "true");
}
