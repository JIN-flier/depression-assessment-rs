//! 所有输入约束在 FFT 规划/分配前检查。领域字段是公开且可反序列化的，
//! 因此不能假定输入必然经过 EegRecording::new。
use crate::{FeatureConfig, FeatureError, FeatureResult, FrequencyRange, error::configuration};
use domain::{Channel, ChannelKind, DomainError, EegRecording};
use std::collections::BTreeSet;

pub(crate) struct SelectedChannel {
    pub index: usize,
    pub scale_uv: f64,
}

pub(crate) struct Prepared {
    pub selected: Vec<SelectedChannel>,
    pub excluded: Vec<String>,
    pub window: usize,
    pub overlap: usize,
    pub hop: usize,
    pub fft_size: usize,
    pub windows: usize,
    pub trailing: usize,
}

pub(crate) fn validate_config(config: &FeatureConfig) -> FeatureResult<()> {
    let w = &config.welch;
    if !w.window_seconds.is_finite() || w.window_seconds <= 0.0 {
        return Err(configuration("window_seconds must be finite and positive"));
    }
    if !w.overlap_fraction.is_finite() || !(0.0..1.0).contains(&w.overlap_fraction) {
        return Err(configuration("overlap_fraction must be in [0, 1)"));
    }
    if w.fft_size.is_some_and(|n| n < 3) {
        return Err(configuration("FFT size must be at least three"));
    }
    validate_range(config.relative_power_range)?;
    if config.bands.is_empty() {
        return Err(configuration("at least one frequency band is required"));
    }
    let mut names = BTreeSet::new();
    for band in &config.bands {
        validate_range(band.range)?;
        if !names.insert(band.band) {
            return Err(configuration("duplicate frequency band"));
        }
        if band.range.low_hz < config.relative_power_range.low_hz
            || band.range.high_hz > config.relative_power_range.high_hz
        {
            return Err(configuration(
                "bands must be contained in relative_power_range",
            ));
        }
    }
    // 名称顺序不必与频率顺序一致；只约束区间无重叠，边界接触合法。
    let mut ranges: Vec<_> = config.bands.iter().map(|band| band.range).collect();
    ranges.sort_by(|a, b| a.low_hz.total_cmp(&b.low_hz));
    if ranges
        .windows(2)
        .any(|pair| pair[0].high_hz > pair[1].low_hz)
    {
        return Err(configuration("frequency bands must not overlap"));
    }
    let mut exclusions = BTreeSet::new();
    for label in &config.excluded_channels {
        if label.trim().is_empty() || !exclusions.insert(label) {
            return Err(configuration(
                "excluded channels must have unique, nonempty labels",
            ));
        }
    }
    let limits = &config.limits;
    if [
        limits.max_total_samples,
        limits.max_fft_size,
        limits.max_windows,
        limits.max_output_values,
        limits.max_fft_operations,
    ]
    .contains(&0)
    {
        return Err(configuration("resource limits must be positive"));
    }
    Ok(())
}

fn validate_range(range: FrequencyRange) -> FeatureResult<()> {
    if !range.low_hz.is_finite()
        || !range.high_hz.is_finite()
        || range.low_hz < 0.0
        || range.high_hz <= range.low_hz
    {
        return Err(configuration(
            "frequency ranges require finite 0 <= low < high",
        ));
    }
    Ok(())
}

pub(crate) fn prepare(input: &EegRecording, config: &FeatureConfig) -> FeatureResult<Prepared> {
    let rate = input.sampling_rate_hz;
    if !rate.is_finite() || rate <= 0.0 {
        return Err(FeatureError::InvalidRecording(
            "sampling rate must be finite and positive".into(),
        ));
    }
    if input.channels.is_empty() {
        return Err(DomainError::NoChannels.into());
    }
    if input.channels.len() != input.samples.len() {
        return Err(DomainError::ChannelSampleCountMismatch {
            channels: input.channels.len(),
            sample_rows: input.samples.len(),
        }
        .into());
    }
    let count = input.sample_count();
    if count == 0 {
        return Err(FeatureError::InvalidRecording(
            "recording has no samples".into(),
        ));
    }
    bounded_product(
        count,
        input.channels.len(),
        config.limits.max_total_samples,
        "total samples",
    )?;
    let duration = count as f64 / rate;
    if !duration.is_finite()
        || duration <= 0.0
        || !input.duration_seconds.is_finite()
        || input.duration_seconds <= 0.0
        || (input.duration_seconds - duration).abs() > duration * 1e-12
    {
        return Err(FeatureError::InvalidRecording(
            "duration does not match shape and rate".into(),
        ));
    }
    let mut labels = BTreeSet::new();
    let mut selected = Vec::new();
    let mut excluded = Vec::new();
    let exclusions: BTreeSet<_> = config.excluded_channels.iter().collect();
    for (index, (channel, row)) in input.channels.iter().zip(&input.samples).enumerate() {
        Channel::new(&channel.label, channel.kind.clone(), &channel.unit)?;
        if !labels.insert(&channel.label) {
            return Err(DomainError::DuplicateChannel {
                label: channel.label.clone(),
            }
            .into());
        }
        if row.len() != count {
            return Err(DomainError::UnequalSampleCount {
                channel: channel.label.clone(),
                expected: count,
                actual: row.len(),
            }
            .into());
        }
        // 特征提取严格拒绝缺失值，不插值、不拼接断点。P5 可以先分析缺失。
        if let Some(sample_index) = row.iter().position(|value| !value.is_finite()) {
            return Err(DomainError::NonFiniteSample {
                channel: channel.label.clone(),
                sample_index,
            }
            .into());
        }
        if channel.kind != ChannelKind::Eeg || exclusions.contains(&channel.label) {
            excluded.push(channel.label.clone());
            continue;
        }
        let scale_uv = match channel.unit.trim() {
            "uV" | "µV" | "μV" => 1.0,
            "mV" => 1_000.0,
            "V" => 1_000_000.0,
            _ => {
                return Err(FeatureError::UnsupportedUnit {
                    channel: channel.label.clone(),
                    unit: channel.unit.clone(),
                });
            }
        };
        selected.push(SelectedChannel { index, scale_uv });
    }
    for label in exclusions {
        if !labels.contains(label) {
            return Err(FeatureError::UnknownChannel(label.clone()));
        }
    }
    if selected.is_empty() {
        return Err(FeatureError::NoEegChannels);
    }
    let length = (rate * config.welch.window_seconds).round();
    if !length.is_finite() || length >= usize::MAX as f64 {
        return Err(FeatureError::LimitExceeded("window length overflow"));
    }
    // 2 点周期 Hann 为 [0,1]，缺乏实际频谱估计意义；要求至少三个样本。
    if length < 3.0 {
        return Err(configuration("window must contain at least three samples"));
    }
    let window = length as usize;
    let fft_size = config.welch.fft_size.unwrap_or(window);
    if fft_size < window {
        return Err(configuration("FFT size cannot be shorter than window"));
    }
    if fft_size > config.limits.max_fft_size {
        return Err(FeatureError::LimitExceeded("FFT size"));
    }
    if count < window {
        return Err(FeatureError::TooShort {
            minimum: window,
            actual: count,
        });
    }
    let overlap = (window as f64 * config.welch.overlap_fraction).floor() as usize;
    // 防止极大整数转 f64 舍入后使原本 <1 的比例产生 overlap == window。
    if overlap >= window {
        return Err(configuration("overlap leaves no forward progress"));
    }
    let hop = window - overlap;
    let windows = 1 + (count - window) / hop;
    let total_windows = bounded_product(
        windows,
        selected.len(),
        config.limits.max_windows,
        "window count",
    )?;
    bounded_product(
        fft_size / 2 + 1,
        selected.len(),
        config.limits.max_output_values,
        "PSD output values",
    )?;
    let levels = usize::BITS - (fft_size - 1).leading_zeros();
    let per_window = bounded_product(
        fft_size,
        levels as usize,
        config.limits.max_fft_operations,
        "FFT work",
    )?;
    bounded_product(
        per_window,
        total_windows,
        config.limits.max_fft_operations,
        "FFT work",
    )?;
    let range = config.relative_power_range;
    if range.high_hz > rate / 2.0 {
        return Err(FeatureError::UnavailableRange {
            low_hz: range.low_hz,
            high_hz: range.high_hz,
            nyquist_hz: rate / 2.0,
        });
    }
    Ok(Prepared {
        selected,
        excluded,
        window,
        overlap,
        hop,
        fft_size,
        windows,
        trailing: (count - window) % hop,
    })
}

fn bounded_product(a: usize, b: usize, limit: usize, name: &'static str) -> FeatureResult<usize> {
    let product = a.checked_mul(b).ok_or(FeatureError::LimitExceeded(name))?;
    if product > limit {
        return Err(FeatureError::LimitExceeded(name));
    }
    Ok(product)
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{RecordingId, RecordingMetadata, RecordingState, SubjectId};

    fn input() -> EegRecording {
        EegRecording::new(
            RecordingId::new(),
            SubjectId::new(),
            250.0,
            vec![Channel::new("Cz", ChannelKind::Eeg, "uV").unwrap()],
            vec![vec![0.0; 1283]],
            RecordingState::Raw,
            RecordingMetadata::default(),
        )
        .unwrap()
    }

    #[test]
    fn epoch_shape_has_expected_rounding_overlap_and_uncovered_tail() {
        let mut config = FeatureConfig::default();
        let prepared = prepare(&input(), &config).unwrap();
        assert_eq!(
            (
                prepared.window,
                prepared.overlap,
                prepared.hop,
                prepared.fft_size,
                prepared.windows,
                prepared.trailing
            ),
            (500, 250, 250, 500, 4, 33)
        );
        config.welch.window_seconds = 1.01; // round(252.5) = 253
        let prepared = prepare(&input(), &config).unwrap();
        assert_eq!(
            (prepared.window, prepared.overlap, prepared.hop),
            (253, 126, 127)
        );
    }

    #[test]
    fn selected_units_and_excluded_auxiliary_channels_keep_source_indices() {
        let mut input = input();
        input.channels[0].unit = "mV".into();
        input.channels.insert(
            0,
            Channel::new("Status", ChannelKind::Trigger, "code").unwrap(),
        );
        input.samples.insert(0, vec![0.0; 1283]);
        let prepared = prepare(&input, &FeatureConfig::default()).unwrap();
        assert_eq!(prepared.selected[0].index, 1);
        assert_eq!(prepared.selected[0].scale_uv, 1000.0);
        assert_eq!(prepared.excluded, ["Status"]);
        let config = FeatureConfig {
            excluded_channels: vec!["Cz".into()],
            ..Default::default()
        };
        assert!(matches!(
            prepare(&input, &config),
            Err(FeatureError::NoEegChannels)
        ));
    }

    #[test]
    fn invalid_shapes_finite_values_and_stale_duration_are_rejected() {
        let mut input = input();
        input.samples[0][3] = f32::NAN;
        assert!(matches!(
            prepare(&input, &FeatureConfig::default()),
            Err(FeatureError::Domain(DomainError::NonFiniteSample {
                sample_index: 3,
                ..
            }))
        ));
        input.samples[0][3] = 0.0;
        input.duration_seconds = 1.0;
        assert!(matches!(
            prepare(&input, &FeatureConfig::default()),
            Err(FeatureError::InvalidRecording(_))
        ));
        input.duration_seconds = 1283.0 / 250.0;
        input.samples.clear();
        assert!(matches!(
            prepare(&input, &FeatureConfig::default()),
            Err(FeatureError::Domain(
                DomainError::ChannelSampleCountMismatch { .. }
            ))
        ));
    }

    #[test]
    fn invalid_ranges_and_overlap_fail_before_processing() {
        let mut config = FeatureConfig::default();
        config.welch.overlap_fraction = 1.0;
        assert!(matches!(
            validate_config(&config),
            Err(FeatureError::Configuration(_))
        ));
        config = FeatureConfig::default();
        config.bands[0].range.high_hz = 5.0;
        assert!(matches!(
            validate_config(&config),
            Err(FeatureError::Configuration(_))
        ));
        config = FeatureConfig::default();
        config.bands[0].range.high_hz = f64::NAN;
        assert!(matches!(
            validate_config(&config),
            Err(FeatureError::Configuration(_))
        ));
        config = FeatureConfig::default();
        config.relative_power_range.low_hz = 1.0;
        assert!(matches!(
            validate_config(&config),
            Err(FeatureError::Configuration(_))
        ));
    }

    #[test]
    fn budgets_bound_windows_outputs_and_fft_work() {
        let input = input();
        for (field, limit) in [
            ("samples", 100),
            ("fft", 256),
            ("windows", 3),
            ("outputs", 250),
            ("work", 100),
        ] {
            let mut config = FeatureConfig::default();
            match field {
                "samples" => config.limits.max_total_samples = limit,
                "fft" => config.limits.max_fft_size = limit,
                "windows" => config.limits.max_windows = limit,
                "outputs" => config.limits.max_output_values = limit,
                _ => config.limits.max_fft_operations = limit,
            }
            assert!(
                matches!(
                    prepare(&input, &config),
                    Err(FeatureError::LimitExceeded(_))
                ),
                "{field}"
            );
        }
        assert!(matches!(
            bounded_product(usize::MAX, 2, usize::MAX, "overflow"),
            Err(FeatureError::LimitExceeded(_))
        ));
    }

    #[test]
    fn sampling_limits_and_short_windows_fail_explicitly() {
        let mut input = input();
        input.sampling_rate_hz = 32.0;
        input.duration_seconds = 1283.0 / 32.0;
        assert!(matches!(
            prepare(&input, &FeatureConfig::default()),
            Err(FeatureError::UnavailableRange { .. })
        ));
        let mut config = FeatureConfig::default();
        config.welch.window_seconds = f64::MAX;
        assert!(matches!(
            prepare(&input, &config),
            Err(FeatureError::LimitExceeded(_))
        ));
        config.welch.window_seconds = 0.001;
        assert!(matches!(
            prepare(&input, &config),
            Err(FeatureError::Configuration(_))
        ));
        input.samples[0].truncate(16);
        input.duration_seconds = 16.0 / 32.0;
        assert!(matches!(
            prepare(&input, &FeatureConfig::default()),
            Err(FeatureError::TooShort {
                minimum: 64,
                actual: 16
            })
        ));
    }
}
