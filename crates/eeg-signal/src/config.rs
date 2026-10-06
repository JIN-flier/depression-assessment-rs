//! Serializable algorithm choices separate from execution and domain metadata.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Causal filtering has phase delay. ZeroPhase runs forward and backward with
/// reflection padding, cancelling phase delay while squaring magnitude response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterPhase {
    Causal,
    ZeroPhase,
}

/// Ordered processing instruction. No default pipeline silently changes imported data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SignalStep {
    /// Empty struct variant deliberately rejects unknown JSON fields, unlike a
    /// serde unit variant which can silently accept unused configuration keys.
    DcRemoval {},
    /// One second-order Butterworth highpass and one second-order Butterworth
    /// lowpass. Each edge is -3 dB per pass, not an ideal brick wall.
    Bandpass {
        low_hz: f64,
        high_hz: f64,
        phase: FilterPhase,
    },
    /// RBJ notch with explicit Q (larger Q gives a narrower rejection region).
    Notch {
        frequency_hz: f64,
        q: f64,
        phase: FilterPhase,
    },
    /// Hann-windowed sinc with explicit anti-alias cutoff.
    /// `half_width` is the nominal kernel radius in source samples, widened for
    /// downsampling. `rolloff` also changes the number of sinc zero crossings.
    Resample {
        target_rate_hz: f64,
        half_width: usize,
        rolloff: f64,
    },
}

impl SignalStep {
    #[must_use]
    pub fn bandpass(low_hz: f64, high_hz: f64) -> Self {
        Self::Bandpass {
            low_hz,
            high_hz,
            phase: FilterPhase::ZeroPhase,
        }
    }
    #[must_use]
    pub fn notch(frequency_hz: f64) -> Self {
        Self::Notch {
            frequency_hz,
            q: 30.0,
            phase: FilterPhase::ZeroPhase,
        }
    }
    #[must_use]
    pub fn resample(target_rate_hz: f64) -> Self {
        Self::Resample {
            target_rate_hz,
            half_width: 32,
            rolloff: 0.9,
        }
    }
}

/// Bounds output allocation and sinc work, including extreme downsampling.
/// Every intermediate shape is checked before any DSP executes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessingLimits {
    pub max_total_samples: usize,
    pub max_kernel_radius: usize,
    pub max_kernel_evaluations: usize,
}
impl Default for ProcessingLimits {
    fn default() -> Self {
        Self {
            max_total_samples: 64_000_000,
            max_kernel_radius: 65_536,
            max_kernel_evaluations: 500_000_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PipelineConfig {
    pub steps: Vec<SignalStep>,
    #[serde(default)]
    pub limits: ProcessingLimits,
}

/// Injected rather than reading a wall clock inside DSP. The same recording,
/// configuration and context produce an identical full result on the same runtime.
#[derive(Debug, Clone)]
pub struct ProcessingContext {
    pub applied_at: DateTime<Utc>,
    /// Application version performing computation, not the input importer.
    pub software_version: String,
}
impl ProcessingContext {
    #[must_use]
    pub fn new(applied_at: DateTime<Utc>) -> Self {
        Self {
            applied_at,
            software_version: env!("CARGO_PKG_VERSION").into(),
        }
    }
}
