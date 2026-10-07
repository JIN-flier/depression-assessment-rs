//! Welch 数值核：仅接收选中通道的样本、单位换算及已验证的窗参数。
use crate::{Detrend, FeatureError, FeatureResult, validation::Prepared};
use rustfft::{Fft, FftPlanner, num_complex::Complex};
use std::{f64::consts::TAU, sync::Arc};

/// 一次提取中复用 FFT 计划、Hann 窗及工作区；不在全局或提取器内缓存
/// 可变状态，因此不同调用可安全并行，内存随调用结束释放。
pub(crate) struct WelchEngine {
    fft: Arc<dyn Fft<f64>>,
    hann: Vec<f64>,
    window_energy: f64,
    buffer: Vec<Complex<f64>>,
    scratch: Vec<Complex<f64>>,
}

impl WelchEngine {
    pub fn new(shape: &Prepared) -> Self {
        let hann: Vec<_> = (0..shape.window)
            .map(|n| 0.5 - 0.5 * (TAU * n as f64 / shape.window as f64).cos())
            .collect();
        let window_energy = hann.iter().map(|value| value * value).sum();
        let fft = FftPlanner::<f64>::new().plan_fft_forward(shape.fft_size);
        let scratch = vec![Complex::default(); fft.get_inplace_scratch_len()];
        Self {
            fft,
            hann,
            window_energy,
            scratch,
            buffer: vec![Complex::default(); shape.fft_size],
        }
    }

    pub fn estimate(
        &mut self,
        row: &[f32],
        scale_uv: f64,
        rate: f64,
        detrend: Detrend,
        shape: &Prepared,
    ) -> FeatureResult<Vec<f64>> {
        let mut powers = vec![0.0; shape.fft_size / 2 + 1];
        for index in 0..shape.windows {
            let start = index * shape.hop;
            let segment = &row[start..start + shape.window];
            let mean = match detrend {
                Detrend::Constant => {
                    // 先以首样本为中心累加偏差，再恢复均值。数学上仍是
                    // 算术均值，但常量/大 DC 通道不会因长窗累加误差产生假功率。
                    let origin = f64::from(segment[0]) * scale_uv;
                    origin
                        + segment
                            .iter()
                            .map(|&v| f64::from(v) * scale_uv - origin)
                            .sum::<f64>()
                            / shape.window as f64
                }
                Detrend::None => 0.0,
            };
            // 尾部零填充仅属于 FFT 网格选择；没有补充原始信号样本。
            self.buffer.fill(Complex::default());
            for (n, &sample) in segment.iter().enumerate() {
                self.buffer[n].re = (f64::from(sample) * scale_uv - mean) * self.hann[n];
            }
            self.fft
                .process_with_scratch(&mut self.buffer, &mut self.scratch);
            for (k, power) in powers.iter_mut().enumerate() {
                // 实信号负频率折叠到正频率。偶数 FFT 才有独立 Nyquist bin；
                // 奇数 FFT 的最后一个正频率仍然有负频率配对，必须乘 2。
                let unpaired =
                    k == 0 || (shape.fft_size.is_multiple_of(2) && k == shape.fft_size / 2);
                let multiplier = if unpaired { 1.0 } else { 2.0 };
                // 分步除法避免 fs*sum(w²) 在极端但有限采样率下溢出。
                let density = self.buffer[k].norm_sqr() / self.window_energy / rate * multiplier;
                *power += density / shape.windows as f64;
            }
        }
        if powers
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0)
        {
            return Err(FeatureError::Numerical("Welch PSD"));
        }
        Ok(powers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FeatureConfig, validation::prepare};
    use domain::{
        Channel, ChannelKind, EegRecording, RecordingId, RecordingMetadata, RecordingState,
        SubjectId,
    };

    #[test]
    fn fft_psd_matches_independent_direct_dft_with_odd_even_and_padded_lengths() {
        // 小规模 O(N²) DFT 是独立参考实现，不调用 FFT 或生产积分代码。
        // 含 DC、非周期信号、重叠和未处理尾段，可检出缩放/奇偶/索引错误。
        for (window, fft_size) in [(7, 7), (8, 8), (7, 12), (8, 15)] {
            for detrend in [Detrend::None, Detrend::Constant] {
                let row: Vec<_> = (0..23).map(|n| ((n * 17 + 3) % 11) as f32 - 2.0).collect();
                let input = EegRecording::new(
                    RecordingId::new(),
                    SubjectId::new(),
                    64.0,
                    vec![Channel::new("Cz", ChannelKind::Eeg, "uV").unwrap()],
                    vec![row.clone()],
                    RecordingState::Raw,
                    RecordingMetadata::default(),
                )
                .unwrap();
                let mut config = FeatureConfig::default();
                config.welch.window_seconds = window as f64 / 64.0;
                config.welch.fft_size = Some(fft_size);
                let shape = prepare(&input, &config).unwrap();
                let actual = WelchEngine::new(&shape)
                    .estimate(&row, 1.0, 64.0, detrend, &shape)
                    .unwrap();
                let hann: Vec<_> = (0..window)
                    .map(|n| 0.5 - 0.5 * (TAU * n as f64 / window as f64).cos())
                    .collect();
                let energy = hann.iter().map(|v| v * v).sum::<f64>();
                for (k, observed) in actual.iter().enumerate() {
                    let mut expected = 0.0;
                    for segment in row.windows(window).step_by(shape.hop) {
                        let mean = if detrend == Detrend::Constant {
                            segment.iter().map(|&v| f64::from(v)).sum::<f64>() / window as f64
                        } else {
                            0.0
                        };
                        let (mut re, mut im) = (0.0, 0.0);
                        for (t, &value) in segment.iter().enumerate() {
                            let phase = TAU * k as f64 * t as f64 / fft_size as f64;
                            let v = (f64::from(value) - mean) * hann[t];
                            re += v * phase.cos();
                            im -= v * phase.sin();
                        }
                        let factor = if k == 0 || (fft_size.is_multiple_of(2) && k == fft_size / 2)
                        {
                            1.0
                        } else {
                            2.0
                        };
                        expected +=
                            factor * (re * re + im * im) / (64.0 * energy * shape.windows as f64);
                    }
                    assert!(
                        (observed - expected).abs() < 1e-12,
                        "n={window}, fft={fft_size}, k={k}: {observed} != {expected}"
                    );
                }
            }
        }
    }
}
