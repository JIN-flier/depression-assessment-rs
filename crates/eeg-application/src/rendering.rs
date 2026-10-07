//! 后台准备 P7 图表 DTO：选点、物理单位换算、PSD 投影与地形插值。
//! 像素绘制由桌面适配器负责；不生成 SVG，也不修改原始样本/领域特征。
use crate::*;
use domain::{ChannelKind, EegRecording};
use eeg_visualization::*;

pub(crate) fn render(
    raw: &EegRecording,
    analysis: Option<&AnalysisResult>,
    view: &ViewRequest,
) -> AppResult<PlotData> {
    view.validate()?;
    let input = if view.processed {
        &analysis
            .ok_or(AppError::InvalidState("请先完成分析，再查看处理后波形"))?
            .processed
    } else {
        raw
    };
    let channels = view.channels.clone().unwrap_or_else(|| {
        input
            .channels
            .iter()
            .filter(|c| c.kind == ChannelKind::Eeg)
            .map(|c| c.label.clone())
            .collect()
    });
    let service = VisualizationService::default();
    let mut plots = PlotData::default();
    // P7 的空列表表示“所有 EEG”，因此“全不选”必须在应用层拦截，
    // 否则用户取消最后一个复选框后通道会重新全部出现。
    if !channels.is_empty() {
        plots.waveform = Some(
            service
                .waveform(
                    input,
                    &WaveformRequest {
                        start_seconds: view.start_seconds,
                        end_seconds: view.start_seconds + view.window_seconds,
                        channels: channels.clone(),
                        // 为高分辨率显示保留更多包络点，且服从 P7 总点数预算。
                        max_points_per_channel: (service.limits().max_output_points
                            / channels.len())
                        .min(8192),
                    },
                )
                .map_err(|e| AppError::service("波形准备", e))?,
        );
    }
    if let Some(analysis) = analysis {
        let included: Vec<_> = channels
            .iter()
            .filter(|label| {
                analysis
                    .features
                    .spectral
                    .as_ref()
                    .is_some_and(|s| s.psd_by_channel.contains_key(label.as_str()))
            })
            .cloned()
            .collect();
        if !included.is_empty() {
            let psd = service
                .psd(
                    &analysis.features,
                    &PsdRequest {
                        channels: included,
                        max_hz: 45.0_f64.min(analysis.processed.sampling_rate_hz / 2.0),
                        ..Default::default()
                    },
                )
                .map_err(|e| AppError::service("PSD 准备", e))?;
            plots.psd = Some(psd);
        } else if !channels.is_empty() {
            plots
                .warnings
                .push("所选通道未参与特征提取，PSD 不可用".into());
        }
        // 地形图总是使用所有参与分析的导联，避免波形筛选导致空间信息被截断。
        let topomap = service.topomap(
            &analysis.features,
            &ElectrodeLayout::schematic_10_20(),
            &TopomapRequest {
                // 桌面默认填满头圆；这是展示插值，原始电极功率和固定色标不变。
                coverage: TopomapCoverage::HeadCircle,
                band: view.band,
                measure: if view.relative_power {
                    BandPowerMeasure::Relative
                } else {
                    BandPowerMeasure::Absolute
                },
                // 256² × 最多 19 个示意电极，仍在 P7 的插值预算内。
                resolution: 256,
                missing_positions: MissingPositionPolicy::Skip,
                ..Default::default()
            },
        );
        match topomap {
            Ok(plot) => {
                if !plot.skipped_channels.is_empty() {
                    plots.warnings.push(format!(
                        "地形图跳过未知位置：{}",
                        plot.skipped_channels.join(", ")
                    ));
                }
                plots.topomap = Some(plot);
                plots
                    .warnings
                    .push("地形图采用 19 导联二维示意坐标和 IDW 展示插值".into());
            }
            Err(e) => plots.warnings.push(format!("地形图不可用：{e}")),
        }
    }
    Ok(plots)
}
