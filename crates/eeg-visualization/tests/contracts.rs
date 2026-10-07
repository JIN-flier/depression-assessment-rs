//! 非法公开字段、预算、序列化回放与后端解耦契约。
mod support;
use eeg_visualization::*;
use support::*;

#[test]
fn invalid_waveform_inputs_return_errors_without_panics() {
    let service = VisualizationService::default();
    let valid = recording(10.0, &[("Fp1", vec![0.0; 20])]);
    for mutate in 0..7 {
        let mut input = valid.clone();
        match mutate {
            0 => input.sampling_rate_hz = f64::NAN,
            1 => input.samples.clear(),
            2 => input.samples[0][0] = f32::INFINITY,
            3 => input.duration_seconds = 10.0,
            4 => input.channels[0].label.clear(),
            5 => input.channels[0].unit = "count".into(),
            _ => input.samples[0].clear(),
        }
        assert!(
            service
                .waveform(&input, &WaveformRequest::default())
                .is_err(),
            "case {mutate}"
        );
    }
    for request in [
        WaveformRequest {
            start_seconds: -1.0,
            ..Default::default()
        },
        WaveformRequest {
            end_seconds: f64::NAN,
            ..Default::default()
        },
        WaveformRequest {
            max_points_per_channel: 3,
            ..Default::default()
        },
        WaveformRequest {
            channels: vec!["Fp1".into(), "Fp1".into()],
            ..Default::default()
        },
        WaveformRequest {
            channels: vec!["missing".into()],
            ..Default::default()
        },
        WaveformRequest {
            start_seconds: 3.0,
            end_seconds: 4.0,
            ..Default::default()
        },
    ] {
        assert!(service.waveform(&valid, &request).is_err());
    }
}

#[test]
fn malformed_psd_and_missing_spectral_are_rejected() {
    let service = VisualizationService::default();
    for mutate in 0..7 {
        let mut input = features();
        match mutate {
            0 => input.spectral = None,
            1 => input.spectral.as_mut().unwrap().frequencies_hz[1] = 0.0,
            2 => input.spectral.as_mut().unwrap().frequencies_hz[1] = f64::INFINITY,
            3 => {
                input
                    .spectral
                    .as_mut()
                    .unwrap()
                    .psd_by_channel
                    .get_mut("Fp1")
                    .unwrap()
                    .pop();
            }
            4 => {
                input
                    .spectral
                    .as_mut()
                    .unwrap()
                    .psd_by_channel
                    .get_mut("Fp1")
                    .unwrap()[0] = -1.0
            }
            5 => {
                input
                    .spectral
                    .as_mut()
                    .unwrap()
                    .psd_by_channel
                    .get_mut("Fp1")
                    .unwrap()[0] = f64::NAN
            }
            _ => {
                input
                    .provenance
                    .parameters
                    .insert("psd_unit".into(), "V^2/Hz".into());
            }
        }
        assert!(
            service.psd(&input, &PsdRequest::default()).is_err(),
            "case {mutate}"
        );
    }
    for floor in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(
            service
                .psd(
                    &features(),
                    &PsdRequest {
                        scale: PsdScale::Decibel {
                            floor_uv2_per_hz: floor
                        },
                        ..Default::default()
                    }
                )
                .is_err()
        );
    }
    assert!(matches!(
        service.psd(
            &features(),
            &PsdRequest {
                min_hz: 30.0,
                max_hz: 40.0,
                ..Default::default()
            }
        ),
        Err(VisualizationError::EmptyRange)
    ));
}

#[test]
fn invalid_layouts_and_degenerate_geometry_are_rejected() {
    let service = VisualizationService::default();
    for mutate in 0..6 {
        let mut layout = triangle();
        match mutate {
            0 => layout.positions[0].x = f64::NAN,
            1 => layout.positions[0].x = 2.0,
            2 => layout.positions[1].channel = "Fp1".into(),
            3 => {
                layout.positions[1].x = layout.positions[0].x;
                layout.positions[1].y = layout.positions[0].y;
            }
            4 => layout.positions[2].y = 0.0,
            _ => layout.positions.truncate(2),
        }
        assert!(
            service
                .topomap(&features(), &layout, &TopomapRequest::default())
                .is_err(),
            "case {mutate}"
        );
    }
    assert!(matches!(
        service.topomap(
            &features(),
            &triangle(),
            &TopomapRequest {
                band: domain::FrequencyBand::Beta,
                ..Default::default()
            }
        ),
        Err(VisualizationError::MissingBand { .. })
    ));
    assert!(
        service
            .topomap(
                &features(),
                &triangle(),
                &TopomapRequest {
                    resolution: usize::MAX,
                    ..Default::default()
                }
            )
            .is_err()
    );
    let mut input = features();
    input
        .spectral
        .as_mut()
        .unwrap()
        .band_power_by_channel
        .get_mut("Fp1")
        .unwrap()
        .get_mut(&domain::FrequencyBand::Alpha)
        .unwrap()
        .absolute = -1.0;
    assert!(
        service
            .topomap(&input, &triangle(), &TopomapRequest::default())
            .is_err()
    );
}

#[test]
fn budgets_bound_input_output_grid_and_interpolation_before_allocation() {
    let input = recording(10.0, &[("Fp1", vec![0.0; 20])]);
    let limits = VisualizationLimits {
        max_input_values: 10,
        ..Default::default()
    };
    assert!(matches!(
        VisualizationService::new(limits)
            .unwrap()
            .waveform(&input, &WaveformRequest::default()),
        Err(VisualizationError::LimitExceeded(_))
    ));
    let limits = VisualizationLimits {
        max_output_points: 3,
        ..Default::default()
    };
    let service = VisualizationService::new(limits).unwrap();
    assert!(matches!(
        service.waveform(&input, &WaveformRequest::default()),
        Err(VisualizationError::LimitExceeded(_))
    ));
    assert!(matches!(
        service.psd(&features(), &PsdRequest::default()),
        Err(VisualizationError::LimitExceeded(_))
    ));
    for limits in [
        VisualizationLimits {
            max_grid_cells: 100,
            ..Default::default()
        },
        VisualizationLimits {
            max_interpolation_operations: 100,
            ..Default::default()
        },
    ] {
        assert!(matches!(
            VisualizationService::new(limits).unwrap().topomap(
                &features(),
                &triangle(),
                &TopomapRequest::default()
            ),
            Err(VisualizationError::LimitExceeded(_))
        ));
    }
    assert!(
        VisualizationService::new(VisualizationLimits {
            max_grid_cells: 0,
            ..Default::default()
        })
        .is_err()
    );
}

#[test]
fn serializable_plots_replay_deterministically_and_services_are_object_safe() {
    let service: Box<dyn EegVisualizer> = Box::new(VisualizationService::default());
    let renderer: Box<dyn PlotRenderer> = Box::new(SvgRenderer::default());
    let plots = [
        EegPlot::Waveform(
            service
                .waveform(
                    &recording(10.0, &[("Fp1", vec![0.0; 20])]),
                    &WaveformRequest::default(),
                )
                .unwrap(),
        ),
        EegPlot::Psd(service.psd(&features(), &PsdRequest::default()).unwrap()),
        EegPlot::Topomap(
            service
                .topomap(&features(), &triangle(), &TopomapRequest::default())
                .unwrap(),
        ),
    ];
    for plot in plots {
        let replay: EegPlot = serde_json::from_str(&serde_json::to_string(&plot).unwrap()).unwrap();
        assert_eq!(plot, replay);
        assert_eq!(
            renderer.render(&plot, SvgOptions::default()).unwrap(),
            renderer.render(&replay, SvgOptions::default()).unwrap()
        );
    }
    assert!(serde_json::from_str::<WaveformRequest>(r#"{"start_seconds":0,"end_seconds":1,"channels":[],"max_points_per_channel":10,"typo":true}"#).is_err());
}

#[test]
fn shared_services_produce_identical_results_across_threads() {
    let service = std::sync::Arc::new(VisualizationService::default());
    let input = std::sync::Arc::new(features());
    let expected = service.psd(&input, &PsdRequest::default()).unwrap();
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let service = service.clone();
            let input = input.clone();
            std::thread::spawn(move || service.psd(&input, &PsdRequest::default()).unwrap())
        })
        .collect();
    for handle in handles {
        assert_eq!(expected, handle.join().unwrap());
    }
}
