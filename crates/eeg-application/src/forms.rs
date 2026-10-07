//! 文本表单的验证同样不依赖 UI；禁止无效数字静默回退到默认配置。
use crate::*;
use domain::Channel;
use eeg_io::{CsvOptions, MatLayout, MatOptions, MatSamplingRate};
use eeg_signal::SignalStep;

fn positive(name: &str, text: &str) -> AppResult<f64> {
    let value: f64 = text
        .trim()
        .parse()
        .map_err(|_| AppError::InvalidInput(format!("{name}必须是数字")))?;
    if !value.is_finite() || value <= 0.0 {
        return Err(AppError::InvalidInput(format!("{name}必须是有限正数")));
    }
    Ok(value)
}

#[derive(Debug, Clone)]
pub struct SubjectForm {
    pub age: String,
    pub sex_index: i32,
    pub notes: String,
}
impl SubjectForm {
    pub fn parse(&self) -> AppResult<SubjectDraft> {
        let age = if self.age.trim().is_empty() {
            None
        } else {
            let age: u8 = self
                .age
                .trim()
                .parse()
                .map_err(|_| AppError::InvalidInput("年龄必须是 0–125 的整数或留空".into()))?;
            if age > domain::Subject::MAX_AGE {
                return Err(AppError::InvalidInput("年龄不能超过 125".into()));
            }
            Some(age)
        };
        let sex = match self.sex_index {
            0 => Sex::Unknown,
            1 => Sex::Female,
            2 => Sex::Male,
            3 => Sex::Intersex,
            4 => Sex::Other,
            _ => return Err(AppError::InvalidInput("请选择性别".into())),
        };
        Ok(SubjectDraft {
            age,
            sex,
            notes: self.notes.clone(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct ImportForm {
    pub format_index: i32,
    pub sampling_rate: String,
    pub unit: String,
    pub csv_headers: bool,
    pub labels: String,
    pub mat_variable: String,
    pub mat_rate_variable: String,
    pub mat_samples_by_channels: bool,
}
impl ImportForm {
    pub fn parse(&self) -> AppResult<ImportFormat> {
        if self.format_index == 0 {
            return Ok(ImportFormat::Edf);
        }
        let channels = self
            .labels
            .split(',')
            .filter(|s| !s.trim().is_empty())
            .map(|s| {
                Channel::new(s.trim(), ChannelKind::Eeg, self.unit.trim())
                    .map_err(|e| AppError::service("通道配置", e))
            })
            .collect::<AppResult<Vec<_>>>()?;
        match self.format_index {
            1 => {
                let mut options = CsvOptions::new(positive("CSV 采样率", &self.sampling_rate)?);
                options.unit = self.unit.trim().into();
                options.has_headers = self.csv_headers;
                options.channels = channels;
                if !options.has_headers && options.channels.is_empty() {
                    return Err(AppError::InvalidInput("无表头 CSV 必须填写通道标签".into()));
                }
                Ok(ImportFormat::Csv(options))
            }
            2 => {
                if channels.is_empty() || self.mat_variable.trim().is_empty() {
                    return Err(AppError::InvalidInput(
                        "MAT 必须填写矩阵变量名和通道标签".into(),
                    ));
                }
                let sampling_rate = if self.mat_rate_variable.trim().is_empty() {
                    MatSamplingRate::Hertz(positive("MAT 采样率", &self.sampling_rate)?)
                } else {
                    MatSamplingRate::Variable(self.mat_rate_variable.trim().into())
                };
                Ok(ImportFormat::Mat(MatOptions {
                    data_variable: self.mat_variable.trim().into(),
                    sampling_rate,
                    channels,
                    layout: if self.mat_samples_by_channels {
                        MatLayout::SamplesByChannels
                    } else {
                        MatLayout::ChannelsBySamples
                    },
                }))
            }
            _ => Err(AppError::InvalidInput("请选择 EDF / CSV / MAT".into())),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProcessingForm {
    pub dc: bool,
    pub bandpass: bool,
    pub low_hz: String,
    pub high_hz: String,
    pub notch: bool,
    pub notch_hz: String,
    pub resample: bool,
    pub target_hz: String,
    pub exclude_bad_channels: bool,
}
impl ProcessingForm {
    pub fn parse(&self) -> AppResult<AnalysisRequest> {
        let mut steps = Vec::new();
        if self.dc {
            steps.push(SignalStep::DcRemoval {});
        }
        if self.bandpass {
            let low = positive("高通频率", &self.low_hz)?;
            let high = positive("低通频率", &self.high_hz)?;
            if low >= high {
                return Err(AppError::InvalidInput("高通频率必须小于低通频率".into()));
            }
            steps.push(SignalStep::bandpass(low, high));
        }
        if self.notch {
            steps.push(SignalStep::notch(positive("陷波频率", &self.notch_hz)?));
        }
        if self.resample {
            steps.push(SignalStep::resample(positive(
                "目标采样率",
                &self.target_hz,
            )?));
        }
        Ok(AnalysisRequest {
            pipeline: PipelineConfig {
                steps,
                ..Default::default()
            },
            exclude_bad_channels: self.exclude_bad_channels,
            ..Default::default()
        })
    }
}

impl ViewRequest {
    pub fn validate(&self) -> AppResult<()> {
        if !self.start_seconds.is_finite() || self.start_seconds < 0.0 {
            return Err(AppError::InvalidInput("窗口起点必须是有限非负数".into()));
        }
        if !self.window_seconds.is_finite()
            || self.window_seconds <= 0.0
            || !self.amplitude_uv.is_finite()
            || !(0.001..=1e9).contains(&self.amplitude_uv)
            || !(self.start_seconds + self.window_seconds).is_finite()
        {
            return Err(AppError::InvalidInput(
                "窗口长度必须为有限正数，幅度范围须为 0.001–1e9 uV".into(),
            ));
        }
        // 8 是 UI 同屏行数，不是所选通道上限。这里仅应用 P7 的资源预算。
        if self
            .channels
            .as_ref()
            .is_some_and(|channels| channels.len() > 256)
        {
            return Err(AppError::InvalidInput("显示通道超过绘图预算（256）".into()));
        }
        Ok(())
    }
}
