//! 仅负责协调验证、Welch 与积分，并组装领域结果及 provenance。
use crate::{
    FeatureConfig, FeatureContext, FeatureError, FeatureExtraction, FeatureResult, SpectralAudit,
    band_power, validation, welch::WelchEngine,
};
use domain::{BandPower, EegFeatures, EegRecording, Provenance, SpectralFeatures, UnitInterval};
use std::collections::BTreeMap;

/// 应用层/测试替身只依赖此 object-safe 接口，可替换算法实现。
pub trait EegFeatureExtractor: Send + Sync {
    fn extract(&self, input: &EegRecording, context: &FeatureContext)
    -> FeatureResult<EegFeatures>;
}

/// 可共享的不可变提取服务；所有逐次调用的数值工作区都属于局部变量。
#[derive(Debug, Clone)]
pub struct SpectralExtractor {
    config: FeatureConfig,
}

impl SpectralExtractor {
    /// 验证静态配置。采样率相关的限制在 extract 时结合录制重新检查。
    pub fn new(config: FeatureConfig) -> FeatureResult<Self> {
        validation::validate_config(&config)?;
        Ok(Self { config })
    }

    /// 只读配置视图；调用者不能在构造后绕过参数验证。
    #[must_use]
    pub fn config(&self) -> &FeatureConfig {
        &self.config
    }

    /// 返回稳定的 P1 领域结果；详细审计信息随 provenance 一同保留。
    pub fn extract(
        &self,
        input: &EegRecording,
        context: &FeatureContext,
    ) -> FeatureResult<EegFeatures> {
        Ok(self.extract_detailed(input, context)?.features)
    }

    /// 返回领域结果及类型化审计数据。失败时不修改输入或产出部分结果。
    pub fn extract_detailed(
        &self,
        input: &EegRecording,
        context: &FeatureContext,
    ) -> FeatureResult<FeatureExtraction> {
        let mut provenance = Provenance::new(
            &context.software_version,
            "welch-band-power",
            "p6-welch-v1",
            context.generated_at,
        )?;
        let shape = validation::prepare(input, &self.config)?;
        let bin_width = input.sampling_rate_hz / shape.fft_size as f64;
        if !bin_width.is_finite() || bin_width <= 0.0 {
            return Err(FeatureError::Numerical("frequency spacing"));
        }
        let nyquist = input.sampling_rate_hz / 2.0;
        let frequencies: Vec<_> = (0..=shape.fft_size / 2)
            .map(|k| {
                // 偶数长度最后一点直接取 Nyquist，避免浮点乘法舍入影响端点归属。
                if shape.fft_size.is_multiple_of(2) && k == shape.fft_size / 2 {
                    nyquist
                } else {
                    k as f64 * bin_width
                }
            })
            .collect();
        let reference_bins = band_power::bins(
            &frequencies,
            self.config.relative_power_range,
            nyquist,
            bin_width,
        )?;
        let band_bins: Vec<_> = self
            .config
            .bands
            .iter()
            .map(|band| band_power::bins(&frequencies, band.range, nyquist, bin_width))
            .collect::<FeatureResult<_>>()?;
        let mut audit = SpectralAudit {
            window_samples: shape.window,
            overlap_samples: shape.overlap,
            hop_samples: shape.hop,
            fft_size: shape.fft_size,
            window_count: shape.windows,
            trailing_samples: shape.trailing,
            frequency_resolution_hz: bin_width,
            included_channels: shape
                .selected
                .iter()
                .map(|selected| input.channels[selected.index].label.clone())
                .collect(),
            excluded_channels: shape.excluded.clone(),
            reference_power_uv2_by_channel: BTreeMap::new(),
            zero_power_channels: Vec::new(),
        };
        let mut psd_by_channel = BTreeMap::new();
        let mut band_power_by_channel = BTreeMap::new();
        let mut engine = WelchEngine::new(&shape);
        for selected in &shape.selected {
            let label = &input.channels[selected.index].label;
            let psd = engine.estimate(
                &input.samples[selected.index],
                selected.scale_uv,
                input.sampling_rate_hz,
                self.config.welch.detrend,
                &shape,
            )?;
            let total = band_power::integrate(&psd, reference_bins.clone(), bin_width)?;
            if total == 0.0 {
                audit.zero_power_channels.push(label.clone());
            }
            audit
                .reference_power_uv2_by_channel
                .insert(label.clone(), total);
            let mut bands = BTreeMap::new();
            for (band, bins) in self.config.bands.iter().zip(&band_bins) {
                let absolute = band_power::integrate(&psd, bins.clone(), bin_width)?;
                let relative = if total == 0.0 { 0.0 } else { absolute / total };
                // 子集求和在最后几个 ulp 处可能比总和略大，只容忍舍入误差。
                if !relative.is_finite() || !(0.0..=1.0 + 1e-12).contains(&relative) {
                    return Err(FeatureError::Numerical("relative band power"));
                }
                bands.insert(
                    band.band,
                    BandPower {
                        absolute,
                        relative: UnitInterval::new("relative band power", relative.min(1.0))?,
                    },
                );
            }
            psd_by_channel.insert(label.clone(), psd);
            band_power_by_channel.insert(label.clone(), bands);
        }
        let spectral = SpectralFeatures::new(frequencies, psd_by_channel, band_power_by_channel)?;
        let p = &mut provenance.parameters;
        p.insert("configuration".into(), serde_json::to_string(&self.config)?);
        p.insert("audit".into(), serde_json::to_string(&audit)?);
        p.insert(
            "input_processing_history".into(),
            serde_json::to_string(&input.processing_history)?,
        );
        p.insert(
            "input_recording_state".into(),
            serde_json::to_string(&input.recording_state)?,
        );
        p.insert(
            "input_channels".into(),
            serde_json::to_string(&input.channels)?,
        );
        p.insert(
            "input_sampling_rate_hz".into(),
            input.sampling_rate_hz.to_string(),
        );
        p.insert(
            "input_sample_count".into(),
            input.sample_count().to_string(),
        );
        p.insert("psd_unit".into(), "uV^2/Hz".into());
        p.insert("band_power_unit".into(), "uV^2".into());
        p.insert("window_function".into(), "periodic-hann".into());
        p.insert("averaging".into(), "arithmetic-mean".into());
        p.insert(
            "band_integration".into(),
            "bin-center-half-open-nyquist-inclusive-rectangle-v1".into(),
        );
        p.insert(
            "eeg_features_version".into(),
            env!("CARGO_PKG_VERSION").into(),
        );
        p.insert("fft_backend".into(), "rustfft-6".into());
        Ok(FeatureExtraction {
            features: EegFeatures::spectral(input.id, spectral, provenance),
            audit,
        })
    }
}

impl EegFeatureExtractor for SpectralExtractor {
    fn extract(
        &self,
        input: &EegRecording,
        context: &FeatureContext,
    ) -> FeatureResult<EegFeatures> {
        SpectralExtractor::extract(self, input, context)
    }
}
