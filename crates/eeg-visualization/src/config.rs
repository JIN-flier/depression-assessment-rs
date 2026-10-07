//! 配置与图形无关：调用者明确指定范围、通道顺序和显示变换。
use domain::FrequencyBand;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VisualizationLimits {
    /// 限制通道/坐标数量，约束标签查找和几何处理的工作量。
    pub max_channels: usize,
    pub max_input_values: usize,
    pub max_output_points: usize,
    pub max_grid_cells: usize,
    /// IDW 的工程操作预算为 grid_cells * electrode_count。
    pub max_interpolation_operations: usize,
}

impl Default for VisualizationLimits {
    fn default() -> Self {
        Self {
            max_channels: 256,
            max_input_values: 64_000_000,
            max_output_points: 1_000_000,
            max_grid_cells: 262_144,
            max_interpolation_operations: 16_000_000,
        }
    }
}

/// 所有范围使用实际物理值；波形窗口为 [start_seconds, end_seconds)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaveformRequest {
    pub start_seconds: f64,
    pub end_seconds: f64,
    /// 空列表按录制顺序选择全部 EEG；非空列表保留用户顺序。
    pub channels: Vec<String>,
    /// 每通道最大点数，至少 4。降采样保留每桶 min/max 及窗口首尾点。
    pub max_points_per_channel: usize,
}

impl Default for WaveformRequest {
    fn default() -> Self {
        Self {
            start_seconds: 0.0,
            end_seconds: 10.0,
            channels: Vec::new(),
            max_points_per_channel: 2_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PsdScale {
    Linear,
    /// 绘图值为 10*log10(max(PSD, floor_uv2_per_hz) / (1 uV²/Hz))。
    /// floor 仅防止 log(0)，原始 PSD 与裁剪计数同时保留。
    Decibel {
        floor_uv2_per_hz: f64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PsdRequest {
    /// 频率显示范围闭区间 [min_hz, max_hz]，不重新积分或计算 PSD。
    pub min_hz: f64,
    pub max_hz: f64,
    /// 空列表使用领域 BTreeMap 的排序；应用层可传 P6 audit 的通道顺序。
    pub channels: Vec<String>,
    pub scale: PsdScale,
}

impl Default for PsdRequest {
    fn default() -> Self {
        Self {
            min_hz: 0.0,
            max_hz: 45.0,
            channels: Vec::new(),
            scale: PsdScale::Decibel {
                floor_uv2_per_hz: 1e-12,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BandPowerMeasure {
    Absolute,
    Relative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingPositionPolicy {
    /// 默认拒绝，避免把未知电极擅自投到示意位置。
    Error,
    /// 显式跳过，并在数据和 SVG 中保留 skipped_channels 提示。
    Skip,
}

/// 展示插值的覆盖区域；扩展到头圆不会生成新的测量值或分析特征。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TopomapCoverage {
    /// 保留电极覆盖之外的透明区域，也作为旧序列化数据的兼容默认值。
    #[default]
    ElectrodeHull,
    /// 在整个单位头圆内进行 IDW 展示插值，圆外仍透明。
    HeadCircle,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopomapRequest {
    pub band: FrequencyBand,
    pub measure: BandPowerMeasure,
    pub channels: Vec<String>,
    /// 行列均为 resolution，按像素中心采样。
    pub resolution: usize,
    #[serde(default)]
    pub coverage: TopomapCoverage,
    pub missing_positions: MissingPositionPolicy,
}

impl Default for TopomapRequest {
    fn default() -> Self {
        Self {
            band: FrequencyBand::Alpha,
            measure: BandPowerMeasure::Relative,
            channels: Vec::new(),
            resolution: 64,
            coverage: TopomapCoverage::ElectrodeHull,
            missing_positions: MissingPositionPolicy::Error,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SvgOptions {
    pub width: u32,
    pub height: u32,
}

impl Default for SvgOptions {
    fn default() -> Self {
        Self {
            width: 960,
            height: 640,
        }
    }
}
