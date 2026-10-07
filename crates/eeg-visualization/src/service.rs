//! 只协调显示数据准备；既不重新执行 P6，也不依赖具体图像后端。
use crate::{error::configuration, validation as v, *};
use domain::{EegFeatures, EegRecording};

/// Object-safe 的服务边界，便于 P8 注入测试替身或其他显示策略。
pub trait EegVisualizer: Send + Sync {
    fn waveform(
        &self,
        input: &EegRecording,
        request: &WaveformRequest,
    ) -> VisualizationResult<WaveformPlot>;
    fn psd(&self, input: &EegFeatures, request: &PsdRequest) -> VisualizationResult<PsdPlot>;
    fn topomap(
        &self,
        input: &EegFeatures,
        layout: &ElectrodeLayout,
        request: &TopomapRequest,
    ) -> VisualizationResult<TopomapPlot>;
}

/// 不可变服务，各次调用仅持有局部工作区，可安全跨后台线程共享。
#[derive(Debug, Clone, Default)]
pub struct VisualizationService {
    limits: VisualizationLimits,
}

impl VisualizationService {
    pub fn new(limits: VisualizationLimits) -> VisualizationResult<Self> {
        v::limits(limits)?;
        Ok(Self { limits })
    }

    #[must_use]
    pub fn limits(&self) -> VisualizationLimits {
        self.limits
    }
}

impl EegVisualizer for VisualizationService {
    fn waveform(
        &self,
        input: &EegRecording,
        request: &WaveformRequest,
    ) -> VisualizationResult<WaveformPlot> {
        v::ordered_range(request.start_seconds, request.end_seconds)?;
        if request.max_points_per_channel < 4 {
            return Err(configuration("waveform point limit must be >= 4"));
        }
        v::recording(input, self.limits)?;
        let labels = v::select(v::eeg_labels(input), &request.channels)?;
        let fs = input.sampling_rate_hz;
        let end_seconds = request.end_seconds.min(input.duration_seconds);
        if request.start_seconds >= end_seconds {
            return Err(VisualizationError::EmptyRange);
        }
        // 最左侧包含第一个 t>=start 的样本；右侧不包含 t>=end 的样本。
        // 用实际时间比较修正 ceil 的浮点乘法边界，不提前一个采样点。
        let bound = |seconds: f64| {
            let mut index = (seconds * fs).ceil().min(input.sample_count() as f64) as usize;
            while index > 0 && (index - 1) as f64 / fs >= seconds {
                index -= 1;
            }
            while index < input.sample_count() && (index as f64) / fs < seconds {
                index += 1;
            }
            index
        };
        let start = bound(request.start_seconds);
        let end = bound(end_seconds);
        if start >= end {
            return Err(VisualizationError::EmptyRange);
        }
        let count = end - start;
        v::product(
            labels.len(),
            count.min(request.max_points_per_channel),
            self.limits.max_output_points,
            "waveform points",
        )?;
        let mut traces = Vec::with_capacity(labels.len());
        for label in labels {
            // labels 已验证，位置查找仍使用错误分支而非 unwrap。
            let index = input
                .channels
                .iter()
                .position(|c| c.label == label)
                .ok_or_else(|| VisualizationError::UnknownChannel(label.clone()))?;
            let scale = v::microvolt_scale(&input.channels[index].unit, &label)?;
            let samples = &input.samples[index][start..end];
            let indices = envelope_indices(samples, request.max_points_per_channel);
            let points = indices
                .into_iter()
                .map(|i| PlotPoint {
                    x: (start + i) as f64 / fs,
                    y: f64::from(samples[i]) * scale,
                })
                .collect();
            traces.push(WaveformTrace {
                channel: label,
                points,
                source_samples: count,
            });
        }
        Ok(WaveformPlot {
            recording_id: input.id,
            start_seconds: request.start_seconds,
            end_seconds,
            traces,
        })
    }

    fn psd(&self, input: &EegFeatures, request: &PsdRequest) -> VisualizationResult<PsdPlot> {
        v::ordered_range(request.min_hz, request.max_hz)?;
        if let PsdScale::Decibel { floor_uv2_per_hz } = request.scale
            && (!floor_uv2_per_hz.is_finite() || floor_uv2_per_hz <= 0.0)
        {
            return Err(configuration("PSD floor must be finite and positive"));
        }
        let spectral = v::spectral(input)?;
        v::budget(
            spectral.psd_by_channel.len(),
            self.limits.max_channels,
            "PSD channels",
        )?;
        let frequencies = &spectral.frequencies_hz;
        v::product(
            frequencies.len(),
            spectral.psd_by_channel.len().max(1),
            self.limits.max_input_values,
            "PSD values",
        )?;
        if frequencies.is_empty()
            || frequencies.iter().any(|f| !f.is_finite() || *f < 0.0)
            || frequencies.windows(2).any(|p| p[0] >= p[1])
        {
            return Err(error::invalid(
                "PSD frequencies must be non-negative and strictly increasing",
            ));
        }
        let labels = v::select(
            spectral.psd_by_channel.keys().cloned().collect(),
            &request.channels,
        )?;
        let start = frequencies.partition_point(|f| *f < request.min_hz);
        let end = frequencies.partition_point(|f| *f <= request.max_hz);
        if start == end {
            return Err(VisualizationError::EmptyRange);
        }
        v::product(
            labels.len(),
            end - start,
            self.limits.max_output_points,
            "PSD points",
        )?;
        let mut traces = Vec::with_capacity(labels.len());
        for label in labels {
            let psd = &spectral.psd_by_channel[&label];
            if psd.len() != frequencies.len() || psd.iter().any(|p| !p.is_finite() || *p < 0.0) {
                return Err(error::invalid("PSD shape mismatch or invalid power"));
            }
            let raw_uv2_per_hz = psd[start..end].to_vec();
            let mut floored_bins = 0;
            let points = frequencies[start..end]
                .iter()
                .zip(&raw_uv2_per_hz)
                .map(|(&x, &power)| {
                    let y = match request.scale {
                        PsdScale::Linear => power,
                        PsdScale::Decibel { floor_uv2_per_hz } => {
                            if power < floor_uv2_per_hz {
                                floored_bins += 1;
                            }
                            10.0 * power.max(floor_uv2_per_hz).log10()
                        }
                    };
                    PlotPoint { x, y }
                })
                .collect();
            traces.push(PsdTrace {
                channel: label,
                points,
                raw_uv2_per_hz,
                floored_bins,
            });
        }
        Ok(PsdPlot {
            recording_id: input.recording_id,
            scale: request.scale,
            traces,
        })
    }

    fn topomap(
        &self,
        input: &EegFeatures,
        layout: &ElectrodeLayout,
        request: &TopomapRequest,
    ) -> VisualizationResult<TopomapPlot> {
        topomap::prepare(input, layout, request, self.limits)
    }
}

/// 逐桶 min/max，而非等间隔抽点，避免窄尖峰消失；输出按原采样索引排序。
/// 首尾独立保留，桶覆盖内部全部样本。每桶至多 2 点，输出不超过 max_points。
fn envelope_indices(samples: &[f32], max_points: usize) -> Vec<usize> {
    if samples.len() <= max_points {
        return (0..samples.len()).collect();
    }
    let buckets = (max_points - 2) / 2;
    let inner = samples.len() - 2;
    let mut result = Vec::with_capacity(max_points);
    result.push(0);
    for bucket in 0..buckets {
        // 商/余数形式避免 inner*bucket 整数溢出，也避免最后一桶遗漏。
        let boundary = |b: usize| 1 + (inner / buckets) * b + (inner % buckets).min(b);
        let start = boundary(bucket);
        let end = boundary(bucket + 1);
        let (mut min, mut max) = (start, start);
        for i in start + 1..end {
            if samples[i] < samples[min] {
                min = i;
            }
            if samples[i] > samples[max] {
                max = i;
            }
        }
        result.push(min.min(max));
        if min != max {
            result.push(min.max(max));
        }
    }
    result.push(samples.len() - 1);
    result
}
