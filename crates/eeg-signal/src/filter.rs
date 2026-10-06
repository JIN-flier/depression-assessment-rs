//! RBJ coefficients: https://www.w3.org/TR/audio-eq-cookbook/
//! Private DSP primitives keep pipeline orchestration free of filter math.
use crate::{FilterPhase, SignalError, SignalResult, error::sample};
use std::f64::consts::{FRAC_1_SQRT_2, PI};

/// Fixed padding is part of algorithm version 1 and recorded in history.
pub(crate) const REFLECTION_SAMPLES: usize = 24;

#[derive(Clone, Copy)]
pub(crate) struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
}
impl Biquad {
    fn design(rate: f64, frequency: f64, q: f64, kind: u8) -> SignalResult<Self> {
        let angle = 2.0 * PI * (frequency / rate);
        let (sin, cos) = angle.sin_cos();
        let alpha = sin / (2.0 * q);
        // Half-angle expressions avoid cancellation near DC/Nyquist.
        let b = match kind {
            0 => {
                let v = (angle / 2.0).sin().powi(2);
                [v, 2.0 * v, v]
            }
            1 => {
                let v = (angle / 2.0).cos().powi(2);
                [v, -2.0 * v, v]
            }
            _ => [1.0, -2.0 * cos, 1.0],
        };
        let denominator = 1.0 + alpha;
        let filter = Self {
            b: b.map(|v| v / denominator),
            a: [-2.0 * cos / denominator, (1.0 - alpha) / denominator],
        };
        // Strict second-order Jury conditions reject numerically degenerate poles
        // at +-1 even when a frequency was formally inside Nyquist.
        if filter.b.iter().chain(&filter.a).any(|v| !v.is_finite())
            || filter.a[1].abs() >= 1.0
            || 1.0 + filter.a[0] + filter.a[1] <= 0.0
            || 1.0 - filter.a[0] + filter.a[1] <= 0.0
        {
            return Err(crate::error::configuration(
                "filter coefficients are numerically unstable",
            ));
        }
        Ok(filter)
    }
    pub(crate) fn bandpass(rate: f64, low: f64, high: f64) -> SignalResult<Vec<Self>> {
        Ok(vec![
            Self::design(rate, low, FRAC_1_SQRT_2, 1)?,
            Self::design(rate, high, FRAC_1_SQRT_2, 0)?,
        ])
    }
    pub(crate) fn notch(rate: f64, frequency: f64, q: f64) -> SignalResult<Vec<Self>> {
        Ok(vec![Self::design(rate, frequency, q, 2)?])
    }
    fn run(self, data: &mut [f64]) -> SignalResult<()> {
        // Direct form II transposed, initialized to first-input steady state.
        // This prevents artificial startup jumps on constant EEG.
        let dc_gain = self.b.iter().sum::<f64>() / (1.0 + self.a[0] + self.a[1]);
        let x0 = data[0];
        let y0 = x0 * dc_gain;
        let mut z1 = y0 - self.b[0] * x0;
        let mut z2 = self.b[2] * x0 - self.a[1] * y0;
        for x in data {
            let y = self.b[0] * *x + z1;
            z1 = self.b[1] * *x - self.a[0] * y + z2;
            z2 = self.b[2] * *x - self.a[1] * y;
            if !y.is_finite() || !z1.is_finite() || !z2.is_finite() {
                return Err(SignalError::Numerical {
                    operation: "biquad",
                });
            }
            *x = y;
        }
        Ok(())
    }
}

pub(crate) fn apply(row: &mut [f32], filters: &[Biquad], phase: FilterPhase) -> SignalResult<()> {
    let padding = if phase == FilterPhase::ZeroPhase {
        REFLECTION_SAMPLES
    } else {
        0
    };
    if row.len() <= padding {
        return Err(SignalError::TooShort {
            operation: "filter",
            minimum: padding + 1,
            actual: row.len(),
        });
    }
    let mut work = Vec::with_capacity(row.len() + 2 * padding);
    // Odd reflection preserves endpoint slope rather than assuming zeros.
    for index in (1..=padding).rev() {
        work.push(2.0 * f64::from(row[0]) - f64::from(row[index]));
    }
    work.extend(row.iter().map(|&value| f64::from(value)));
    for offset in 1..=padding {
        work.push(2.0 * f64::from(row[row.len() - 1]) - f64::from(row[row.len() - 1 - offset]));
    }
    for &filter in filters {
        filter.run(&mut work)?;
    }
    if phase == FilterPhase::ZeroPhase {
        work.reverse();
        for &filter in filters {
            filter.run(&mut work)?;
        }
        work.reverse();
    }
    for (index, value) in row.iter_mut().enumerate() {
        *value = sample(work[index + padding], "filter")?;
    }
    Ok(())
}
