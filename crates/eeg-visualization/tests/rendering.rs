mod support;
use eeg_visualization::*;
use support::*;

#[test]
fn svg_has_units_dimensions_axes_head_orientation_and_no_external_assets() {
    let service = VisualizationService::default();
    let plots = [
        EegPlot::Waveform(
            service
                .waveform(
                    &recording(10.0, &[("Fp1", vec![0.0; 20])]),
                    &WaveformRequest::default(),
                )
                .unwrap(),
        ),
        EegPlot::Psd(
            service
                .psd(
                    &features(),
                    &PsdRequest {
                        channels: vec!["Fp1".into()],
                        ..Default::default()
                    },
                )
                .unwrap(),
        ),
        EegPlot::Topomap(
            service
                .topomap(
                    &features(),
                    &ElectrodeLayout::schematic_10_20(),
                    &TopomapRequest::default(),
                )
                .unwrap(),
        ),
    ];
    for (index, plot) in plots.iter().enumerate() {
        let svg = SvgRenderer::default()
            .render(
                plot,
                SvgOptions {
                    width: 800,
                    height: 600,
                },
            )
            .unwrap();
        assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
        assert!(svg.contains("viewBox=\"0 0 800 600\"") && svg.ends_with("</svg>"));
        assert!(!svg.contains("<script") && !svg.contains("href=") && !svg.contains("<image"));
        assert!(!svg.contains("NaN") && !svg.contains("Infinity"));
        match index {
            0 => assert!(svg.contains("Time (s)") && svg.contains("uV")),
            1 => assert!(
                svg.contains("Frequency (Hz)")
                    && svg.contains("dB re 1 uV²/Hz")
                    && svg.contains("clipped bins")
            ),
            _ => assert!(
                svg.contains("Schematic coordinates")
                    && svg.contains("head-mask")
                    && svg.contains("Fp1")
            ),
        }
    }
}

#[test]
fn arbitrary_channel_and_layout_text_is_xml_escaped() {
    let label = "<&\"'Fp1><script>alert(1)</script>";
    let plot = VisualizationService::default()
        .waveform(
            &recording(10.0, &[(label, vec![1.0; 20])]),
            &WaveformRequest::default(),
        )
        .unwrap();
    let svg = SvgRenderer::default()
        .render(&EegPlot::Waveform(plot), SvgOptions::default())
        .unwrap();
    assert!(svg.contains("&lt;&amp;&quot;&apos;Fp1&gt;"));
    assert!(!svg.contains("<script>"));
    let mut layout = triangle();
    layout.name = "<unsafe & layout>".into();
    let plot = VisualizationService::default()
        .topomap(&features(), &layout, &TopomapRequest::default())
        .unwrap();
    let svg = SvgRenderer::default()
        .render(&EegPlot::Topomap(plot), SvgOptions::default())
        .unwrap();
    assert!(svg.contains("&lt;unsafe &amp; layout&gt;"));
}

#[test]
fn renderer_rejects_mutated_dtos_and_invalid_sizes() {
    let service = VisualizationService::default();
    let renderer = SvgRenderer::default();
    let waveform = service
        .waveform(
            &recording(10.0, &[("Fp1", vec![1.0; 20])]),
            &WaveformRequest::default(),
        )
        .unwrap();
    for mutate in 0..5 {
        let mut plot = waveform.clone();
        match mutate {
            0 => plot.traces[0].points[0].y = f64::NAN,
            1 => plot.traces[0].points.reverse(),
            2 => plot.traces[0].points[0].x = -1.0,
            3 => plot.traces[0].source_samples = 0,
            _ => plot.traces.push(plot.traces[0].clone()),
        }
        assert!(
            renderer
                .render(&EegPlot::Waveform(plot), SvgOptions::default())
                .is_err()
        );
    }
    let mut psd = service.psd(&features(), &PsdRequest::default()).unwrap();
    psd.traces[0].points[0].y += 5.0;
    assert!(
        renderer
            .render(&EegPlot::Psd(psd), SvgOptions::default())
            .is_err()
    );
    let topomap = service
        .topomap(&features(), &triangle(), &TopomapRequest::default())
        .unwrap();
    for mutate in 0..6 {
        let mut plot = topomap.clone();
        match mutate {
            0 => {
                plot.values.pop();
            }
            1 => plot.color_max = f64::NAN,
            2 => plot.values[0] = Some(plot.color_min), // 凸包外不能伪装成有效图形区域。
            3 => plot.electrodes[0].x = 2.0,
            4 => plot.color_min = 0.0,
            _ => {
                plot.electrodes[0].x = plot.electrodes[1].x;
                plot.electrodes[0].y = plot.electrodes[1].y;
            }
        }
        assert!(
            renderer
                .render(&EegPlot::Topomap(plot), SvgOptions::default())
                .is_err()
        );
    }
    assert!(
        renderer
            .render(
                &EegPlot::Waveform(waveform),
                SvgOptions {
                    width: 1,
                    height: 1
                }
            )
            .is_err()
    );
}

#[test]
fn renderer_enforces_its_own_point_grid_and_channel_density_budgets() {
    let service = VisualizationService::default();
    let plot = EegPlot::Psd(service.psd(&features(), &PsdRequest::default()).unwrap());
    let renderer = SvgRenderer::new(VisualizationLimits {
        max_output_points: 3,
        ..Default::default()
    })
    .unwrap();
    assert!(matches!(
        renderer.render(&plot, SvgOptions::default()),
        Err(VisualizationError::LimitExceeded(_))
    ));
    assert!(
        SvgRenderer::default()
            .render(
                &plot,
                SvgOptions {
                    width: 640,
                    height: 240
                }
            )
            .is_err()
    );
    let plot = EegPlot::Topomap(
        service
            .topomap(&features(), &triangle(), &TopomapRequest::default())
            .unwrap(),
    );
    let renderer = SvgRenderer::new(VisualizationLimits {
        max_grid_cells: 10,
        ..Default::default()
    })
    .unwrap();
    assert!(matches!(
        renderer.render(&plot, SvgOptions::default()),
        Err(VisualizationError::LimitExceeded(_))
    ));
}
