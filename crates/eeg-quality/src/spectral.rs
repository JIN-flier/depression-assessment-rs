//! Private quality-only spectral engine. It returns noise ratios, not P6 EEG
//! features. Non-overlapping, constant-detrended periodic-Hann periodograms are
//! summed before taking ratios (power-weighted across epochs).
use crate::{SpectralConfig, time_domain::Moments};
use std::f64::consts::TAU;

pub(crate) struct SpectralMeasurements {
    pub windows: usize,
    pub line: Option<f64>,
    pub high: Option<f64>,
    pub unavailable: Vec<&'static str>,
}

pub(crate) fn measure(
    row: &[f32],
    scale_uv: f64,
    rate: f64,
    window: usize,
    fft_size: usize,
    config: &SpectralConfig,
) -> SpectralMeasurements {
    let mut result = SpectralMeasurements {
        windows: 0,
        line: None,
        high: None,
        unavailable: Vec::new(),
    };
    if config.line_frequency_hz.is_none() && config.high_frequency_start_hz.is_none() {
        return result;
    }
    let nyquist = rate / 2.0;
    let line_available = config.line_frequency_hz.is_some_and(|line| {
        line + config.line_half_width_hz < nyquist
            // Zero padding does not improve the true resolving power of an epoch.
            && rate / window as f64 <= config.line_half_width_hz
    });
    let high_available = config
        .high_frequency_start_hz
        .is_some_and(|high| high < nyquist);
    if config.line_frequency_hz.is_some() && !line_available {
        result.unavailable.push("LINE_BAND_UNAVAILABLE");
    }
    if config.high_frequency_start_hz.is_some() && !high_available {
        result.unavailable.push("HIGH_FREQUENCY_BAND_UNAVAILABLE");
    }
    if row.len() < window {
        result.unavailable.push("NO_VALID_SPECTRAL_WINDOWS");
        return result;
    }

    let hann: Vec<f64> = (0..window)
        .map(|i| 0.5 - 0.5 * (TAU * i as f64 / window as f64).cos())
        .collect();
    let mut buffer = vec![Complex::default(); fft_size];
    let mut powers = vec![0.0; fft_size / 2 + 1];
    for epoch in row.chunks_exact(window) {
        // Never bridge a missing-data gap or zero-fill missing EEG. Zero padding
        // here extends a *valid* epoch to the radix-2 FFT length only.
        if epoch.iter().any(|value| !value.is_finite()) {
            continue;
        }
        let mut moments = Moments::default();
        for &value in epoch {
            moments.add(f64::from(value) * scale_uv);
        }
        buffer.fill(Complex::default());
        for (i, &value) in epoch.iter().enumerate() {
            buffer[i].re = (f64::from(value) * scale_uv - moments.mean()) * hann[i];
        }
        fft(&mut buffer);
        for (i, power) in powers.iter_mut().enumerate() {
            let value = buffer[i];
            // A real signal's negative frequencies contribute to positive ones.
            // DC and the Nyquist bin have no distinct negative counterpart.
            let multiplier = if i == 0 || i == fft_size / 2 {
                1.0
            } else {
                2.0
            };
            *power += multiplier * (value.re * value.re + value.im * value.im);
        }
        result.windows += 1;
    }
    if result.windows == 0 {
        result.unavailable.push("NO_VALID_SPECTRAL_WINDOWS");
        return result;
    }

    // The PSD scale 1/(fs*sum(w²)), bin width and epoch averaging factor are
    // identical in numerator/denominator and cancel exactly. Working with raw
    // powers avoids unnecessary underflow at extreme but finite sampling rates.
    let (mut total, mut line_power, mut high_power) = (0.0, 0.0, 0.0);
    let (mut total_bins, mut line_bins, mut high_bins) = (0, 0, 0);
    for (i, &power) in powers.iter().enumerate().skip(1) {
        let frequency = i as f64 * (rate / fft_size as f64);
        if frequency >= config.minimum_hz {
            total += power;
            total_bins += 1;
            if let Some(line) = config.line_frequency_hz
                && (frequency - line).abs() <= config.line_half_width_hz
            {
                line_power += power;
                line_bins += 1;
            }
            if let Some(high) = config.high_frequency_start_hz
                && frequency >= high
            {
                high_power += power;
                high_bins += 1;
            }
        }
    }
    if total_bins == 0 {
        result.unavailable.push("ANALYSIS_BAND_UNAVAILABLE");
        return result;
    }
    // Zero-power valid epochs have zero noise ratio; the time-domain detectors
    // flag their flatness/low variance. They must not yield 0/0 or NaN.
    let ratio = |power: f64| {
        if total > 0.0 {
            (power / total).clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    if line_available && line_bins > 0 {
        result.line = Some(ratio(line_power));
    }
    if high_available && high_bins > 0 {
        result.high = Some(ratio(high_power));
    }
    result
}

#[derive(Clone, Copy, Default)]
struct Complex {
    re: f64,
    im: f64,
}

/// Iterative radix-2 Cooley–Tukey forward FFT, unnormalized, in place.
/// The analyzer validates a bounded power-of-two length before calling this.
/// No feature-engine, external runtime or mutable global FFT cache is needed.
fn fft(values: &mut [Complex]) {
    let n = values.len();
    let mut reversed = 0;
    for index in 1..n {
        let mut bit = n >> 1;
        while reversed & bit != 0 {
            reversed ^= bit;
            bit >>= 1;
        }
        reversed ^= bit;
        if index < reversed {
            values.swap(index, reversed);
        }
    }
    let mut width = 2;
    loop {
        for block in values.chunks_exact_mut(width) {
            for i in 0..width / 2 {
                let (sin, cos) = (-TAU * i as f64 / width as f64).sin_cos();
                let odd = block[i + width / 2];
                let product = Complex {
                    re: odd.re * cos - odd.im * sin,
                    im: odd.re * sin + odd.im * cos,
                };
                let even = block[i];
                block[i] = Complex {
                    re: even.re + product.re,
                    im: even.im + product.im,
                };
                block[i + width / 2] = Complex {
                    re: even.re - product.re,
                    im: even.im - product.im,
                };
            }
        }
        if width == n {
            break;
        }
        width *= 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_matches_independent_direct_dft_including_complex_phase() {
        for n in [8, 16, 64] {
            let input: Vec<Complex> = (0..n)
                .map(|i| Complex {
                    re: ((i * 13 + 3) % 17) as f64 - 8.0,
                    im: (i % 3) as f64,
                })
                .collect();
            let mut actual = input.clone();
            fft(&mut actual);
            for (k, value) in actual.iter().enumerate() {
                let mut expected = Complex::default();
                for (t, sample) in input.iter().enumerate() {
                    let (sin, cos) = (-TAU * k as f64 * t as f64 / n as f64).sin_cos();
                    expected.re += sample.re * cos - sample.im * sin;
                    expected.im += sample.re * sin + sample.im * cos;
                }
                assert!((value.re - expected.re).abs() < 1e-10);
                assert!((value.im - expected.im).abs() < 1e-10);
            }
        }
    }
}
