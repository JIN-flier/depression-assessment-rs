//! Validation, ordered execution and audit history. DSP math lives in other modules.
use crate::{
    FilterPhase, PipelineConfig, ProcessingContext, SignalError, SignalResult, SignalStep, dc,
    error::configuration,
    filter::{self, Biquad, REFLECTION_SAMPLES},
    resample::ResamplePlan,
};
use domain::{Channel, ChannelKind, EegRecording, ProcessingStep, RecordingState};

/// Object-safe application boundary; alternative signal implementations can be
/// injected without making a controller depend on DSP primitives or file readers.
pub trait EegProcessor: Send + Sync {
    fn process(
        &self,
        input: &EegRecording,
        context: &ProcessingContext,
    ) -> SignalResult<EegRecording>;
}

/// Immutable, reusable recipe. Each call owns fresh channel/filter state and returns
/// a new recording. Failure never mutates the caller's raw data or partial history.
#[derive(Debug, Clone)]
pub struct SignalPipeline {
    config: PipelineConfig,
}

enum PreparedStep {
    Dc,
    Filter(Vec<Biquad>, FilterPhase),
    Resample(ResamplePlan, f64),
}

impl SignalPipeline {
    /// Validates rate-independent parameters, including configurations decoded from JSON.
    pub fn new(config: PipelineConfig) -> SignalResult<Self> {
        let limits = config.limits;
        if limits.max_total_samples == 0
            || limits.max_kernel_radius == 0
            || limits.max_kernel_evaluations == 0
        {
            return Err(configuration("processing limits must be positive"));
        }
        for step in &config.steps {
            validate_step(step)?;
        }
        Ok(Self { config })
    }

    #[must_use]
    pub fn config(&self) -> &PipelineConfig {
        &self.config
    }

    pub fn process(
        &self,
        input: &EegRecording,
        context: &ProcessingContext,
    ) -> SignalResult<EegRecording> {
        if context.software_version.trim().is_empty() {
            return Err(configuration("software version must not be empty"));
        }
        let count = input.sample_count();
        let channel_count = input.channels.len();
        self.check_shape(channel_count, count)?;
        positive(input.sampling_rate_hz, "input sampling rate")?;
        if input.samples.len() != channel_count {
            return Err(domain::DomainError::ChannelSampleCountMismatch {
                channels: channel_count,
                sample_rows: input.samples.len(),
            }
            .into());
        }
        // Validate public/deserialized domain fields before any indexing in DSP.
        for channel in &input.channels {
            Channel::new(&channel.label, channel.kind.clone(), &channel.unit)?;
        }
        for (channel, row) in input.channels.iter().zip(&input.samples) {
            if row.len() != count {
                return Err(domain::DomainError::UnequalSampleCount {
                    channel: channel.label.clone(),
                    expected: count,
                    actual: row.len(),
                }
                .into());
            }
        }
        let duration = count as f64 / input.sampling_rate_hz;
        if !duration.is_finite()
            || !input.duration_seconds.is_finite()
            || (duration - input.duration_seconds).abs() > 1e-9 * duration
        {
            return Err(SignalError::InvalidRecording(
                "duration does not match shape/rate".into(),
            ));
        }
        // Preflight the ENTIRE recipe, tracking intermediate rates and lengths.
        // A 50Hz notch after downsampling to 80Hz is rejected before processing starts.
        let prepared = self.prepare(
            input.sampling_rate_hz,
            count,
            channel_count,
            input.eeg_channel_count(),
        )?;
        let mut output = EegRecording::new(
            input.id,
            input.subject_id,
            input.sampling_rate_hz,
            input.channels.clone(),
            input.samples.clone(),
            input.recording_state,
            input.metadata.clone(),
        )?;
        // Preserve an accepted stored duration (within relative floating-point
        // tolerance) until resampling actually changes the time grid. In particular,
        // an empty recipe must preserve the entire domain object exactly.
        output.duration_seconds = input.duration_seconds;
        output.processing_history = input.processing_history.clone();
        for (step, prepared) in self.config.steps.iter().zip(prepared) {
            let from_rate = output.sampling_rate_hz;
            let from_count = output.sample_count();
            match prepared {
                PreparedStep::Dc => {
                    for (channel, row) in output.channels.iter().zip(&mut output.samples) {
                        if channel.kind == ChannelKind::Eeg {
                            dc::remove(row)?;
                        }
                    }
                }
                PreparedStep::Filter(filters, phase) => {
                    for (channel, row) in output.channels.iter().zip(&mut output.samples) {
                        if channel.kind == ChannelKind::Eeg {
                            filter::apply(row, &filters, phase)?;
                        }
                    }
                }
                PreparedStep::Resample(plan, target_rate) => {
                    let mut rows = Vec::with_capacity(output.channels.len());
                    for (channel, row) in output.channels.iter().zip(&output.samples) {
                        rows.push(plan.apply(row, channel.kind == ChannelKind::Trigger)?);
                    }
                    output.samples = rows;
                    output.sampling_rate_hz = target_rate;
                    output.duration_seconds = output.sample_count() as f64 / target_rate;
                }
            }
            output
                .processing_history
                .push(history(step, context, from_rate, from_count, &output)?);
            output.recording_state = RecordingState::Preprocessed;
        }
        Ok(output)
    }

    fn check_shape(&self, channels: usize, count: usize) -> SignalResult<()> {
        if channels == 0 || count == 0 {
            return Err(SignalError::InvalidRecording(
                "channels and samples must be nonempty".into(),
            ));
        }
        if count >= (isize::MAX / 4) as usize
            || channels
                .checked_mul(count)
                .is_none_or(|n| n > self.config.limits.max_total_samples)
        {
            return Err(SignalError::LimitExceeded("total samples"));
        }
        Ok(())
    }

    fn prepare(
        &self,
        mut rate: f64,
        mut count: usize,
        channels: usize,
        eeg_channels: usize,
    ) -> SignalResult<Vec<PreparedStep>> {
        let mut prepared = Vec::with_capacity(self.config.steps.len());
        for step in &self.config.steps {
            if !matches!(step, SignalStep::Resample { .. }) && eeg_channels == 0 {
                return Err(configuration(
                    "DC removal and filtering require at least one EEG channel",
                ));
            }
            let operation = match *step {
                SignalStep::DcRemoval {} => PreparedStep::Dc,
                SignalStep::Bandpass {
                    low_hz,
                    high_hz,
                    phase,
                } => {
                    nyquist(high_hz, rate)?;
                    filter_length(phase, count)?;
                    PreparedStep::Filter(Biquad::bandpass(rate, low_hz, high_hz)?, phase)
                }
                SignalStep::Notch {
                    frequency_hz,
                    q,
                    phase,
                } => {
                    nyquist(frequency_hz, rate)?;
                    filter_length(phase, count)?;
                    PreparedStep::Filter(Biquad::notch(rate, frequency_hz, q)?, phase)
                }
                SignalStep::Resample {
                    target_rate_hz,
                    half_width,
                    rolloff,
                } => {
                    let plan = ResamplePlan::new(
                        count,
                        channels,
                        rate,
                        target_rate_hz,
                        half_width,
                        rolloff,
                        self.config.limits,
                    )?;
                    count = plan.output_count;
                    self.check_shape(channels, count)?;
                    rate = target_rate_hz;
                    if !(count as f64 / rate).is_finite() {
                        return Err(configuration("resampling produces non-finite duration"));
                    }
                    PreparedStep::Resample(plan, target_rate_hz)
                }
            };
            prepared.push(operation);
        }
        Ok(prepared)
    }
}

impl EegProcessor for SignalPipeline {
    fn process(
        &self,
        input: &EegRecording,
        context: &ProcessingContext,
    ) -> SignalResult<EegRecording> {
        SignalPipeline::process(self, input, context)
    }
}

fn positive(value: f64, name: &str) -> SignalResult<()> {
    if !value.is_finite() || value <= 0.0 {
        return Err(configuration(format!("{name} must be finite and positive")));
    }
    Ok(())
}
fn nyquist(frequency: f64, rate: f64) -> SignalResult<()> {
    if frequency >= rate / 2.0 {
        return Err(configuration(format!(
            "frequency {frequency} must be below Nyquist {}",
            rate / 2.0
        )));
    }
    Ok(())
}
fn filter_length(phase: FilterPhase, count: usize) -> SignalResult<()> {
    if phase == FilterPhase::ZeroPhase && count <= REFLECTION_SAMPLES {
        return Err(SignalError::TooShort {
            operation: "zero_phase_filter",
            minimum: REFLECTION_SAMPLES + 1,
            actual: count,
        });
    }
    Ok(())
}
fn validate_step(step: &SignalStep) -> SignalResult<()> {
    match *step {
        SignalStep::DcRemoval {} => {}
        SignalStep::Bandpass {
            low_hz, high_hz, ..
        } => {
            positive(low_hz, "low cutoff")?;
            positive(high_hz, "high cutoff")?;
            if low_hz >= high_hz {
                return Err(configuration("low cutoff must be below high cutoff"));
            }
        }
        SignalStep::Notch {
            frequency_hz, q, ..
        } => {
            positive(frequency_hz, "notch frequency")?;
            if !q.is_finite() || !(0.1..=10_000.0).contains(&q) {
                return Err(configuration("notch Q must be in [0.1, 10000]"));
            }
        }
        SignalStep::Resample {
            target_rate_hz,
            half_width,
            rolloff,
        } => {
            positive(target_rate_hz, "target sampling rate")?;
            if !(8..=128).contains(&half_width) {
                return Err(configuration("sinc half_width must be in [8, 128]"));
            }
            if !rolloff.is_finite() || !(0.5..=0.99).contains(&rolloff) {
                return Err(configuration("sinc rolloff must be in [0.5, 0.99]"));
            }
        }
    }
    Ok(())
}

fn history(
    step: &SignalStep,
    context: &ProcessingContext,
    from_rate: f64,
    from_count: usize,
    output: &EegRecording,
) -> SignalResult<ProcessingStep> {
    let (operation, version) = match step {
        SignalStep::DcRemoval {} => ("dc_removal", "mean-f64-v1"),
        SignalStep::Bandpass { .. } => ("bandpass", "butterworth-hp2-lp2-v1"),
        SignalStep::Notch { .. } => ("notch", "rbj-notch-v1"),
        SignalStep::Resample { .. } => ("resample", "hann-windowed-sinc-v1"),
    };
    let mut history = ProcessingStep::new(operation, version, context.applied_at)?;
    let parameters = &mut history.parameters;
    parameters.insert("configuration".into(), serde_json::to_string(step)?);
    parameters.insert("software_version".into(), context.software_version.clone());
    parameters.insert(
        "eeg_signal_version".into(),
        env!("CARGO_PKG_VERSION").into(),
    );
    parameters.insert("from_hz".into(), from_rate.to_string());
    parameters.insert("to_hz".into(), output.sampling_rate_hz.to_string());
    parameters.insert("input_samples".into(), from_count.to_string());
    parameters.insert("output_samples".into(), output.sample_count().to_string());
    match *step {
        SignalStep::DcRemoval {} => {
            parameters.insert("channel_scope".into(), "eeg_only".into());
        }
        SignalStep::Bandpass { phase, .. } | SignalStep::Notch { phase, .. } => {
            parameters.insert("channel_scope".into(), "eeg_only".into());
            parameters.insert(
                "boundary".into(),
                if phase == FilterPhase::ZeroPhase {
                    "steady_state_odd_reflection"
                } else {
                    "steady_state"
                }
                .into(),
            );
            parameters.insert(
                "padding_samples".into(),
                if phase == FilterPhase::ZeroPhase {
                    REFLECTION_SAMPLES
                } else {
                    0
                }
                .to_string(),
            );
        }
        SignalStep::Resample { .. } => {
            parameters.insert(
                "channel_scope".into(),
                "all_analog_sinc_trigger_hold".into(),
            );
            parameters.insert("boundary".into(), "even_reflection".into());
            parameters.insert(
                "sample_count_rule".into(),
                "round_input_count_times_ratio".into(),
            );
            parameters.insert(
                "identity".into(),
                (from_rate == output.sampling_rate_hz).to_string(),
            );
        }
    }
    Ok(history)
}
