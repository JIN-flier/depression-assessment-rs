//! P3 导入 → P5 质量 → P4 预处理 → P6 特征，无 UI/数据库/LLM。
//! 其他 EEG crate 仅为 dev-dependencies，工作流编排属于应用层。
mod support;
use domain::FrequencyBand;
use eeg_features::{FeatureConfig, SpectralExtractor};
use eeg_io::{CsvOptions, CsvReader, EegReader, ImportContext};
use eeg_quality::{QualityAnalyzer, QualityConfig, QualityContext};
use eeg_signal::{PipelineConfig, ProcessingContext, SignalPipeline, SignalStep};
use serde::Deserialize;
use std::collections::BTreeMap;
use support::*;

#[test]
fn import_quality_preprocessing_features_preserve_input_and_processing_history() {
    let mut csv = String::from("Fp1,Fp2\n");
    for (alpha, line) in sine(256.0, 10.0, 20.0, 2048)
        .iter()
        .zip(sine(256.0, 50.0, 60.0, 2048))
    {
        csv.push_str(&format!("{},0\n", alpha + line));
    }
    let raw = CsvReader::new(CsvOptions::new(256.0))
        .read(
            &mut csv.as_bytes(),
            &ImportContext::new(domain::RecordingId::new(), domain::SubjectId::new()),
        )
        .unwrap();
    let original = raw.clone();
    let quality = QualityAnalyzer::new(QualityConfig::default())
        .unwrap()
        .assess(&raw, &QualityContext::new(context().generated_at))
        .unwrap();
    assert_eq!(quality.bad_channels, ["Fp1", "Fp2"]);
    let pipeline = SignalPipeline::new(PipelineConfig {
        steps: vec![SignalStep::DcRemoval {}, SignalStep::notch(50.0)],
        ..Default::default()
    })
    .unwrap();
    let processed = pipeline
        .process(&raw, &ProcessingContext::new(context().generated_at))
        .unwrap();
    let quality = QualityAnalyzer::new(QualityConfig::default())
        .unwrap()
        .assess(&processed, &QualityContext::new(context().generated_at))
        .unwrap();
    assert_eq!(quality.bad_channels, ["Fp2"]);
    // 应用层显式将质量策略转成通道选择；P6 不依赖或重新计算 P5 结果。
    let config = FeatureConfig {
        excluded_channels: quality.bad_channels,
        ..Default::default()
    };
    let result = SpectralExtractor::new(config)
        .unwrap()
        .extract_detailed(&processed, &context())
        .unwrap();
    assert_eq!(result.features.recording_id, raw.id);
    assert_eq!(result.audit.included_channels, ["Fp1"]);
    assert_eq!(result.audit.excluded_channels, ["Fp2"]);
    let s = result.features.spectral.as_ref().unwrap();
    close(
        s.band_power_by_channel["Fp1"][&FrequencyBand::Alpha].absolute,
        200.0,
        1.0,
    );
    let history: Vec<domain::ProcessingStep> =
        serde_json::from_str(&result.features.provenance.parameters["input_processing_history"])
            .unwrap();
    assert_eq!(history, processed.processing_history);
    assert_eq!(raw, original);
    assert_eq!(processed.processing_history.len(), 2);
}

#[test]
fn analytic_mixture_golden_has_expected_psd_and_each_band_power() {
    #[derive(Deserialize)]
    struct Golden {
        sampling_rate_hz: f64,
        sample_count: usize,
        components: Vec<[f64; 2]>,
        expected_absolute_uv2: BTreeMap<FrequencyBand, f64>,
        expected_relative: BTreeMap<FrequencyBand, f64>,
        expected_reference_power_uv2: f64,
        expected_window_count: usize,
        expected_bin_width_hz: f64,
    }
    let g: Golden = serde_json::from_str(include_str!("data/analytic-mixture.json")).unwrap();
    let mut row = vec![0.0; g.sample_count];
    for [frequency, amplitude] in g.components {
        for (v, component) in row.iter_mut().zip(sine(
            g.sampling_rate_hz,
            frequency,
            amplitude,
            g.sample_count,
        )) {
            *v += component;
        }
    }
    let result = extractor()
        .extract_detailed(&recording(g.sampling_rate_hz, vec![row]), &context())
        .unwrap();
    assert_eq!(result.audit.window_count, g.expected_window_count);
    close(
        result.audit.frequency_resolution_hz,
        g.expected_bin_width_hz,
        1e-12,
    );
    close(
        result.audit.reference_power_uv2_by_channel["Ch0"],
        g.expected_reference_power_uv2,
        1e-5,
    );
    let s = result.features.spectral.unwrap();
    for (band, expected) in g.expected_absolute_uv2 {
        close(
            s.band_power_by_channel["Ch0"][&band].absolute,
            expected,
            1e-5,
        );
        close(
            s.band_power_by_channel["Ch0"][&band].relative.get(),
            g.expected_relative[&band],
            1e-7,
        );
    }
    // 10 Hz、6 uV 正弦在 2 秒周期 Hann 下主 bin 密度为 A²*T/3=24。
    close(s.psd_by_channel["Ch0"][20], 24.0, 1e-5);
}
