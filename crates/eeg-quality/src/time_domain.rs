//! Time-domain measurements only: no thresholds for bad-channel scoring here.
//! Each detector marks samples in a shared union mask so overlapping artifacts
//! cannot produce a noisy fraction greater than one.
use crate::{ChannelMeasurements, QualityConfig, QualityResult};
use domain::UnitInterval;

pub(crate) fn fraction(count: usize, total: usize) -> QualityResult<UnitInterval> {
    Ok(UnitInterval::new(
        "quality fraction",
        count as f64 / total as f64,
    )?)
}

/// Welford's population variance avoids cancellation for large DC offsets.
/// f32 samples (even converted from V to uV) stay well within f64 arithmetic.
#[derive(Default)]
pub(crate) struct Moments {
    count: usize,
    mean: f64,
    m2: f64,
}
impl Moments {
    pub(crate) fn add(&mut self, value: f64) {
        self.count += 1;
        let delta = value - self.mean;
        self.mean += delta / self.count as f64;
        self.m2 += delta * (value - self.mean);
    }
    fn variance(&self) -> Option<f64> {
        (self.count >= 2).then(|| (self.m2 / self.count as f64).max(0.0))
    }
    pub(crate) fn mean(&self) -> f64 {
        self.mean
    }
}

pub(crate) fn measure(
    row: &[f32],
    scale_uv: f64,
    window: usize,
    flatline_minimum: usize,
    config: &QualityConfig,
) -> QualityResult<ChannelMeasurements> {
    let mut noisy = vec![false; row.len()];
    let mut moments = Moments::default();
    let mut extreme_count = 0;
    for (index, &value) in row.iter().enumerate() {
        if value.is_finite() {
            let value = f64::from(value) * scale_uv;
            moments.add(value);
            if value.abs() >= config.extreme_amplitude_uv {
                noisy[index] = true;
                extreme_count += 1;
            }
        }
    }

    // A NaN/Inf breaks a flat run, as does a jump larger than tolerance. Runs
    // cross epoch boundaries; a plateau cannot disappear due to segmentation.
    let mut flat_count = 0;
    let mut start = 0;
    for end in 1..=row.len() {
        let continues = end < row.len()
            && row[end - 1].is_finite()
            && row[end].is_finite()
            && (f64::from(row[end]) - f64::from(row[end - 1])).abs() * scale_uv
                <= config.flatline.tolerance_uv;
        if !continues {
            if row[start].is_finite() && end - start >= flatline_minimum {
                noisy[start..end].fill(true);
                flat_count += end - start;
            }
            start = end;
        }
    }

    let mut variance_count = 0;
    for (epoch_index, epoch) in row.chunks(window).enumerate() {
        let mut epoch_moments = Moments::default();
        for &value in epoch.iter().filter(|value| value.is_finite()) {
            epoch_moments.add(f64::from(value) * scale_uv);
        }
        // Partial epochs with >=2 finite points still have time-domain evidence.
        // Missing values never become zeros or synthetic interpolated points.
        if let Some(variance) = epoch_moments.variance()
            && (variance < config.variance.minimum_uv2 || variance > config.variance.maximum_uv2)
        {
            let offset = epoch_index * window;
            for (index, value) in epoch.iter().enumerate() {
                if value.is_finite() {
                    noisy[offset + index] = true;
                    variance_count += 1;
                }
            }
        }
    }
    Ok(ChannelMeasurements {
        sample_count: row.len(),
        finite_sample_count: moments.count,
        variance_uv2: moments.variance(),
        missing_fraction: fraction(row.len() - moments.count, row.len())?,
        flatline_fraction: fraction(flat_count, row.len())?,
        extreme_amplitude_fraction: fraction(extreme_count, row.len())?,
        variance_anomaly_fraction: fraction(variance_count, row.len())?,
        noisy_fraction: fraction(noisy.iter().filter(|&&value| value).count(), row.len())?,
        spectral_windows: 0,
        spectral_coverage: UnitInterval::ZERO,
        line_power_fraction: None,
        high_frequency_power_fraction: None,
    })
}
