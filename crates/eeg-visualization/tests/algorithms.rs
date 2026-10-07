//! 数值断言以手算窗口、dB 和插值性质为对照，避免只测试“能够运行”。
mod support;
use domain::{ChannelKind, FrequencyBand};
use eeg_visualization::*;
use support::*;

#[test]
fn waveform_half_open_window_uses_absolute_sample_times_and_preserves_order() {
    let input = recording(
        10.0,
        &[
            ("Fp1", (0..20).map(|n| n as f32).collect()),
            ("Cz", vec![1.0; 20]),
        ],
    );
    let before = input.clone();
    let plot = VisualizationService::default()
        .waveform(
            &input,
            &WaveformRequest {
                start_seconds: 0.21,
                end_seconds: 0.70,
                channels: vec!["Cz".into(), "Fp1".into()],
                max_points_per_channel: 100,
            },
        )
        .unwrap();
    assert_eq!(
        plot.traces
            .iter()
            .map(|t| t.channel.as_str())
            .collect::<Vec<_>>(),
        ["Cz", "Fp1"]
    );
    assert_eq!(
        plot.traces[1]
            .points
            .iter()
            .map(|p| p.y)
            .collect::<Vec<_>>(),
        [3.0, 4.0, 5.0, 6.0]
    );
    close(plot.traces[1].points[0].x, 0.3);
    assert_eq!(input, before);
}

#[test]
fn waveform_clamps_recording_end_and_displays_one_sample_window() {
    let input = recording(10.0, &[("Fp1", vec![7.0; 20])]);
    let service = VisualizationService::default();
    let plot = service
        .waveform(
            &input,
            &WaveformRequest {
                start_seconds: 1.85,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(plot.end_seconds, 2.0);
    assert_eq!(plot.traces[0].points, [PlotPoint { x: 1.9, y: 7.0 }]);
    let svg = SvgRenderer::default()
        .render(&EegPlot::Waveform(plot), SvgOptions::default())
        .unwrap();
    assert!(svg.contains("<circle"));
}

#[test]
fn envelope_retains_narrow_positive_negative_peaks_and_both_endpoints() {
    let mut samples = vec![0.0; 1000];
    samples[0] = 11.0;
    samples[999] = -13.0;
    samples[101] = 100.0;
    samples[102] = -90.0;
    samples[890] = 80.0;
    let plot = VisualizationService::default()
        .waveform(
            &recording(100.0, &[("Fp1", samples)]),
            &WaveformRequest {
                max_points_per_channel: 10,
                ..Default::default()
            },
        )
        .unwrap();
    let points = &plot.traces[0].points;
    assert!(points.len() <= 10);
    assert_eq!(points.first().unwrap().y, 11.0);
    assert_eq!(points.last().unwrap().y, -13.0);
    for amplitude in [100.0, -90.0, 80.0] {
        assert!(points.iter().any(|p| p.y == amplitude));
    }
    assert!(points.windows(2).all(|p| p[0].x < p[1].x));
    assert_eq!(plot.traces[0].source_samples, 1000);
}

#[test]
fn every_bucket_preserves_peak_order_even_when_max_precedes_min() {
    let input = recording(
        1.0,
        &[("Fp1", vec![0.0, 20.0, -30.0, 0.0, 4.0, -5.0, 0.0, 0.0])],
    );
    let plot = VisualizationService::default()
        .waveform(
            &input,
            &WaveformRequest {
                end_seconds: 8.0,
                max_points_per_channel: 6,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        plot.traces[0]
            .points
            .iter()
            .map(|p| p.x)
            .collect::<Vec<_>>(),
        [0.0, 1.0, 2.0, 4.0, 5.0, 7.0]
    );
}

#[test]
fn units_are_normalized_without_changing_source_and_non_eeg_is_excluded() {
    let mut input = recording(
        100.0,
        &[
            ("Fp1", vec![0.001; 100]),
            ("Cz", vec![1.0; 100]),
            ("EOG", vec![1.0; 100]),
        ],
    );
    input.channels[0].unit = "mV".into();
    input.channels[2].kind = ChannelKind::Eog;
    let plot = VisualizationService::default()
        .waveform(&input, &WaveformRequest::default())
        .unwrap();
    assert_eq!(plot.traces.len(), 2);
    assert!((plot.traces[0].points[0].y - 1.0).abs() < 1e-6);
    assert_eq!(input.channels[0].unit, "mV");
    for unit in ["V", "µV", "μV", "uV"] {
        input.channels[0].unit = unit.into();
        assert!(
            VisualizationService::default()
                .waveform(&input, &WaveformRequest::default())
                .is_ok()
        );
    }
}

#[test]
fn psd_uses_existing_bins_and_explicit_decibel_reference_without_mutating_power() {
    let input = features();
    let before = input.clone();
    let plot = VisualizationService::default()
        .psd(
            &input,
            &PsdRequest {
                channels: vec!["Fp1".into()],
                scale: PsdScale::Decibel {
                    floor_uv2_per_hz: 0.01,
                },
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(plot.traces[0].raw_uv2_per_hz, [0.0, 1.0, 10.0, 100.0]);
    assert_eq!(
        plot.traces[0]
            .points
            .iter()
            .map(|p| p.y)
            .collect::<Vec<_>>(),
        [-20.0, 0.0, 10.0, 20.0]
    );
    assert_eq!(plot.traces[0].floored_bins, 1);
    assert_eq!(input, before);
}

#[test]
fn linear_psd_is_identity_and_range_is_closed() {
    let plot = VisualizationService::default()
        .psd(
            &features(),
            &PsdRequest {
                min_hz: 1.0,
                max_hz: 10.0,
                scale: PsdScale::Linear,
                channels: vec!["Fp1".into()],
            },
        )
        .unwrap();
    assert_eq!(
        plot.traces[0].points,
        [PlotPoint { x: 1.0, y: 1.0 }, PlotPoint { x: 10.0, y: 10.0 }]
    );
    assert_eq!(plot.traces[0].floored_bins, 0);
}

#[test]
fn topomap_mask_is_convex_hull_and_interpolation_stays_within_original_range() {
    let plot = VisualizationService::default()
        .topomap(
            &features(),
            &triangle(),
            &TopomapRequest {
                measure: BandPowerMeasure::Absolute,
                resolution: 21,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(plot.values.len(), 441);
    assert_eq!((plot.color_min, plot.color_max), (2.0, 10.0));
    // 21x21 中心在 (0,0)，三个电极等距，IDW 结果应为 6。
    close(plot.values[10 * 21 + 10].unwrap(), 6.0);
    assert!(plot.values[0].is_none());
    assert!(plot.values[20 * 21 + 10].is_none()); // y<0，在头圆内但在三角凸包外。
    assert!(
        plot.values
            .iter()
            .flatten()
            .all(|v| (2.0..=10.0).contains(v))
    );
    assert!(!plot.schematic_layout);
}

#[test]
fn head_circle_coverage_fills_circle_without_changing_electrode_values_or_scale() {
    let service = VisualizationService::default();
    let request = TopomapRequest {
        resolution: 21,
        measure: BandPowerMeasure::Absolute,
        ..Default::default()
    };
    let hull = service.topomap(&features(), &triangle(), &request).unwrap();
    let circle = service
        .topomap(
            &features(),
            &triangle(),
            &TopomapRequest {
                coverage: TopomapCoverage::HeadCircle,
                ..request
            },
        )
        .unwrap();
    assert_eq!(circle.electrodes, hull.electrodes);
    assert_eq!(
        (circle.color_min, circle.color_max),
        (hull.color_min, hull.color_max)
    );
    assert_eq!(circle.skipped_channels, hull.skipped_channels);
    for (index, value) in circle.values.iter().enumerate() {
        let x = -1.0 + 2.0 * ((index % 21) as f64 + 0.5) / 21.0;
        let y = 1.0 - 2.0 * ((index / 21) as f64 + 0.5) / 21.0;
        assert_eq!(value.is_some(), x.hypot(y) <= 1.0);
        if let Some(value) = value {
            assert!((2.0..=10.0).contains(value));
        }
        if let Some(original) = hull.values[index] {
            assert_eq!(*value, Some(original));
        }
    }
    assert!(hull.values[20 * 21 + 10].is_none());
    assert!(circle.values[20 * 21 + 10].is_some());
    let svg = SvgRenderer::default()
        .render(&EegPlot::Topomap(circle.clone()), SvgOptions::default())
        .unwrap();
    assert!(svg.contains("head circle (display extrapolation)"));
    let json = serde_json::to_string(&circle).unwrap();
    assert_eq!(serde_json::from_str::<TopomapPlot>(&json).unwrap(), circle);
    // 旧 DTO 未记录 coverage 时仍按原凸包语义解释。
    let mut old = serde_json::to_value(&hull).unwrap();
    old.as_object_mut().unwrap().remove("coverage");
    assert_eq!(
        serde_json::from_value::<TopomapPlot>(old).unwrap().coverage,
        TopomapCoverage::ElectrodeHull
    );
}

#[test]
fn constant_topomap_keeps_original_color_scale_and_relative_values() {
    let mut input = features();
    for bands in input
        .spectral
        .as_mut()
        .unwrap()
        .band_power_by_channel
        .values_mut()
    {
        bands.get_mut(&FrequencyBand::Alpha).unwrap().relative =
            domain::UnitInterval::new("test", 0.4).unwrap();
    }
    let plot = VisualizationService::default()
        .topomap(
            &input,
            &triangle(),
            &TopomapRequest {
                resolution: 21,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!((plot.color_min, plot.color_max), (0.4, 0.4));
    assert!(plot.values.iter().flatten().all(|v| *v == 0.4));
    let svg = SvgRenderer::default()
        .render(&EegPlot::Topomap(plot), SvgOptions::default())
        .unwrap();
    assert!(svg.contains("ratio (0..1)"));
    assert!(!svg.contains("NaN") && !svg.contains("inf"));
}

#[test]
fn missing_position_requires_explicit_skip_and_is_auditable() {
    let mut input = features();
    let band = input.spectral.as_ref().unwrap().band_power_by_channel["Cz"].clone();
    input
        .spectral
        .as_mut()
        .unwrap()
        .band_power_by_channel
        .insert("Unknown".into(), band);
    let service = VisualizationService::default();
    assert!(matches!(
        service.topomap(&input, &triangle(), &TopomapRequest::default()),
        Err(VisualizationError::MissingPosition(_))
    ));
    let plot = service
        .topomap(
            &input,
            &triangle(),
            &TopomapRequest {
                missing_positions: MissingPositionPolicy::Skip,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(plot.skipped_channels, ["Unknown"]);
    let svg = SvgRenderer::default()
        .render(&EegPlot::Topomap(plot), SvgOptions::default())
        .unwrap();
    assert!(svg.contains("Skipped 1 channels") && svg.contains("Unknown"));
}
