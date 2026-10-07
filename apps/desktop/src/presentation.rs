//! 只负责展示投影，不推断风险、诊断或修改信号。后台准备 UI 无关 DTO，
//! 独立 raster 后端写入 SharedPixelBuffer，再创建 Slint Image，无文件 IO。
use crate::{
    AppWindow, ChannelOption, MapLabel, plot_renderer::PlotRenderer, raster,
    render_size::RenderTargets,
};
use eeg_application::*;
use slint::{Image, ModelRc, VecModel};
use std::{fmt::Write, rc::Rc};

/// 无持续宿主的初次投影。DesktopApp 使用下方缓存版本接入视区/DPI 变化。
pub fn apply_snapshot(window: &AppWindow, state: &AppSnapshot) -> Result<(), String> {
    apply_snapshot_with_renderer(window, state, &mut PlotRenderer::default())
}
pub(crate) fn apply_snapshot_with_renderer(
    window: &AppWindow,
    state: &AppSnapshot,
    renderer: &mut PlotRenderer,
) -> Result<(), String> {
    window.set_project_path(
        state
            .project_path
            .as_ref()
            .map_or(String::new(), |p| p.display().to_string())
            .into(),
    );
    window.set_project_open(state.project_path.is_some());
    window.set_subject_selected(state.selected_subject.is_some());
    window.set_recording_loaded(state.raw.is_some());
    window.set_analysis_ready(state.analysis.is_some());
    window.set_processed_saved(state.saved_processed.is_some());
    window.set_confirm_delete(false);
    window.set_subject_items(strings(state.subjects.iter().map(|s| {
        format!(
            "{} · {}岁 · {:?}",
            s.id,
            s.age.map_or("未知".into(), |a| a.to_string()),
            s.sex
        )
    })));
    window.set_recording_items(strings(state.recordings.iter().map(|r| {
        format!(
            "{} · {:.2}s · {:.0}Hz · {:?}",
            r.id, r.duration_seconds, r.sampling_rate_hz, r.recording_state
        )
    })));
    window.set_subject_index(
        state
            .subjects
            .iter()
            .position(|s| Some(s.id) == state.selected_subject)
            .map_or(-1, |i| i as i32),
    );
    window.set_recording_index(
        state
            .recordings
            .iter()
            .position(|r| state.raw.as_ref().is_some_and(|raw| raw.id == r.id))
            .map_or(-1, |i| i as i32),
    );
    if let Some(subject) = state
        .subjects
        .iter()
        .find(|s| Some(s.id) == state.selected_subject)
    {
        window.set_subject_age(subject.age.map_or(String::new(), |a| a.to_string()).into());
        window.set_sex_index(match subject.sex {
            Sex::Unknown => 0,
            Sex::Female => 1,
            Sex::Male => 2,
            Sex::Intersex => 3,
            Sex::Other => 4,
        });
        window.set_subject_notes(
            subject
                .metadata
                .get("notes")
                .cloned()
                .unwrap_or_default()
                .into(),
        );
        window.set_subject_summary(
            format!("匿名 ID：{}\n创建时间：{}", subject.id, subject.created_at).into(),
        );
    } else {
        window.set_subject_age("".into());
        window.set_sex_index(0);
        window.set_subject_notes("".into());
        window.set_subject_summary("尚未选择受试者".into());
    }
    let summary = state.raw.as_ref().map_or("尚未加载录制".into(), |raw| format!(
        "录制 ID：{}\n通道：{}\n采样率：{} Hz\n时长：{:.3} s\n设备：{}\n电极系统：{}\n采集时间：{}\n状态：{}\n导入格式：{}\n处理历史：{} 步",
        raw.id, raw.channels.iter().map(|c| c.label.as_str()).collect::<Vec<_>>().join(", "), raw.sampling_rate_hz,
        raw.duration_seconds, raw.metadata.device.as_deref().unwrap_or("未知"), raw.metadata.electrode_system.as_deref().unwrap_or("未知"),
        raw.metadata.recorded_at.map_or("未知".into(), |t| t.to_rfc3339()), raw.metadata.extra.get("recording_condition").map_or("未知", String::as_str),
        raw.metadata.source_format.as_deref().unwrap_or("未知"), raw.processing_history.len()));
    window.set_recording_summary(summary.clone().into());
    window.set_recording_overview(
        state
            .raw
            .as_ref()
            .map_or("尚未加载录制".into(), |raw| {
                format!(
                    "{} 通道 · {} Hz · {:.2} s",
                    raw.channels.len(),
                    raw.sampling_rate_hz,
                    raw.duration_seconds
                )
            })
            .into(),
    );
    window.set_start_seconds(state.view.start_seconds.to_string().into());
    window.set_window_seconds(state.view.window_seconds.to_string().into());
    window.set_amplitude_uv(state.view.amplitude_uv.to_string().into());
    let options: Vec<_> = state.raw.as_ref().map_or_else(Vec::new, |raw| {
        raw.channels
            .iter()
            .filter(|c| c.kind == ChannelKind::Eeg)
            .map(|c| ChannelOption {
                label: c.label.clone().into(),
                selected: state
                    .view
                    .channels
                    .as_ref()
                    .is_none_or(|labels| labels.contains(&c.label)),
            })
            .collect()
    });
    window.set_selected_channel_count(options.iter().filter(|c| c.selected).count() as i32);
    window.set_channel_options(Rc::new(VecModel::from(options)).into());
    window.set_show_processed(state.view.processed);
    window.set_relative_power(state.view.relative_power);
    window.set_band_index(match state.view.band {
        FrequencyBand::Delta => 0,
        FrequencyBand::Theta => 1,
        FrequencyBand::Alpha => 2,
        FrequencyBand::Beta => 3,
        FrequencyBand::Gamma => 4,
    });
    let mut quality_text = String::from("质量阈值为工程启发式，用于信号质量控制。\n\n");
    if let Some(quality) = &state.quality {
        append_quality(&mut quality_text, "原始信号", quality);
    } else {
        quality_text.push_str("尚未进行质量评价\n");
    }
    if let Some(analysis) = &state.analysis {
        append_quality(&mut quality_text, "处理后信号", &analysis.processed_quality);
    }
    window.set_quality_summary(quality_text.clone().into());
    let mut bands = String::from("绝对功率：uV²；相对功率：%；默认分母为 0.5–30 Hz 范围积分。\n\n");
    let mut result = format!("{summary}\n\n{quality_text}");
    if let Some(analysis) = &state.analysis {
        if let Some(spectral) = &analysis.features.spectral {
            for (channel, values) in &spectral.band_power_by_channel {
                let _ = writeln!(bands, "{channel}");
                for (band, power) in values {
                    let _ = writeln!(
                        bands,
                        "  {band:?}: {:.6} uV² / {:.3}%",
                        power.absolute,
                        power.relative.get() * 100.0
                    );
                }
            }
            let _ = writeln!(
                result,
                "\n处理后录制 ID：{}\n处理后采样率：{} Hz\nPSD 频点：{}\n参与特征通道：{}\n",
                analysis.processed.id,
                analysis.processed.sampling_rate_hz,
                spectral.frequencies_hz.len(),
                spectral
                    .psd_by_channel
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        let _ = writeln!(
            result,
            "处理参数：{:#?}\n特征参数：{:#?}\n质量参数：{:#?}\n特征溯源：{:#?}\n处理历史：{:#?}\n{}",
            analysis.request.pipeline,
            analysis.request.features,
            analysis.request.quality,
            analysis.features.provenance,
            analysis.processed.processing_history,
            bands
        );
        if analysis.request.exclude_bad_channels {
            result
                .push_str("特征显式排除了原始质量判定的坏通道；实际排除列表见特征 provenance。\n");
        }
    } else {
        bands.push_str("完成分析后显示各通道 Delta / Theta / Alpha / Beta 功率。\n");
    }
    window.set_band_summary(bands.into());
    window.set_result_summary(result.into());
    window.set_plot_warning(state.plots.warnings.join("\n").into());
    renderer.refresh(window, state);
    Ok(())
}

pub(crate) fn render_plots(window: &AppWindow, state: &AppSnapshot, targets: RenderTargets) {
    // 空 DTO 必须产生空模型，防止上个录制的曲线留在窗口中。
    let waveforms = state.plots.waveform.as_ref().map_or_else(Vec::new, |plot| {
        raster::waveform_rows(plot, state.view.amplitude_uv, targets.waveform)
    });
    let psd = state
        .plots
        .psd
        .as_ref()
        .map_or_else(Vec::new, |plot| raster::psd_rows(plot, targets.psd));
    // 改变通道集合时从首行开始；时间导航/幅度调整保持当前垂直位置。
    if row_labels(&window.get_waveform_rows())
        != waveforms
            .iter()
            .map(|r| r.label.clone())
            .collect::<Vec<_>>()
    {
        window.set_waveform_scroll_y(0.0);
    }
    if row_labels(&window.get_psd_rows()) != psd.iter().map(|r| r.label.clone()).collect::<Vec<_>>()
    {
        window.set_psd_scroll_y(0.0);
    }
    window.set_has_psd(!psd.is_empty());
    window.set_waveform_rows(Rc::new(VecModel::from(waveforms)).into());
    window.set_psd_rows(Rc::new(VecModel::from(psd)).into());
    window.set_topomap_image(Image::default());
    window.set_topomap_caption("".into());
    window.set_topomap_labels(Rc::new(VecModel::<MapLabel>::default()).into());
    window.set_topomap_upper_label("".into());
    window.set_topomap_lower_label("".into());
    window.set_topomap_unit("".into());
    window.set_has_topomap(false);
    if let Some(plot) = &state.plots.topomap {
        window.set_topomap_image(Image::from_rgba8(raster::topomap_buffer(
            plot,
            targets.topomap,
        )));
        let multiplier = if plot.measure == BandPowerMeasure::Relative {
            100.0
        } else {
            1.0
        };
        let min = plot.color_min * multiplier;
        let max = plot.color_max * multiplier;
        window.set_topomap_upper_label(color_label(max, max - min).into());
        window.set_topomap_lower_label(color_label(min, max - min).into());
        window.set_topomap_unit(
            if plot.measure == BandPowerMeasure::Relative {
                "%"
            } else {
                "uV²"
            }
            .into(),
        );
        window.set_topomap_labels(
            Rc::new(VecModel::from(
                plot.electrodes
                    .iter()
                    .map(|e| MapLabel {
                        label: e.channel.clone().into(),
                        x: ((274.0 + e.x * 210.0) / 640.0) as f32,
                        y: ((240.0 - e.y * 210.0) / 480.0) as f32,
                    })
                    .collect::<Vec<_>>(),
            ))
            .into(),
        );
        window.set_topomap_caption(
            format!(
                "{:?} · {} · 10–20 示意位置",
                plot.band,
                if plot.measure == BandPowerMeasure::Relative {
                    "相对功率（%）"
                } else {
                    "绝对功率（uV²）"
                }
            )
            .into(),
        );
        window.set_has_topomap(true);
    }
}
// 小范围差异使用更多小数位，避免不同色标端点都被显示成同一个两位小数。
fn color_label(value: f64, span: f64) -> String {
    if value != 0.0 && !(0.001..1e5).contains(&value.abs()) {
        return format!("{value:.5e}");
    }
    let precision = if span > 0.0 {
        (-span.log10()).ceil().clamp(2.0, 8.0) as usize + 1
    } else {
        2
    };
    format!("{value:.precision$}")
}
fn row_labels(rows: &ModelRc<crate::ChannelPlot>) -> Vec<slint::SharedString> {
    use slint::Model;
    rows.iter().map(|row| row.label).collect()
}
fn strings(values: impl Iterator<Item = String>) -> ModelRc<slint::SharedString> {
    Rc::new(VecModel::from(values.map(Into::into).collect::<Vec<_>>())).into()
}
fn append_quality(out: &mut String, title: &str, quality: &SignalQuality) {
    let _ = writeln!(
        out,
        "{title}：{:.2}%\n坏通道：{}",
        quality.overall_score.get() * 100.0,
        if quality.bad_channels.is_empty() {
            "无".into()
        } else {
            quality.bad_channels.join(", ")
        }
    );
    for (channel, values) in &quality.channel_quality {
        let _ = writeln!(
            out,
            "{channel} 质量 {:.2}% | 缺失 {:.2}% | 噪声 {:.2}% | 工频 {:.2}%",
            values.score.get() * 100.0,
            values.missing_fraction.get() * 100.0,
            values.noisy_fraction.get() * 100.0,
            values.power_line_interference.get() * 100.0
        );
        for warning in &values.warnings {
            let _ = writeln!(out, "  [{}] {}", warning.code, warning.message);
        }
    }
    for warning in &quality.warnings {
        let _ = writeln!(out, "[{}] {}", warning.code, warning.message);
    }
    let _ = writeln!(
        out,
        "算法：{} {}\n",
        quality.provenance.algorithm, quality.provenance.algorithm_version
    );
}
