//! 解析功率、Parseval 能量守恒及窗平均测试，不从生产函数生成期望值。
mod support;
use domain::FrequencyBand;
use eeg_features::{BandDefinition, Detrend, FeatureConfig, FrequencyRange, SpectralExtractor};
use support::*;

#[test]
fn known_sines_have_correct_peak_absolute_power_and_relative_power() {
    for (frequency, band) in [
        (2.0, FrequencyBand::Delta),
        (6.0, FrequencyBand::Theta),
        (10.0, FrequencyBand::Alpha),
        (20.0, FrequencyBand::Beta),
    ] {
        let input = recording(256.0, vec![sine(256.0, frequency, 20.0, 2048)]);
        let result = extractor().extract(&input, &context()).unwrap();
        let s = result.spectral.unwrap();
        let psd = &s.psd_by_channel["Ch0"];
        let peak = psd
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        close(s.frequencies_hz[peak], frequency, 1e-12);
        close(s.band_power_by_channel["Ch0"][&band].absolute, 200.0, 2e-5);
        close(
            s.band_power_by_channel["Ch0"][&band].relative.get(),
            1.0,
            1e-12,
        );
        // 全单边谱积分应等于 A²/2，不因 50% 重叠改变幅值。
        close(psd.iter().sum::<f64>() * 0.5, 200.0, 2e-5);
    }
}

#[test]
fn band_subset_uses_explicit_reference_denominator() {
    let row = sine(256.0, 6.0, 10.0, 1024)
        .iter()
        .zip(sine(256.0, 10.0, 20.0, 1024))
        .map(|(a, b)| a + b)
        .collect();
    let mut config = FeatureConfig::default();
    config
        .bands
        .retain(|band| band.band == FrequencyBand::Alpha);
    let result = SpectralExtractor::new(config)
        .unwrap()
        .extract_detailed(&recording(256.0, vec![row]), &context())
        .unwrap();
    let s = result.features.spectral.unwrap();
    close(
        s.band_power_by_channel["Ch0"][&FrequencyBand::Alpha].absolute,
        200.0,
        2e-5,
    );
    close(
        result.audit.reference_power_uv2_by_channel["Ch0"],
        250.0,
        2e-5,
    );
    close(
        s.band_power_by_channel["Ch0"][&FrequencyBand::Alpha]
            .relative
            .get(),
        0.8,
        1e-7,
    );
}

#[test]
fn constant_and_zero_channels_have_explicit_zero_energy_without_nan() {
    for value in [0.0, 123.0, f32::MAX] {
        let result = extractor()
            .extract_detailed(&recording(256.0, vec![vec![value; 1024]]), &context())
            .unwrap();
        let s = result.features.spectral.unwrap();
        assert!(s.psd_by_channel["Ch0"].iter().all(|&v| v == 0.0));
        assert!(
            s.band_power_by_channel["Ch0"]
                .values()
                .all(|p| p.absolute == 0.0 && p.relative.get() == 0.0)
        );
        assert_eq!(result.audit.zero_power_channels, ["Ch0"]);
    }
}

#[test]
fn detrending_removes_dc_offset_without_changing_alpha_power() {
    let original = sine(256.0, 10.0, 20.0, 1024);
    let offset = original.iter().map(|v| v + 100.0).collect();
    let a = extractor()
        .extract(&recording(256.0, vec![original]), &context())
        .unwrap();
    let b = extractor()
        .extract(&recording(256.0, vec![offset]), &context())
        .unwrap();
    let pa = a.spectral.unwrap().band_power_by_channel["Ch0"][&FrequencyBand::Alpha].absolute;
    let pb = b.spectral.unwrap().band_power_by_channel["Ch0"][&FrequencyBand::Alpha].absolute;
    close(pb, pa, 1e-4); // f32 加偏置会产生不可避免的输入量化误差。
}

#[test]
fn unpaired_dc_and_nyquist_bins_preserve_energy_when_not_detrended() {
    let range = FrequencyRange {
        low_hz: 0.0,
        high_hz: 128.0,
    };
    let config = FeatureConfig {
        bands: vec![BandDefinition {
            band: FrequencyBand::Beta,
            range,
        }],
        relative_power_range: range,
        welch: eeg_features::WelchConfig {
            detrend: Detrend::None,
            ..Default::default()
        },
        ..FeatureConfig::default()
    };
    let extractor = SpectralExtractor::new(config).unwrap();
    for row in [
        vec![3.0; 512],
        (0_usize..512)
            .map(|n| if n.is_multiple_of(2) { 3.0 } else { -3.0 })
            .collect(),
    ] {
        let result = extractor
            .extract(&recording(256.0, vec![row]), &context())
            .unwrap();
        let s = result.spectral.unwrap();
        close(
            s.band_power_by_channel["Ch0"][&FrequencyBand::Beta].absolute,
            9.0,
            1e-12,
        );
        close(
            s.band_power_by_channel["Ch0"][&FrequencyBand::Beta]
                .relative
                .get(),
            1.0,
            1e-12,
        );
    }
}

#[test]
fn zero_padding_and_non_power_of_two_epochs_preserve_total_power() {
    for (rate, fft_size) in [(250.0, None), (250.0, Some(1000)), (250.0, Some(1001))] {
        let mut config = FeatureConfig::default();
        config.welch.fft_size = fft_size;
        let result = SpectralExtractor::new(config)
            .unwrap()
            .extract_detailed(
                &recording(rate, vec![sine(rate, 10.0, 20.0, 1500)]),
                &context(),
            )
            .unwrap();
        assert_eq!(result.audit.window_samples, 500);
        assert_eq!(result.audit.window_count, 5);
        let s = result.features.spectral.unwrap();
        close(
            s.psd_by_channel["Ch0"].iter().sum::<f64>() * result.audit.frequency_resolution_hz,
            200.0,
            3e-5,
        );
        close(
            s.band_power_by_channel["Ch0"][&FrequencyBand::Alpha].absolute,
            200.0,
            0.02,
        );
    }
}

#[test]
fn full_spectrum_integral_matches_independent_window_weighted_parseval_energy() {
    // 宽带非周期波形，比单频正弦更容易发现归一化/尾段/奇数 Nyquist 错误。
    for fft_size in [500, 501, 1000] {
        let row: Vec<_> = (0..1283)
            .map(|n| ((n * 17 + 5) % 37) as f32 - 18.0)
            .collect();
        let mut config = FeatureConfig::default();
        config.welch.fft_size = Some(fft_size);
        let result = SpectralExtractor::new(config)
            .unwrap()
            .extract_detailed(&recording(250.0, vec![row.clone()]), &context())
            .unwrap();
        let mut expected = 0.0;
        for segment in row.windows(500).step_by(250) {
            let mean = segment.iter().map(|&v| f64::from(v)).sum::<f64>() / 500.0;
            let (mut numerator, mut denominator) = (0.0, 0.0);
            for (n, &v) in segment.iter().enumerate() {
                let w = 0.5 - 0.5 * (std::f64::consts::TAU * n as f64 / 500.0).cos();
                numerator += ((f64::from(v) - mean) * w).powi(2);
                denominator += w * w;
            }
            expected += numerator / denominator / result.audit.window_count as f64;
        }
        close(
            result.features.spectral.unwrap().psd_by_channel["Ch0"]
                .iter()
                .sum::<f64>()
                * result.audit.frequency_resolution_hz,
            expected,
            1e-10,
        );
        assert_eq!(result.audit.trailing_samples, 33);
    }
}

#[test]
fn welch_averages_power_instead_of_amplitude_and_ignores_incomplete_tail() {
    let mut row = sine(256.0, 10.0, 10.0, 512);
    row.extend(sine(256.0, 10.0, 30.0, 512));
    row.extend([100_000.0; 17]);
    let mut config = FeatureConfig::default();
    config.welch.overlap_fraction = 0.0;
    let result = SpectralExtractor::new(config)
        .unwrap()
        .extract_detailed(&recording(256.0, vec![row]), &context())
        .unwrap();
    assert_eq!(result.audit.window_count, 2);
    assert_eq!(result.audit.trailing_samples, 17);
    close(
        result.features.spectral.unwrap().band_power_by_channel["Ch0"][&FrequencyBand::Alpha]
            .absolute,
        250.0,
        2e-5,
    );
}

#[test]
fn equivalent_uv_mv_and_volt_inputs_produce_same_physical_power() {
    let base = sine(256.0, 10.0, 20.0, 1024);
    let mut input = recording(
        256.0,
        vec![
            base.clone(),
            base.iter().map(|v| v / 1000.0).collect(),
            base.iter().map(|v| v / 1_000_000.0).collect(),
        ],
    );
    input.channels[1].unit = "mV".into();
    input.channels[2].unit = "V".into();
    let s = extractor()
        .extract(&input, &context())
        .unwrap()
        .spectral
        .unwrap();
    for channel in ["Ch0", "Ch1", "Ch2"] {
        close(
            s.band_power_by_channel[channel][&FrequencyBand::Alpha].absolute,
            200.0,
            2e-5,
        );
    }
}
