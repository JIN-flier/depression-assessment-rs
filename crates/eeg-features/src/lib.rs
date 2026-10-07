//! P6：独立、可复现的 EEG 频谱特征提取。
//!
//! 运行时只依赖领域数据与数值/序列化库。输入不会被修改，也不会隐式执行
//! 滤波、重采样或质量筛选。应用层负责把此同步 CPU 任务放入后台线程。
//! 默认采用 2 秒周期 Hann 窗、50% 重叠、逐窗去均值的单边 Welch PSD。
//! PSD 单位为 uV²/Hz，频段绝对功率为 uV²；具体积分约定见 README。
//!
//! ```
//! use chrono::Utc;
//! use eeg_features::{EegFeatureExtractor, FeatureConfig, FeatureContext, SpectralExtractor};
//! # fn main() -> Result<(), eeg_features::FeatureError> {
//! let extractor: Box<dyn EegFeatureExtractor> =
//!     Box::new(SpectralExtractor::new(FeatureConfig::default())?);
//! let context = FeatureContext::new(Utc::now());
//! // let features = extractor.extract(&recording, &context)?;
//! # let _ = (extractor, context);
//! # Ok(()) }
//! ```

#![forbid(unsafe_code)]

mod band_power;
mod config;
mod error;
mod extractor;
mod validation;
mod welch;

pub use config::{
    BandDefinition, Detrend, FeatureConfig, FeatureContext, FeatureLimits, FrequencyRange,
    WelchConfig,
};
pub use error::{FeatureError, FeatureResult};
pub use extractor::{EegFeatureExtractor, SpectralExtractor};

use domain::EegFeatures;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 实际执行参数；区别于配置中的秒数/比例，避免采样点取整造成复现歧义。
/// 所有选中通道采用相同形状，因此窗数与尾段样本数不必逐通道重复保存。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpectralAudit {
    pub window_samples: usize,
    pub overlap_samples: usize,
    pub hop_samples: usize,
    pub fft_size: usize,
    pub window_count: usize,
    /// 最后一个完整窗之后未参与分析的尾段，不会补零伪装成有效 EEG。
    pub trailing_samples: usize,
    pub frequency_resolution_hz: f64,
    /// 原始通道顺序；领域结果使用 BTreeMap，图形层可据此恢复采集顺序。
    pub included_channels: Vec<String>,
    pub excluded_channels: Vec<String>,
    /// 相对功率分母：配置的 reference 范围内积分，不是已输出频段之和。
    pub reference_power_uv2_by_channel: BTreeMap<String, f64>,
    /// 分母恰为零时相对功率约定为零，并记录此处以区分“比例为零”和无能量。
    pub zero_power_channels: Vec<String>,
}

/// 便于科研/应用层读取的完整结果；audit 同时写入领域 provenance，
/// 因此仅持久化 `features` 也能保留全部参数与数据选择信息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeatureExtraction {
    pub features: EegFeatures,
    pub audit: SpectralAudit,
}
