//! P3 -> P5 -> P4 -> P5 integration, entirely headless. Import and processing
//! crates are dev dependencies; neither is needed by the quality runtime.
mod support;
use eeg_io::{CsvOptions, CsvReader, EegReader, ImportContext};
use eeg_quality::QualityAnalyzer;
use eeg_signal::{PipelineConfig, ProcessingContext, SignalPipeline, SignalStep};
use serde::Deserialize;
use support::*;

#[test]
fn import_quality_preprocessing_and_reassessment_preserve_audit_and_input() {
    let mut csv = String::from("Fp1,Fp2\n");
    for (eeg, line) in sine(256.0, 10.0, 20.0, 2048)
        .iter()
        .zip(sine(256.0, 50.0, 60.0, 2048))
    {
        csv.push_str(&format!("{},0\n", eeg + line));
    }
    let import = ImportContext::new(domain::RecordingId::new(), domain::SubjectId::new());
    let raw = CsvReader::new(CsvOptions::new(256.0))
        .read(&mut csv.as_bytes(), &import)
        .unwrap();
    let original = raw.clone();
    let before = analyzer().assess(&raw, &context()).unwrap();
    assert_eq!(before.bad_channels, ["Fp1", "Fp2"]);
    assert!(has_warning(
        &before.channel_quality["Fp1"],
        "POWER_LINE_INTERFERENCE"
    ));
    let pipeline = SignalPipeline::new(PipelineConfig {
        steps: vec![SignalStep::DcRemoval {}, SignalStep::notch(50.0)],
        ..PipelineConfig::default()
    })
    .unwrap();
    let processed = pipeline
        .process(&raw, &ProcessingContext::new(context().generated_at))
        .unwrap();
    let after = analyzer().assess(&processed, &context()).unwrap();
    assert_eq!(after.recording_id, before.recording_id);
    assert_eq!(after.bad_channels, ["Fp2"]);
    assert!(after.channel_quality["Fp1"].score > before.channel_quality["Fp1"].score);
    assert!(after.channel_quality["Fp1"].power_line_interference.get() < 0.01);
    let history: Vec<domain::ProcessingStep> =
        serde_json::from_str(&after.provenance.parameters["input_processing_history"]).unwrap();
    assert_eq!(history, processed.processing_history);
    assert_eq!(raw, original);
    let replay: eeg_quality::QualityConfig =
        serde_json::from_str(&after.provenance.parameters["configuration"]).unwrap();
    assert_eq!(
        QualityAnalyzer::new(replay)
            .unwrap()
            .assess(&processed, &context())
            .unwrap(),
        after
    );
}

#[test]
fn analytic_mixture_golden_validates_power_ratios_and_final_score() {
    #[derive(Deserialize)]
    struct Golden {
        sampling_rate_hz: f64,
        sample_count: usize,
        components: Vec<[f64; 2]>,
        expected_variance_uv2: f64,
        expected_line_fraction: f64,
        expected_high_frequency_fraction: f64,
        expected_score: f64,
    }
    let golden: Golden = serde_json::from_str(include_str!("data/analytic-mixture.json")).unwrap();
    let mut row = vec![0.0; golden.sample_count];
    for [frequency, amplitude] in golden.components {
        for (sample, component) in row.iter_mut().zip(sine(
            golden.sampling_rate_hz,
            frequency,
            amplitude,
            golden.sample_count,
        )) {
            *sample += component;
        }
    }
    let result = analyzer()
        .assess_detailed(&recording(golden.sampling_rate_hz, vec![row]), &context())
        .unwrap();
    let m = &result.measurements["Ch0"];
    close(m.variance_uv2.unwrap(), golden.expected_variance_uv2, 1e-5);
    close(
        m.line_power_fraction.unwrap().get(),
        golden.expected_line_fraction,
        1e-7,
    );
    close(
        m.high_frequency_power_fraction.unwrap().get(),
        golden.expected_high_frequency_fraction,
        1e-7,
    );
    close(
        result.signal_quality.overall_score.get(),
        golden.expected_score,
        1e-7,
    );
}

#[test]
fn non_power_of_two_epochs_zero_pad_only_valid_data_and_keep_expected_ratios() {
    // 250 Hz acquisition makes 500-sample epochs and a 512-point FFT. This is
    // intentionally off the FFT bins; compare to independent energy fractions
    // with tolerance for the Hann leakage near the line band's edges.
    let rows = sine(250.0, 10.0, 20.0, 1000)
        .iter()
        .zip(sine(250.0, 50.0, 10.0, 1000))
        .map(|(alpha, line)| alpha + line)
        .collect();
    let result = analyzer()
        .assess_detailed(&recording(250.0, vec![rows]), &context())
        .unwrap();
    assert_eq!(
        result.signal_quality.provenance.parameters["fft_size"],
        "512"
    );
    close(
        result.measurements["Ch0"]
            .line_power_fraction
            .unwrap()
            .get(),
        0.2,
        0.001,
    );
    close(
        result.measurements["Ch0"]
            .high_frequency_power_fraction
            .unwrap()
            .get(),
        0.2,
        0.001,
    );
}
