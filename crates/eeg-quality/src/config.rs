//! Configuration is serializable and validated before execution. No detector
//! guesses mains frequency, units, thresholds or acquisition state from labels.
use crate::{QualityResult, error::configuration};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlatlineConfig {
    /// Consecutive sample differences no larger than this tolerance form a run.
    pub tolerance_uv: f64,
    pub minimum_duration_seconds: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VarianceConfig {
    /// Population variance bounds in square microvolts, assessed per epoch.
    pub minimum_uv2: f64,
    pub maximum_uv2: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpectralConfig {
    /// Denominator is power in [minimum_hz, Nyquist], excluding DC.
    pub minimum_hz: f64,
    /// `None` explicitly disables mains assessment. Defaults to 50 Hz;
    /// callers in 60 Hz environments must change it explicitly.
    pub line_frequency_hz: Option<f64>,
    pub line_half_width_hz: f64,
    /// `None` explicitly disables high-frequency assessment.
    pub high_frequency_start_hz: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScoringConfig {
    /// Positive fractions in (0, 1]. Reaching any limit flags a bad channel.
    pub missing_fraction_limit: f64,
    pub noisy_fraction_limit: f64,
    pub line_power_fraction_limit: f64,
    pub high_frequency_power_fraction_limit: f64,
    /// A score strictly below this threshold also flags a bad channel.
    pub bad_channel_score_threshold: f64,
    /// Prevents absent spectral evidence from being presented as high quality.
    pub unavailable_score_cap: f64,
    /// Minimum fraction of all samples covered by valid spectral epochs.
    pub minimum_spectral_coverage: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualityLimits {
    pub max_total_samples: usize,
    pub max_fft_size: usize,
    /// Total epoch count across all assessed EEG channels, including invalid ones.
    pub max_windows: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualityConfig {
    /// Shared non-overlapping epoch length; the final partial epoch participates
    /// in time-domain analysis but never in spectral analysis.
    pub window_seconds: f64,
    pub flatline: FlatlineConfig,
    /// Absolute amplitude relative to physical zero (DC offsets are observable).
    pub extreme_amplitude_uv: f64,
    pub variance: VarianceConfig,
    pub spectral: SpectralConfig,
    pub scoring: ScoringConfig,
    pub limits: QualityLimits,
}

impl Default for QualityConfig {
    fn default() -> Self {
        Self {
            window_seconds: 2.0,
            flatline: FlatlineConfig {
                tolerance_uv: 0.01,
                minimum_duration_seconds: 0.5,
            },
            extreme_amplitude_uv: 200.0,
            variance: VarianceConfig {
                minimum_uv2: 0.01,
                maximum_uv2: 10_000.0,
            },
            spectral: SpectralConfig {
                minimum_hz: 1.0,
                line_frequency_hz: Some(50.0),
                line_half_width_hz: 1.0,
                high_frequency_start_hz: Some(30.0),
            },
            scoring: ScoringConfig {
                missing_fraction_limit: 0.05,
                noisy_fraction_limit: 0.2,
                line_power_fraction_limit: 0.2,
                high_frequency_power_fraction_limit: 0.4,
                bad_channel_score_threshold: 0.6,
                unavailable_score_cap: 0.5,
                minimum_spectral_coverage: 0.5,
            },
            limits: QualityLimits {
                max_total_samples: 64_000_000,
                max_fft_size: 16_384,
                max_windows: 1_000_000,
            },
        }
    }
}

impl QualityConfig {
    pub(crate) fn validate(&self) -> QualityResult<()> {
        positive(self.window_seconds, "window duration")?;
        positive(self.extreme_amplitude_uv, "extreme amplitude")?;
        positive(self.flatline.minimum_duration_seconds, "flatline duration")?;
        nonnegative(self.flatline.tolerance_uv, "flatline tolerance")?;
        nonnegative(self.variance.minimum_uv2, "minimum variance")?;
        positive(self.variance.maximum_uv2, "maximum variance")?;
        if self.variance.minimum_uv2 >= self.variance.maximum_uv2 {
            return Err(configuration(
                "minimum variance must be below maximum variance",
            ));
        }
        positive(self.spectral.minimum_hz, "spectral lower frequency")?;
        positive(self.spectral.line_half_width_hz, "line half width")?;
        if let Some(line) = self.spectral.line_frequency_hz {
            positive(line, "line frequency")?;
            if line - self.spectral.line_half_width_hz < self.spectral.minimum_hz
                || !(line + self.spectral.line_half_width_hz).is_finite()
            {
                return Err(configuration("line band must lie within the analysis band"));
            }
        }
        if let Some(high) = self.spectral.high_frequency_start_hz {
            positive(high, "high-frequency boundary")?;
            if high <= self.spectral.minimum_hz {
                return Err(configuration(
                    "high-frequency boundary must exceed minimum_hz",
                ));
            }
        }
        let scoring = &self.scoring;
        for (value, name) in [
            (scoring.missing_fraction_limit, "missing limit"),
            (scoring.noisy_fraction_limit, "noise limit"),
            (scoring.line_power_fraction_limit, "line power limit"),
            (
                scoring.high_frequency_power_fraction_limit,
                "high-frequency limit",
            ),
            (scoring.bad_channel_score_threshold, "bad-channel threshold"),
            (
                scoring.minimum_spectral_coverage,
                "minimum spectral coverage",
            ),
        ] {
            positive(value, name)?;
            if value > 1.0 {
                return Err(configuration(format!("{name} must be at most one")));
            }
        }
        nonnegative(scoring.unavailable_score_cap, "unavailable score cap")?;
        if scoring.unavailable_score_cap >= scoring.bad_channel_score_threshold {
            return Err(configuration(
                "unavailable score cap must be below bad-channel threshold",
            ));
        }
        if self.limits.max_total_samples == 0
            || self.limits.max_windows == 0
            || self.limits.max_fft_size < 8
        {
            return Err(configuration(
                "sample/window limits must be positive; FFT limit must be >= 8",
            ));
        }
        Ok(())
    }
}

fn positive(value: f64, name: &str) -> QualityResult<()> {
    if !value.is_finite() || value <= 0.0 {
        return Err(configuration(format!("{name} must be finite and positive")));
    }
    Ok(())
}
fn nonnegative(value: f64, name: &str) -> QualityResult<()> {
    if !value.is_finite() || value < 0.0 {
        return Err(configuration(format!(
            "{name} must be finite and nonnegative"
        )));
    }
    Ok(())
}

/// Injected context keeps the entire result reproducible without a wall-clock read.
#[derive(Debug, Clone)]
pub struct QualityContext {
    pub generated_at: DateTime<Utc>,
    pub software_version: String,
}
impl QualityContext {
    #[must_use]
    pub fn new(generated_at: DateTime<Utc>) -> Self {
        Self {
            generated_at,
            software_version: env!("CARGO_PKG_VERSION").into(),
        }
    }
}
