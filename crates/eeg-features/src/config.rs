//! 参数声明独立于算法执行。配置可序列化；构造提取器时统一验证数值约束。
use chrono::{DateTime, Utc};
use domain::FrequencyBand;
use serde::{Deserialize, Serialize};

/// 采用频率 bin 中心归属的 `[low_hz, high_hz)` 范围。
/// 当 high_hz 等于 Nyquist 时额外包含 Nyquist bin，保留其完整能量。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrequencyRange {
    pub low_hz: f64,
    pub high_hz: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BandDefinition {
    pub band: FrequencyBand,
    pub range: FrequencyRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Detrend {
    /// 逐窗减去未加窗样本的算术均值；这是局部去趋势，并非修改输入录制。
    Constant,
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WelchConfig {
    pub window_seconds: f64,
    /// overlap_samples = floor(window_samples * overlap_fraction)。
    pub overlap_fraction: f64,
    /// None 使用实际窗长，支持非二次幂；Some 必须 >= 窗长。
    /// 增大 FFT 长度只加密频率网格，不提高实际的频率分辨能力。
    pub fft_size: Option<usize>,
    pub detrend: Detrend,
}

impl Default for WelchConfig {
    fn default() -> Self {
        Self {
            window_seconds: 2.0,
            overlap_fraction: 0.5,
            fft_size: None,
            detrend: Detrend::Constant,
        }
    }
}

/// 在规划 FFT 或分配工作区之前检查预算；操作量是 N*ceil(log2(N))
/// 乘以窗口数和通道数的工程估算，不是实际 CPU 指令数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeatureLimits {
    pub max_total_samples: usize,
    pub max_fft_size: usize,
    pub max_windows: usize,
    pub max_output_values: usize,
    pub max_fft_operations: usize,
}

impl Default for FeatureLimits {
    fn default() -> Self {
        Self {
            max_total_samples: 64_000_000,
            max_fft_size: 1_048_576,
            max_windows: 1_000_000,
            max_output_values: 16_000_000,
            max_fft_operations: 500_000_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeatureConfig {
    pub welch: WelchConfig,
    pub bands: Vec<BandDefinition>,
    /// 所有频段必须位于此范围中，因此 relative 具有一致、明确的分母。
    pub relative_power_range: FrequencyRange,
    /// 显式排除列表由应用策略提供，可来自 P5 的 bad_channels。
    /// 本 crate 不读取 SignalQuality，也不会自主删除低质量通道。
    pub excluded_channels: Vec<String>,
    #[serde(default)]
    pub limits: FeatureLimits,
}

impl Default for FeatureConfig {
    fn default() -> Self {
        Self {
            welch: WelchConfig::default(),
            bands: [
                (FrequencyBand::Delta, 0.5, 4.0),
                (FrequencyBand::Theta, 4.0, 8.0),
                (FrequencyBand::Alpha, 8.0, 13.0),
                (FrequencyBand::Beta, 13.0, 30.0),
            ]
            .into_iter()
            .map(|(band, low_hz, high_hz)| BandDefinition {
                band,
                range: FrequencyRange { low_hz, high_hz },
            })
            .collect(),
            relative_power_range: FrequencyRange {
                low_hz: 0.5,
                high_hz: 30.0,
            },
            excluded_channels: Vec::new(),
            limits: FeatureLimits::default(),
        }
    }
}

/// 时间与应用版本由调用者注入，固定输入/配置/context 可完整回放。
#[derive(Debug, Clone)]
pub struct FeatureContext {
    pub generated_at: DateTime<Utc>,
    pub software_version: String,
}

impl FeatureContext {
    #[must_use]
    pub fn new(generated_at: DateTime<Utc>) -> Self {
        Self {
            generated_at,
            software_version: env!("CARGO_PKG_VERSION").into(),
        }
    }
}
