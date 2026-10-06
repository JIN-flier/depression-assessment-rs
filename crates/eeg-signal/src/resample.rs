//! Time-domain windowed-sinc interpolation, independent of recording/history types.
//!
//! A downsampling kernel is widened by 1/ratio and its cutoff is reduced before
//! decimation. Merely picking every kth sample would fold high frequencies into EEG.
use crate::{
    ProcessingLimits, SignalError, SignalResult,
    error::{configuration, sample},
};
use std::f64::consts::PI;

pub(crate) struct ResamplePlan {
    pub output_count: usize,
    pub radius: usize,
    pub cutoff: f64,
    source_per_output: f64,
    pub identity: bool,
}

impl ResamplePlan {
    pub(crate) fn new(
        count: usize,
        channels: usize,
        source_rate: f64,
        target_rate: f64,
        half_width: usize,
        rolloff: f64,
        limits: ProcessingLimits,
    ) -> SignalResult<Self> {
        // Identity needs no kernel or floating-point length calculation, so even
        // strict kernel/work budgets should accept it after input shape checks.
        if source_rate == target_rate {
            return Ok(Self {
                output_count: count,
                radius: 0,
                cutoff: 0.0,
                source_per_output: 1.0,
                identity: true,
            });
        }
        let ratio = target_rate / source_rate;
        let size = (count as f64 * ratio).round();
        if !size.is_finite() || size >= usize::MAX as f64 {
            return Err(SignalError::LimitExceeded("resampled length"));
        }
        if size < 1.0 {
            return Err(configuration("resampling would produce an empty recording"));
        }
        let output_count = size as usize;
        if output_count
            .checked_mul(channels)
            .is_none_or(|n| n > limits.max_total_samples)
        {
            return Err(SignalError::LimitExceeded("resampled total samples"));
        }
        let radius_float = (half_width as f64 / ratio.min(1.0)).ceil();
        if !radius_float.is_finite()
            || radius_float > limits.max_kernel_radius as f64
            || radius_float >= (isize::MAX / 4) as f64
        {
            return Err(SignalError::LimitExceeded("sinc kernel radius"));
        }
        let radius = radius_float as usize;
        // Conservative upper bound includes all channels even if some are triggers.
        if output_count
            .checked_mul(channels)
            .and_then(|n| n.checked_mul(2 * radius + 1))
            .is_none_or(|n| n > limits.max_kernel_evaluations)
        {
            return Err(SignalError::LimitExceeded("sinc kernel evaluations"));
        }
        Ok(Self {
            output_count,
            radius,
            cutoff: 0.5 * ratio.min(1.0) * rolloff,
            source_per_output: source_rate / target_rate,
            identity: false,
        })
    }

    pub(crate) fn apply(&self, row: &[f32], trigger: bool) -> SignalResult<Vec<f32>> {
        if self.identity {
            return Ok(row.to_vec());
        }
        let mut output = Vec::with_capacity(self.output_count);
        for index in 0..self.output_count {
            let position = index as f64 * self.source_per_output;
            if trigger {
                // Zero-order hold keeps integer event codes intact. Very short pulses
                // may disappear on downsampling; event-aware conversion belongs to IO.
                output.push(row[(position.floor() as usize).min(row.len() - 1)]);
                continue;
            }
            let center = position.floor() as isize;
            let mut weighted_sum = 0.0;
            let mut weight_sum = 0.0;
            for offset in -(self.radius as isize)..=self.radius as isize {
                let source_index = center + offset;
                let distance = position - source_index as f64;
                if distance.abs() > self.radius as f64 {
                    continue;
                }
                let argument = 2.0 * self.cutoff * distance;
                let sinc = if argument.abs() < 1e-12 {
                    1.0
                } else {
                    (PI * argument).sin() / (PI * argument)
                };
                let window = 0.5 * (1.0 + (PI * distance / self.radius as f64).cos());
                let weight = 2.0 * self.cutoff * sinc * window;
                weighted_sum += weight * f64::from(row[reflect(source_index, row.len())]);
                weight_sum += weight;
            }
            // Per-phase normalization preserves constants exactly at finite-record
            // boundaries. Even reflection extends analog samples outside the record.
            if !weight_sum.is_finite() || weight_sum.abs() < 1e-12 {
                return Err(SignalError::Numerical {
                    operation: "resample",
                });
            }
            output.push(sample(weighted_sum / weight_sum, "resample")?);
        }
        Ok(output)
    }
}

fn reflect(index: isize, len: usize) -> usize {
    if len == 1 {
        return 0;
    }
    let period = 2 * (len as isize - 1);
    let folded = index.rem_euclid(period) as usize;
    if folded < len {
        folded
    } else {
        period as usize - folded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reflection_handles_both_edges_and_multiple_periods() {
        assert_eq!(
            (-5..=7).map(|i| reflect(i, 3)).collect::<Vec<_>>(),
            [1, 0, 1, 2, 1, 0, 1, 2, 1, 0, 1, 2, 1]
        );
        assert_eq!(reflect(-100, 1), 0);
    }
}
