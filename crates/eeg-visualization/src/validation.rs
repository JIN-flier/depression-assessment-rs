//! 领域字段公开且可反序列化，因此不能只信任构造时的校验。
use crate::{
    VisualizationError, VisualizationLimits, VisualizationResult,
    error::{configuration, invalid},
};
use domain::{ChannelKind, EegFeatures, EegRecording, SpectralFeatures};
use std::collections::BTreeSet;

pub(crate) fn budget(count: usize, max: usize, field: &'static str) -> VisualizationResult<()> {
    if count > max {
        return Err(VisualizationError::LimitExceeded(field));
    }
    Ok(())
}

pub(crate) fn product(
    a: usize,
    b: usize,
    max: usize,
    field: &'static str,
) -> VisualizationResult<usize> {
    let count = a
        .checked_mul(b)
        .ok_or(VisualizationError::LimitExceeded(field))?;
    budget(count, max, field)?;
    Ok(count)
}

pub(crate) fn limits(limits: VisualizationLimits) -> VisualizationResult<()> {
    if [
        limits.max_channels,
        limits.max_input_values,
        limits.max_output_points,
        limits.max_grid_cells,
        limits.max_interpolation_operations,
    ]
    .contains(&0)
    {
        return Err(configuration("all resource limits must be positive"));
    }
    Ok(())
}

pub(crate) fn name(value: &str) -> VisualizationResult<()> {
    if value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(invalid(
            "labels must contain 1..=256 bytes without control characters",
        ));
    }
    Ok(())
}

pub(crate) fn ordered_range(low: f64, high: f64) -> VisualizationResult<()> {
    if !low.is_finite() || !high.is_finite() || low < 0.0 || low >= high {
        return Err(configuration(
            "range must be finite, non-negative and increasing",
        ));
    }
    Ok(())
}

/// 保留显式选择顺序，拒绝重复或未知通道；空选择使用上游明确提供的顺序。
pub(crate) fn select(
    available: Vec<String>,
    requested: &[String],
) -> VisualizationResult<Vec<String>> {
    let mut known = BTreeSet::new();
    for label in &available {
        name(label)?;
        if !known.insert(label.as_str()) {
            return Err(invalid("duplicate channel labels"));
        }
    }
    let chosen = if requested.is_empty() {
        &available
    } else {
        requested
    };
    let mut unique = BTreeSet::new();
    for label in chosen {
        name(label)?;
        if !known.contains(label.as_str()) {
            return Err(VisualizationError::UnknownChannel(label.clone()));
        }
        if !unique.insert(label) {
            return Err(configuration("duplicate requested channel"));
        }
    }
    if chosen.is_empty() {
        return Err(invalid("no channels to visualize"));
    }
    Ok(chosen.to_vec())
}

/// 不复制大型矩阵。先检查形状与预算，再扫描有限值，避免无上限工作。
pub(crate) fn recording(
    input: &EegRecording,
    limits: VisualizationLimits,
) -> VisualizationResult<()> {
    if !input.sampling_rate_hz.is_finite() || input.sampling_rate_hz <= 0.0 {
        return Err(invalid("invalid sampling rate"));
    }
    if input.channels.is_empty() || input.channels.len() != input.samples.len() {
        return Err(invalid("channel/sample shape mismatch"));
    }
    let count = input.sample_count();
    budget(
        input.channels.len(),
        limits.max_channels,
        "recording channels",
    )?;
    if count == 0 {
        return Err(VisualizationError::EmptyRange);
    }
    product(
        input.channels.len(),
        count,
        limits.max_input_values,
        "recording samples",
    )?;
    let duration = count as f64 / input.sampling_rate_hz;
    if !duration.is_finite()
        || !input.duration_seconds.is_finite()
        || (duration - input.duration_seconds).abs() > 1e-9 * duration.max(1.0)
    {
        return Err(invalid("recording duration does not match samples/rate"));
    }
    let mut labels = BTreeSet::new();
    for (channel, row) in input.channels.iter().zip(&input.samples) {
        name(&channel.label)?;
        if !labels.insert(&channel.label)
            || row.len() != count
            || row.iter().any(|v| !v.is_finite())
        {
            return Err(invalid(
                "duplicate labels, ragged matrix or non-finite samples",
            ));
        }
    }
    Ok(())
}

pub(crate) fn eeg_labels(input: &EegRecording) -> Vec<String> {
    input
        .channels
        .iter()
        .filter(|c| c.kind == ChannelKind::Eeg)
        .map(|c| c.label.clone())
        .collect()
}

pub(crate) fn microvolt_scale(unit: &str, channel: &str) -> VisualizationResult<f64> {
    match unit.trim() {
        "V" => Ok(1e6),
        "mV" => Ok(1e3),
        "uV" | "µV" | "μV" => Ok(1.0),
        _ => Err(VisualizationError::UnsupportedUnit {
            channel: channel.into(),
            unit: unit.into(),
        }),
    }
}

pub(crate) fn spectral(features: &EegFeatures) -> VisualizationResult<&SpectralFeatures> {
    // P6 定义标准频谱单位。已有审计信息若明确声明其他单位，拒绝错误标注。
    for (key, expected) in [("psd_unit", "uV^2/Hz"), ("band_power_unit", "uV^2")] {
        if features
            .provenance
            .parameters
            .get(key)
            .is_some_and(|unit| unit != expected)
        {
            return Err(invalid(format!("unsupported spectral unit: {key}")));
        }
    }
    features
        .spectral
        .as_ref()
        .ok_or(VisualizationError::MissingSpectral)
}
