//! Policy layer: deterministic scores and warning codes derived solely from
//! measurements. Changing policy never changes the spectral/time-domain math.
use crate::{ChannelMeasurements, QualityResult, ScoringConfig};
use domain::{ChannelQuality, UnitInterval, Warning};

pub(crate) fn warning(code: &str, message: impl Into<String>) -> QualityResult<Warning> {
    Ok(Warning::new(code, message)?)
}

pub(crate) fn score(
    measurements: &ChannelMeasurements,
    config: &ScoringConfig,
    unavailable: &[&str],
) -> QualityResult<(ChannelQuality, bool)> {
    let m = measurements;
    let mut warnings = Vec::new();
    for (code, fraction) in [
        ("MISSING_SAMPLES", m.missing_fraction),
        ("FLATLINE", m.flatline_fraction),
        ("EXTREME_AMPLITUDE", m.extreme_amplitude_fraction),
        ("VARIANCE_ANOMALY", m.variance_anomaly_fraction),
    ] {
        if fraction.get() > 0.0 {
            warnings.push(warning(
                code,
                format!("Affected sample fraction: {:.6}", fraction.get()),
            )?);
        }
    }
    let mut quality_score: f64 = 1.0;
    let mut bad = false;
    for (value, limit, code) in [
        (
            Some(m.missing_fraction),
            config.missing_fraction_limit,
            None,
        ),
        (Some(m.noisy_fraction), config.noisy_fraction_limit, None),
        (
            m.line_power_fraction,
            config.line_power_fraction_limit,
            Some("POWER_LINE_INTERFERENCE"),
        ),
        (
            m.high_frequency_power_fraction,
            config.high_frequency_power_fraction_limit,
            Some("HIGH_FREQUENCY_NOISE"),
        ),
    ] {
        if let Some(value) = value {
            // The weakest dimension wins. At its configured limit a dimension
            // scores 0.5; at twice the limit it scores 0.0. No modality can
            // compensate for missing samples or a severely corrupted channel.
            quality_score = quality_score.min((1.0 - value.get() / (2.0 * limit)).clamp(0.0, 1.0));
            if value.get() >= limit {
                bad = true;
                if let Some(code) = code {
                    warnings.push(warning(
                        code,
                        format!("Power fraction {:.6} reached limit {limit:.6}", value.get()),
                    )?);
                }
            }
        }
    }
    if !unavailable.is_empty() {
        quality_score = quality_score.min(config.unavailable_score_cap);
        for &code in unavailable {
            warnings.push(warning(
                code,
                "Quality evidence is unavailable or insufficient; quality score is capped",
            )?);
        }
    }
    bad |= quality_score < config.bad_channel_score_threshold;
    if bad {
        warnings.push(warning(
            "BAD_CHANNEL",
            "Channel failed the configured quality policy",
        )?);
    }
    Ok((
        ChannelQuality {
            score: UnitInterval::new("channel quality", quality_score)?,
            missing_fraction: m.missing_fraction,
            noisy_fraction: m.noisy_fraction,
            // P1 uses a non-optional field. When evidence is absent this zero is only
            // a storage placeholder: the warning and detailed Option retain absence.
            power_line_interference: m.line_power_fraction.unwrap_or(UnitInterval::ZERO),
            warnings,
        },
        bad,
    ))
}
