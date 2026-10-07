//! Boundary validation, measurement orchestration and provenance only.
use crate::{
    QualityAssessment, QualityConfig, QualityContext, QualityError, QualityResult,
    error::configuration, scoring, spectral, time_domain,
};
use domain::{Channel, ChannelKind, EegRecording, Provenance, SignalQuality, UnitInterval};
use std::collections::{BTreeMap, BTreeSet};

/// Object-safe application port. Controllers can inject alternative quality
/// algorithms without knowing detector internals or depending on P4/P6 crates.
pub trait SignalQualityEvaluator: Send + Sync {
    fn assess(
        &self,
        input: &EegRecording,
        context: &QualityContext,
    ) -> QualityResult<SignalQuality>;
}

/// Immutable reusable recipe; all scratch buffers belong to individual calls.
#[derive(Debug, Clone)]
pub struct QualityAnalyzer {
    config: QualityConfig,
}

struct Prepared {
    window: usize,
    fft_size: usize,
    flatline_minimum: usize,
    scales: Vec<Option<f64>>,
}

impl QualityAnalyzer {
    pub fn new(config: QualityConfig) -> QualityResult<Self> {
        config.validate()?;
        Ok(Self { config })
    }

    #[must_use]
    pub fn config(&self) -> &QualityConfig {
        &self.config
    }

    /// The stable domain output includes serialized detailed measurements in its
    /// provenance. Consumers needing typed measurements can use `assess_detailed`.
    pub fn assess(
        &self,
        input: &EegRecording,
        context: &QualityContext,
    ) -> QualityResult<SignalQuality> {
        Ok(self.assess_detailed(input, context)?.signal_quality)
    }

    /// Unlike import/preprocessing, quality intentionally accepts NaN/Inf as
    /// missing observations. Public/deserialized domain fields are revalidated
    /// for shape and metadata before indexing; the input is never mutated.
    pub fn assess_detailed(
        &self,
        input: &EegRecording,
        context: &QualityContext,
    ) -> QualityResult<QualityAssessment> {
        let mut provenance = Provenance::new(
            &context.software_version,
            "eeg-quality",
            "p5-qc-v2",
            context.generated_at,
        )?;
        let prepared = self.prepare(input)?;
        let mut channel_quality = BTreeMap::new();
        let mut measurements = BTreeMap::new();
        let mut bad_channels = Vec::new();
        let mut excluded_channels = Vec::new();
        let mut incomplete = false;
        for ((channel, row), scale) in input
            .channels
            .iter()
            .zip(&input.samples)
            .zip(&prepared.scales)
        {
            let Some(scale) = scale else {
                excluded_channels.push(channel.label.clone());
                continue;
            };
            let mut observed = time_domain::measure(
                row,
                *scale,
                prepared.window,
                prepared.flatline_minimum,
                &self.config,
            )?;
            let spectral = spectral::measure(
                row,
                *scale,
                input.sampling_rate_hz,
                prepared.window,
                prepared.fft_size,
                &self.config.spectral,
            );
            observed.spectral_windows = spectral.windows;
            observed.spectral_coverage =
                time_domain::fraction(spectral.windows * prepared.window, row.len())?;
            observed.line_power_fraction = spectral
                .line
                .map(|value| UnitInterval::new("line power fraction", value))
                .transpose()?;
            observed.high_frequency_power_fraction = spectral
                .high
                .map(|value| UnitInterval::new("high-frequency power fraction", value))
                .transpose()?;
            let mut unavailable = spectral.unavailable;
            if observed.finite_sample_count < 2 {
                unavailable.push("INSUFFICIENT_FINITE_SAMPLES");
            }
            let requested = self.config.spectral.line_frequency_hz.is_some()
                || self.config.spectral.high_frequency_start_hz.is_some();
            if requested
                && observed.spectral_coverage.get() < self.config.scoring.minimum_spectral_coverage
            {
                unavailable.push("INSUFFICIENT_SPECTRAL_COVERAGE");
            }
            incomplete |= !unavailable.is_empty();
            let (quality, bad) = scoring::score(&observed, &self.config.scoring, &unavailable)?;
            if bad {
                bad_channels.push(channel.label.clone());
            }
            channel_quality.insert(channel.label.clone(), quality);
            measurements.insert(channel.label.clone(), observed);
        }

        // Keep bad EEG channels in the denominator. Excluding them would make
        // the overall score increase when the worst channels are discovered.
        let overall = channel_quality
            .values()
            .map(|channel| channel.score.get())
            .sum::<f64>()
            / channel_quality.len() as f64;
        let parameters = &mut provenance.parameters;
        parameters.insert("configuration".into(), serde_json::to_string(&self.config)?);
        parameters.insert("measurements".into(), serde_json::to_string(&measurements)?);
        parameters.insert(
            "excluded_channels".into(),
            serde_json::to_string(&excluded_channels)?,
        );
        parameters.insert(
            "input_processing_history".into(),
            serde_json::to_string(&input.processing_history)?,
        );
        parameters.insert(
            "input_recording_state".into(),
            serde_json::to_string(&input.recording_state)?,
        );
        parameters.insert(
            "input_sampling_rate_hz".into(),
            input.sampling_rate_hz.to_string(),
        );
        parameters.insert(
            "input_sample_count".into(),
            input.sample_count().to_string(),
        );
        parameters.insert("window_samples".into(), prepared.window.to_string());
        parameters.insert("fft_size".into(), prepared.fft_size.to_string());
        parameters.insert(
            "flatline_minimum_samples".into(),
            prepared.flatline_minimum.to_string(),
        );
        parameters.insert("channel_scope".into(), "eeg_only".into());
        parameters.insert("amplitude_measurement_unit".into(), "uV".into());
        parameters.insert(
            "input_channels".into(),
            serde_json::to_string(&input.channels)?,
        );
        parameters.insert(
            "spectral_method".into(),
            "nonoverlap-periodic-hann-detrend-constant-rustfft-v1".into(),
        );
        parameters.insert(
            "scoring_method".into(),
            "weakest-dimension-linear-v1".into(),
        );
        parameters.insert(
            "eeg_quality_version".into(),
            env!("CARGO_PKG_VERSION").into(),
        );
        let bad_count = bad_channels.len();
        let mut signal_quality = SignalQuality::new(
            input.id,
            UnitInterval::new("overall quality", overall.clamp(0.0, 1.0))?,
            bad_channels,
            channel_quality,
            provenance,
        )?;
        if bad_count > 0 {
            signal_quality.warnings.push(scoring::warning(
                "BAD_CHANNELS",
                format!("{bad_count} EEG channels failed the quality policy"),
            )?);
        }
        if !excluded_channels.is_empty() {
            signal_quality.warnings.push(scoring::warning(
                "NON_EEG_CHANNELS_EXCLUDED",
                format!(
                    "{} auxiliary channels excluded from quality scoring",
                    excluded_channels.len()
                ),
            )?);
        }
        if incomplete {
            signal_quality.warnings.push(scoring::warning(
                "INCOMPLETE_QUALITY_EVIDENCE",
                "Some EEG channels have insufficient quality evidence; consult channel warnings",
            )?);
        }
        Ok(QualityAssessment {
            signal_quality,
            measurements,
            excluded_channels,
        })
    }

    fn prepare(&self, input: &EegRecording) -> QualityResult<Prepared> {
        let rate = input.sampling_rate_hz;
        if !rate.is_finite() || rate <= 0.0 {
            return Err(QualityError::InvalidRecording(
                "sampling rate must be finite and positive".into(),
            ));
        }
        if input.channels.is_empty() {
            return Err(domain::DomainError::NoChannels.into());
        }
        if input.channels.len() != input.samples.len() {
            return Err(domain::DomainError::ChannelSampleCountMismatch {
                channels: input.channels.len(),
                sample_rows: input.samples.len(),
            }
            .into());
        }
        let count = input.sample_count();
        if count == 0 {
            return Err(QualityError::InvalidRecording(
                "recording has no samples".into(),
            ));
        }
        let total = count
            .checked_mul(input.channels.len())
            .ok_or(QualityError::LimitExceeded("total sample count overflow"))?;
        if total > self.config.limits.max_total_samples {
            return Err(QualityError::LimitExceeded("total sample count"));
        }
        let duration = count as f64 / rate;
        if !duration.is_finite()
            || !input.duration_seconds.is_finite()
            || input.duration_seconds <= 0.0
            // An absolute epsilon would accept zero/stale durations for a very
            // short recording at a high rate. Use relative tolerance instead.
            || (input.duration_seconds - duration).abs() > duration * 1e-12
        {
            return Err(QualityError::InvalidRecording(
                "duration does not match sample count and sampling rate".into(),
            ));
        }
        let mut labels = BTreeSet::new();
        let mut scales = Vec::with_capacity(input.channels.len());
        let mut eeg_count = 0;
        for (channel, row) in input.channels.iter().zip(&input.samples) {
            Channel::new(&channel.label, channel.kind.clone(), &channel.unit)?;
            if !labels.insert(&channel.label) {
                return Err(domain::DomainError::DuplicateChannel {
                    label: channel.label.clone(),
                }
                .into());
            }
            if row.len() != count {
                return Err(domain::DomainError::UnequalSampleCount {
                    channel: channel.label.clone(),
                    expected: count,
                    actual: row.len(),
                }
                .into());
            }
            if channel.kind == ChannelKind::Eeg {
                eeg_count += 1;
                scales.push(Some(match channel.unit.trim() {
                    "uV" | "µV" | "μV" => 1.0,
                    "mV" => 1_000.0,
                    "V" => 1_000_000.0,
                    _ => {
                        return Err(QualityError::UnsupportedUnit {
                            channel: channel.label.clone(),
                            unit: channel.unit.clone(),
                        });
                    }
                }));
            } else {
                scales.push(None);
            }
        }
        if eeg_count == 0 {
            return Err(QualityError::NoEegChannels);
        }
        let window_f64 = (rate * self.config.window_seconds).round();
        if !window_f64.is_finite() || window_f64 >= usize::MAX as f64 {
            return Err(QualityError::LimitExceeded("epoch length overflow"));
        }
        if window_f64 < 8.0 {
            return Err(configuration(
                "epoch length must be at least eight samples at the input rate",
            ));
        }
        let window = window_f64 as usize;
        let fft_size = window
            .checked_next_power_of_two()
            .ok_or(QualityError::LimitExceeded("FFT length overflow"))?;
        if fft_size > self.config.limits.max_fft_size {
            return Err(QualityError::LimitExceeded("FFT length"));
        }
        let windows = count
            .div_ceil(window)
            .checked_mul(eeg_count)
            .ok_or(QualityError::LimitExceeded("epoch count overflow"))?;
        if windows > self.config.limits.max_windows {
            return Err(QualityError::LimitExceeded("epoch count"));
        }
        // Saturate an unobservable long flatline duration at N+1: such a run
        // cannot exist in this recording, and no enormous allocation is needed.
        let flatline_minimum = (rate * self.config.flatline.minimum_duration_seconds)
            .ceil()
            .max(2.0)
            .min(count.saturating_add(1) as f64) as usize;
        Ok(Prepared {
            window,
            fft_size,
            flatline_minimum,
            scales,
        })
    }
}

impl SignalQualityEvaluator for QualityAnalyzer {
    fn assess(
        &self,
        input: &EegRecording,
        context: &QualityContext,
    ) -> QualityResult<SignalQuality> {
        QualityAnalyzer::assess(self, input, context)
    }
}
