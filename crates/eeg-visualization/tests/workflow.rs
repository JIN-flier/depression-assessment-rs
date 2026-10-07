//! P3 → P5 → P4 → P6 → P7：模块间只交换领域结构，工作流不依赖 UI。
mod support;
use chrono::Utc;
use eeg_features::{FeatureConfig, FeatureContext, SpectralExtractor};
use eeg_io::{CsvOptions, CsvReader, EegReader, ImportContext};
use eeg_quality::{QualityAnalyzer, QualityConfig, QualityContext};
use eeg_signal::{PipelineConfig, ProcessingContext, SignalPipeline, SignalStep};
use eeg_visualization::*;
use support::*;

#[test]
fn import_quality_preprocess_features_render_all_three_plots_preserves_audit() {
    let mut csv = "Fp1,Fp2,Cz\n".to_owned();
    let waves = [
        sine(256.0, 10.0, 20.0, 2048),
        sine(256.0, 10.0, 10.0, 2048),
        sine(256.0, 10.0, 30.0, 2048),
    ];
    for ((fp1, fp2), cz) in waves[0].iter().zip(&waves[1]).zip(&waves[2]) {
        csv.push_str(&format!("{fp1},{fp2},{cz}\n"));
    }
    let context = ImportContext::new(domain::RecordingId::new(), domain::SubjectId::new());
    let raw = CsvReader::new(CsvOptions::new(256.0))
        .read(&mut csv.as_bytes(), &context)
        .unwrap();
    let before = raw.clone();
    let time = Utc::now();
    let quality = QualityAnalyzer::new(QualityConfig::default())
        .unwrap()
        .assess(&raw, &QualityContext::new(time))
        .unwrap();
    assert_eq!(quality.recording_id, raw.id);
    let processed = SignalPipeline::new(PipelineConfig {
        steps: vec![SignalStep::DcRemoval {}],
        ..Default::default()
    })
    .unwrap()
    .process(&raw, &ProcessingContext::new(time))
    .unwrap();
    let features = SpectralExtractor::new(FeatureConfig::default())
        .unwrap()
        .extract(&processed, &FeatureContext::new(time))
        .unwrap();
    let expected_features = features.clone();
    let history = processed.processing_history.clone();
    let service: Box<dyn EegVisualizer> = Box::new(VisualizationService::default());
    let renderer: Box<dyn PlotRenderer> = Box::new(SvgRenderer::default());
    let plots = [
        EegPlot::Waveform(
            service
                .waveform(
                    &raw,
                    &WaveformRequest {
                        start_seconds: 1.0,
                        end_seconds: 3.0,
                        max_points_per_channel: 128,
                        ..Default::default()
                    },
                )
                .unwrap(),
        ),
        EegPlot::Psd(service.psd(&features, &PsdRequest::default()).unwrap()),
        EegPlot::Topomap(
            service
                .topomap(
                    &features,
                    &ElectrodeLayout::schematic_10_20(),
                    &TopomapRequest {
                        measure: BandPowerMeasure::Absolute,
                        ..Default::default()
                    },
                )
                .unwrap(),
        ),
    ];
    for plot in plots {
        let svg = renderer.render(&plot, SvgOptions::default()).unwrap();
        assert!(svg.contains(&raw.id.to_string()) && svg.ends_with("</svg>"));
    }
    let psd = service
        .psd(
            &features,
            &PsdRequest {
                scale: PsdScale::Linear,
                ..Default::default()
            },
        )
        .unwrap();
    let fp1 = psd.traces.iter().find(|t| t.channel == "Fp1").unwrap();
    let peak = fp1
        .points
        .iter()
        .max_by(|a, b| a.y.total_cmp(&b.y))
        .unwrap();
    assert_eq!(peak.x, 10.0);
    let map = service
        .topomap(
            &features,
            &ElectrodeLayout::schematic_10_20(),
            &TopomapRequest {
                measure: BandPowerMeasure::Absolute,
                ..Default::default()
            },
        )
        .unwrap();
    // A²/2 是独立的解析功率对照；渲染不能改变这三个原始电极值。
    for (label, expected) in [("Fp1", 200.0), ("Fp2", 50.0), ("Cz", 450.0)] {
        let actual = map
            .electrodes
            .iter()
            .find(|e| e.channel == label)
            .unwrap()
            .value;
        assert!((actual - expected).abs() < 1e-4);
    }
    assert_eq!(raw, before);
    assert_eq!(features, expected_features);
    assert_eq!(processed.processing_history, history);
    assert_eq!(history.len(), 1);
}
