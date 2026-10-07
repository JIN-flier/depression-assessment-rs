//! Private quality-only spectral engine. It returns noise ratios, not P6 EEG
//! features. Non-overlapping, constant-detrended periodic-Hann periodograms are
//! summed before taking ratios (power-weighted across epochs).
use crate::{SpectralConfig, time_domain::Moments};
use rustfft::{FftPlanner, num_complex::Complex};
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
    // Plan once per channel assessment, then reuse the plan and scratch across
    // all epochs. `process_with_scratch` avoids an allocation per FFT. These
    // buffers belong to this call, so concurrent assessments stay independent.
    // RustFFT's forward transform is unnormalized, matching the power-ratio
    // convention below; no extra scaling or frequency reordering is needed.
    let fft = FftPlanner::<f64>::new().plan_fft_forward(fft_size);
    let mut scratch = vec![Complex::default(); fft.get_inplace_scratch_len()];
    for epoch in row.chunks_exact(window) {
        // Never bridge a missing-data gap or zero-fill missing EEG. Zero padding
        // here extends a *valid* epoch to the selected power-of-two FFT length.
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
        fft.process_with_scratch(&mut buffer, &mut scratch);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frequency_ratios_match_independent_direct_dft_with_plan_and_scratch_reuse() {
        // Exercise the production measurement path, not just RustFFT itself.
        // Multiple non-periodic epochs test plan/buffer reuse; a non-power-of-two
        // epoch tests zero padding. The reference uses O(N²) DFT and no FFT API.
        for window in [8_usize, 13, 32] {
            let fft_size = window.next_power_of_two();
            let row: Vec<f32> = (0..window * 3)
                .map(|i| ((i * 13 + 3) % 17) as f32 - 2.0)
                .collect();
            let config = SpectralConfig {
                minimum_hz: 1.0,
                line_frequency_hz: Some(5.0),
                line_half_width_hz: 2.0,
                high_frequency_start_hz: Some(6.0),
            };
            let actual = measure(&row, 1.0, 16.0, window, fft_size, &config);
            let (mut total, mut line, mut high) = (0.0, 0.0, 0.0);
            for epoch in row.chunks_exact(window) {
                let mean = epoch.iter().map(|&v| f64::from(v)).sum::<f64>() / window as f64;
                for k in 1..=fft_size / 2 {
                    let (mut re, mut im) = (0.0, 0.0);
                    for (t, &value) in epoch.iter().enumerate() {
                        let hann = 0.5 - 0.5 * (TAU * t as f64 / window as f64).cos();
                        let value = (f64::from(value) - mean) * hann;
                        let phase = TAU * k as f64 * t as f64 / fft_size as f64;
                        re += value * phase.cos();
                        im -= value * phase.sin();
                    }
                    let power = (re * re + im * im) * if k == fft_size / 2 { 1.0 } else { 2.0 };
                    let frequency = k as f64 * 16.0 / fft_size as f64;
                    if frequency >= 1.0 {
                        total += power;
                        if (3.0..=7.0).contains(&frequency) {
                            line += power;
                        }
                        if frequency >= 6.0 {
                            high += power;
                        }
                    }
                }
            }
            assert_eq!(actual.windows, 3);
            assert!(actual.unavailable.is_empty());
            assert!((actual.line.unwrap() - line / total).abs() < 1e-12);
            assert!((actual.high.unwrap() - high / total).abs() < 1e-12);
        }
    }
}
