//! P5: deterministic EEG quality assessment, independent of preprocessing,
//! feature extraction, persistence, UI and models.
//!
//! The application injects timestamps and schedules this CPU work on a blocking
//! worker. No samples are changed, imputed, filtered or removed. Thresholds are
//! engineering heuristics, not clinical cutoffs. See the README for equations,
//! missing-data behavior and the meaning of unavailable spectral measurements.
//!
//! ```
//! use chrono::Utc;
//! use eeg_quality::{QualityAnalyzer, QualityConfig, QualityContext, SignalQualityEvaluator};
//! # fn main() -> Result<(), eeg_quality::QualityError> {
//! let analyzer: Box<dyn SignalQualityEvaluator> =
//!     Box::new(QualityAnalyzer::new(QualityConfig::default())?);
//! let context = QualityContext::new(Utc::now());
//! // let quality = analyzer.assess(&recording, &context)?;
//! # let _ = (analyzer, context);
//! # Ok(()) }
//! ```

#![forbid(unsafe_code)]

mod analyzer;
mod config;
mod error;
mod scoring;
mod spectral;
mod time_domain;

pub use analyzer::{QualityAnalyzer, SignalQualityEvaluator};
pub use config::{
    FlatlineConfig, QualityConfig, QualityContext, QualityLimits, ScoringConfig, SpectralConfig,
    VarianceConfig,
};
pub use error::{QualityError, QualityResult};

use domain::{SignalQuality, UnitInterval};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Observed quantities, separate from configurable quality scoring policy.
/// All time-domain fractions use the entire channel length as denominator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelMeasurements {
    pub sample_count: usize,
    pub finite_sample_count: usize,
    pub variance_uv2: Option<f64>,
    pub missing_fraction: UnitInterval,
    pub flatline_fraction: UnitInterval,
    pub extreme_amplitude_fraction: UnitInterval,
    pub variance_anomaly_fraction: UnitInterval,
    /// Union of flatline, extreme-amplitude and variance-anomaly sample masks.
    /// Missing samples are counted separately and never double-counted here.
    pub noisy_fraction: UnitInterval,
    /// Number of complete, entirely finite, non-overlapping spectral epochs.
    pub spectral_windows: usize,
    pub spectral_coverage: UnitInterval,
    /// `None` means disabled or unmeasurable, not zero interference/noise.
    pub line_power_fraction: Option<UnitInterval>,
    pub high_frequency_power_fraction: Option<UnitInterval>,
}

/// Detailed research/audit result; normal consumers can use just `SignalQuality`.
/// The domain schema remains independent of this particular quality algorithm.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QualityAssessment {
    pub signal_quality: SignalQuality,
    pub measurements: BTreeMap<String, ChannelMeasurements>,
    pub excluded_channels: Vec<String>,
}
