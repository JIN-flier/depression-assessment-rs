//! 与 UI/绘图库无关的显示 DTO。记录 ID 与显示参数随结果保留，便于缓存与回放。
use crate::{BandPowerMeasure, PsdScale};
use domain::{FrequencyBand, RecordingId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlotPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaveformTrace {
    pub channel: String,
    /// x 为原录制中的绝对时间（秒），y 为 uV，不加展示偏移。
    pub points: Vec<PlotPoint>,
    pub source_samples: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaveformPlot {
    pub recording_id: RecordingId,
    /// 实际显示范围；请求超过录制末端时截至 sample_count / fs。
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub traces: Vec<WaveformTrace>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PsdTrace {
    pub channel: String,
    /// x 为 Hz，y 是线性 PSD 或 dB 显示值。
    pub points: Vec<PlotPoint>,
    /// 与 points 一一对应，保持原始 uV²/Hz，无对数或 floor 修改。
    pub raw_uv2_per_hz: Vec<f64>,
    pub floored_bins: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PsdPlot {
    pub recording_id: RecordingId,
    pub scale: PsdScale,
    pub traces: Vec<PsdTrace>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TopomapElectrode {
    pub channel: String,
    pub x: f64,
    pub y: f64,
    pub value: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TopomapPlot {
    pub recording_id: RecordingId,
    pub band: FrequencyBand,
    pub measure: BandPowerMeasure,
    pub layout_name: String,
    /// true 表示通用示意坐标，不能解释为该受试者真实空间测量。
    pub schematic_layout: bool,
    pub electrodes: Vec<TopomapElectrode>,
    pub skipped_channels: Vec<String>,
    pub resolution: usize,
    /// 随 DTO 传递覆盖语义，渲染器不需要猜测空格点的含义。
    #[serde(default)]
    pub coverage: crate::TopomapCoverage,
    /// 行主序，从 y=+1 向 y=-1，x=-1 向 x=+1；None 区域透明。
    /// IDW 是展示插值，不是源定位、连接性或新生理特征。
    pub values: Vec<Option<f64>>,
    /// 来自电极原始值，用作固定色标；常量数据允许 min == max。
    pub color_min: f64,
    pub color_max: f64,
}

/// 应用层可以统一缓存/传递，渲染后端只需接收这一类型。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum EegPlot {
    Waveform(WaveformPlot),
    Psd(PsdPlot),
    Topomap(TopomapPlot),
}
