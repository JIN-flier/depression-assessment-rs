//! 可运行的 P6→P7 示例，仅使用合成 EEG，不涉及真实受试者数据。
//! cargo run -p eeg-visualization --example render_eeg -- /tmp/eeg-preview
use chrono::{TimeZone, Utc};
use domain::{
    Channel, ChannelKind, EegRecording, RecordingId, RecordingMetadata, RecordingState, SubjectId,
};
use eeg_features::{FeatureConfig, FeatureContext, SpectralExtractor};
use eeg_visualization::*;
use std::{error::Error, f64::consts::TAU, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let directory = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("usage: render_eeg OUTPUT_DIRECTORY")?,
    );
    let layout = ElectrodeLayout::schematic_10_20();
    let sampling_rate = 256.0;
    let channels = layout
        .positions
        .iter()
        .map(|p| Channel::new(&p.channel, ChannelKind::Eeg, "uV"))
        .collect::<Result<Vec<_>, _>>()?;
    let samples = layout
        .positions
        .iter()
        .map(|p| {
            let alpha_amplitude = 20.0 + 12.0 * p.x + 6.0 * p.y;
            (0..2048)
                .map(|i| {
                    let time = i as f64 / sampling_rate;
                    (alpha_amplitude * (TAU * 10.0 * time).sin()
                        + 6.0 * (TAU * 6.0 * time).sin()
                        + 3.0 * (TAU * 20.0 * time).sin()) as f32
                })
                .collect()
        })
        .collect();
    let recording = EegRecording::new(
        RecordingId::new(),
        SubjectId::new(),
        sampling_rate,
        channels,
        samples,
        RecordingState::Raw,
        RecordingMetadata::default(),
    )?;
    let time = Utc
        .with_ymd_and_hms(2026, 10, 7, 0, 0, 0)
        .single()
        .ok_or("invalid example time")?;
    let features = SpectralExtractor::new(FeatureConfig::default())?
        .extract(&recording, &FeatureContext::new(time))?;
    // 服务和渲染器通过接口注入；未来 Slint 只需把这些 SVG 交给图像适配层。
    let service: Box<dyn EegVisualizer> = Box::new(VisualizationService::default());
    let renderer: Box<dyn PlotRenderer> = Box::new(SvgRenderer::default());
    let visible_channels = vec!["Fp1".into(), "Fp2".into(), "Cz".into(), "O1".into()];
    let plots = [
        (
            "waveform.svg",
            EegPlot::Waveform(service.waveform(
                &recording,
                &WaveformRequest {
                    end_seconds: 2.0,
                    channels: visible_channels.clone(),
                    max_points_per_channel: 800,
                    ..Default::default()
                },
            )?),
        ),
        (
            "psd.svg",
            EegPlot::Psd(service.psd(
                &features,
                &PsdRequest {
                    channels: visible_channels,
                    ..Default::default()
                },
            )?),
        ),
        (
            "topomap.svg",
            EegPlot::Topomap(service.topomap(
                &features,
                &layout,
                &TopomapRequest {
                    measure: BandPowerMeasure::Absolute,
                    resolution: 96,
                    ..Default::default()
                },
            )?),
        ),
    ];
    std::fs::create_dir_all(&directory)?;
    for (filename, plot) in plots {
        let path = directory.join(filename);
        std::fs::write(
            &path,
            renderer.render(
                &plot,
                SvgOptions {
                    width: 1200,
                    height: 800,
                },
            )?,
        )?;
        println!("{}", path.display());
    }
    Ok(())
}
