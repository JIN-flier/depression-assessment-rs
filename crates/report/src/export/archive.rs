//! 归档 DTO 无原始样本字段。录制信息、完整计算结果和参数显式分开，避免
//! 误把 EegRecording 的 Serialize（含大型/敏感 samples）用于结果导出。
use super::ExportError;
use crate::ReportDocument;
use domain::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordingDescriptor {
    pub id: RecordingId,
    pub subject_id: SubjectId,
    pub sampling_rate_hz: f64,
    pub duration_seconds: f64,
    pub sample_count: usize,
    pub channels: Vec<Channel>,
    pub recording_state: RecordingState,
    pub metadata: RecordingMetadata,
    pub processing_history: Vec<ProcessingStep>,
}
impl From<&EegRecording> for RecordingDescriptor {
    fn from(r: &EegRecording) -> Self {
        Self {
            id: r.id,
            subject_id: r.subject_id,
            sampling_rate_hz: r.sampling_rate_hz,
            duration_seconds: r.duration_seconds,
            sample_count: r.sample_count(),
            channels: r.channels.clone(),
            recording_state: r.recording_state,
            metadata: r.metadata.clone(),
            processing_history: r.processing_history.clone(),
        }
    }
}

/// 参数类型属于各算法 crate，report 仅依赖 serde 合同，不反向依赖其实现。
/// 这是 JSON 值而非 Debug 字符串，研究脚本能直接读取嵌套参数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnalysisParameters {
    pub pipeline: Value,
    pub quality: Value,
    pub features: Value,
    pub exclude_bad_channels: bool,
}
impl AnalysisParameters {
    pub fn new<P: Serialize, Q: Serialize, F: Serialize>(
        pipeline: &P,
        quality: &Q,
        features: &F,
        exclude_bad_channels: bool,
    ) -> Result<Self, ExportError> {
        Ok(Self {
            pipeline: serde_json::to_value(pipeline).map_err(ExportError::Serialization)?,
            quality: serde_json::to_value(quality).map_err(ExportError::Serialization)?,
            features: serde_json::to_value(features).map_err(ExportError::Serialization)?,
            exclude_bad_channels,
        })
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnalysisArchive {
    pub raw: RecordingDescriptor,
    pub processed: RecordingDescriptor,
    pub raw_quality: SignalQuality,
    pub processed_quality: SignalQuality,
    pub features: EegFeatures,
    pub parameters: AnalysisParameters,
}

/// 归档读回的是普通数据，不能反序列化成 ReportDocument 或绕过 Narrative Validator。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArchivedReport {
    pub context: ReportContext,
    pub provider: String,
    pub summary: String,
    pub description: String,
    pub interpretation: String,
    pub limitations: Vec<String>,
}
impl From<&ReportDocument> for ArchivedReport {
    fn from(d: &ReportDocument) -> Self {
        Self {
            context: d.context().clone(),
            provider: d.provider().into(),
            summary: d.narrative().summary().into(),
            description: d.narrative().description().into(),
            interpretation: d.narrative().interpretation().into(),
            limitations: d.narrative().limitations().to_vec(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExportBundle {
    pub schema_version: &'static str,
    pub software_version: &'static str,
    pub subject: Subject,
    pub analysis: AnalysisArchive,
    /// 字段私有：外部无法把任意文本装进面向用户的报告导出。
    pub(crate) report: Option<ArchivedReport>,
}
impl ExportBundle {
    pub fn new(
        subject: Subject,
        analysis: AnalysisArchive,
        report: Option<&ReportDocument>,
    ) -> Result<Self, ExportError> {
        let bundle = Self {
            schema_version: "eeg-v1-export/1",
            software_version: env!("CARGO_PKG_VERSION"),
            subject,
            analysis,
            report: report.map(ArchivedReport::from),
        };
        bundle.validate()?;
        Ok(bundle)
    }
    pub fn validate(&self) -> Result<(), ExportError> {
        let a = &self.analysis;
        if a.raw.subject_id != self.subject.id
            || a.processed.subject_id != self.subject.id
            || a.raw_quality.recording_id != a.raw.id
            || a.processed_quality.recording_id != a.processed.id
            || a.features.recording_id != a.processed.id
            || a.raw.id == a.processed.id
            || a.processed.metadata.extra.get("source_recording_id") != Some(&a.raw.id.to_string())
        {
            return Err(ExportError::InvalidArchive);
        }
        for (r, q) in [
            (&a.raw, &a.raw_quality),
            (&a.processed, &a.processed_quality),
        ] {
            let labels: std::collections::BTreeSet<_> =
                r.channels.iter().map(|c| c.label.as_str()).collect();
            if labels.len() != r.channels.len()
                || q.channel_quality
                    .keys()
                    .any(|label| !labels.contains(label.as_str()))
                || q.bad_channels
                    .iter()
                    .any(|label| !q.channel_quality.contains_key(label))
            {
                return Err(ExportError::InvalidArchive);
            }
            if !r.sampling_rate_hz.is_finite()
                || r.sampling_rate_hz <= 0.0
                || !r.duration_seconds.is_finite()
                || r.sample_count == 0
                || r.channels.is_empty()
                || (r.duration_seconds - r.sample_count as f64 / r.sampling_rate_hz).abs() > 1e-8
            {
                return Err(ExportError::InvalidArchive);
            }
        }
        let s = a
            .features
            .spectral
            .as_ref()
            .ok_or(ExportError::InvalidArchive)?;
        let finite = |v: f64| v.is_finite() && v >= 0.0;
        if s.frequencies_hz.is_empty()
            || s.band_power_by_channel.is_empty()
            || s.frequencies_hz.iter().any(|&v| !finite(v))
            || s.frequencies_hz.windows(2).any(|w| w[1] <= w[0])
        {
            return Err(ExportError::InvalidArchive);
        }
        // 编码前限制标量数，避免大矩阵在 pretty JSON / 长 CSV 中无界扩张。
        let total_values = s
            .psd_by_channel
            .values()
            .try_fold(s.frequencies_hz.len(), |count, psd| {
                count.checked_add(psd.len())
            })
            .ok_or(ExportError::TooLarge)?;
        if total_values > 4_000_000 {
            return Err(ExportError::TooLarge);
        }
        for group in [
            &a.features.spatial,
            &a.features.connectivity,
            &a.features.complexity,
        ]
        .into_iter()
        .flatten()
        {
            if group.values.values().any(|v| !v.is_finite()) {
                return Err(ExportError::InvalidArchive);
            }
        }
        for (label, psd) in &s.psd_by_channel {
            if !a.processed.channels.iter().any(|c| &c.label == label)
                || psd.len() != s.frequencies_hz.len()
                || psd.iter().any(|&v| !finite(v))
            {
                return Err(ExportError::InvalidArchive);
            }
        }
        for (label, bands) in &s.band_power_by_channel {
            if !s.psd_by_channel.contains_key(label) || bands.values().any(|p| !finite(p.absolute))
            {
                return Err(ExportError::InvalidArchive);
            }
        }
        if let Some(r) = &self.report {
            // 同一 recording ID 仍不足以证明正文和数值属于同一次成功分析。
            // 复建 P9 的事实投影（仅读取现成标量），拒绝导出陈旧正文配新指标。
            let mut expected = std::collections::BTreeMap::from([
                (
                    "raw_quality_score".to_string(),
                    a.raw_quality.overall_score.get(),
                ),
                (
                    "processed_quality_score".to_string(),
                    a.processed_quality.overall_score.get(),
                ),
                (
                    "raw_bad_channel_count".to_string(),
                    a.raw_quality.bad_channels.len() as f64,
                ),
                (
                    "processed_bad_channel_count".to_string(),
                    a.processed_quality.bad_channels.len() as f64,
                ),
                ("sampling_rate_hz".to_string(), a.processed.sampling_rate_hz),
                ("duration_seconds".to_string(), a.processed.duration_seconds),
            ]);
            for (index, channel) in a.processed.channels.iter().enumerate() {
                if let Some(bands) = s.band_power_by_channel.get(&channel.label) {
                    if r.context.metadata.get(&format!("channel_{}", index + 1))
                        != Some(&channel.label)
                    {
                        return Err(ExportError::InvalidArchive);
                    }
                    for (band, power) in bands {
                        let band = format!("{band:?}").to_ascii_lowercase();
                        expected.insert(
                            format!("channel_{}.{band}.absolute", index + 1),
                            power.absolute,
                        );
                        expected.insert(
                            format!("channel_{}.{band}.relative", index + 1),
                            power.relative.get(),
                        );
                    }
                }
            }
            if r.context.subject != SubjectSummary::from(&self.subject)
                || r.context.eeg_features.as_ref().map(|f| &f.values) != Some(&expected)
                || r.context.subject.subject_id != self.subject.id
                || r.context.signal_quality.as_ref() != Some(&a.processed_quality)
                || r.context.eeg_features.as_ref().map(|f| f.recording_id) != Some(a.processed.id)
            {
                return Err(ExportError::InvalidArchive);
            }
        }
        Ok(())
    }
}

/// 两个人类文档后端共享语义内容与样式级别，不共享二进制编码。
/// 正文不输出私有备注和文件路径；完整本地信息保留在 JSON / CSV 中。
#[derive(Debug, Clone)]
pub(crate) struct Paragraph {
    pub text: String,
    pub level: u8,
}
pub(crate) fn paragraphs(b: &ExportBundle) -> Result<Vec<Paragraph>, ExportError> {
    let r = b.report.as_ref().ok_or(ExportError::MissingReport)?;
    let a = &b.analysis;
    let mut out = Vec::new();
    let mut add = |level, text: String| out.push(Paragraph { text, level });
    add(2, "EEG 分析报告".into());
    add(
        0,
        "本报告描述既有 EEG 测量与工程质量结果，供分析复核使用。".into(),
    );
    add(1, "受试者与采集信息".into());
    add(
        0,
        format!(
            "受试者 ID：{}    年龄：{}    性别：{:?}",
            b.subject.id,
            b.subject.age.map_or("未填写".into(), |v| v.to_string()),
            b.subject.sex
        ),
    );
    add(
        0,
        format!(
            "报告生成时间：{}",
            r.context.integrated_assessment.assessed_at.to_rfc3339()
        ),
    );
    add(0, format!("原始录制 ID：{}", a.raw.id));
    add(0, format!("处理后录制 ID：{}", a.processed.id));
    add(
        0,
        format!(
            "导联序号：{}",
            a.processed
                .channels
                .iter()
                .enumerate()
                .map(|(i, c)| format!("通道 {} = {}", i + 1, c.label))
                .collect::<Vec<_>>()
                .join("；")
        ),
    );
    add(
        0,
        format!(
            "采样率：{} Hz -> {} Hz    时长：{} s    通道：{}",
            a.raw.sampling_rate_hz,
            a.processed.sampling_rate_hz,
            a.processed.duration_seconds,
            a.processed.channels.len()
        ),
    );
    add(
        0,
        format!(
            "采集时间：{}",
            a.raw
                .metadata
                .recorded_at
                .map_or("未填写".into(), |t| t.to_rfc3339())
        ),
    );
    add(
        0,
        format!(
            "设备：{}    电极系统：{}",
            a.raw.metadata.device.as_deref().unwrap_or("未填写"),
            a.raw
                .metadata
                .electrode_system
                .as_deref()
                .unwrap_or("未填写")
        ),
    );
    for (title, text) in [
        ("概要", &r.summary),
        ("指标描述", &r.description),
        ("解释边界", &r.interpretation),
    ] {
        add(1, title.into());
        add(0, text.clone());
    }
    add(1, "局限性".into());
    for text in &r.limitations {
        add(0, text.clone());
    }
    add(1, "信号质量".into());
    for (name, q) in [("原始", &a.raw_quality), ("处理后", &a.processed_quality)] {
        add(
            0,
            format!(
                "{name}工程质量评分：{:.3}%    坏导联：{}",
                q.overall_score.get() * 100.0,
                if q.bad_channels.is_empty() {
                    "无".into()
                } else {
                    q.bad_channels.join("、")
                }
            ),
        );
    }
    for (name, q) in [("原始", &a.raw_quality), ("处理后", &a.processed_quality)] {
        for (channel, quality) in &q.channel_quality {
            add(
                0,
                format!(
                    "{name} {channel}：评分 {:.3}%  缺失 {:.3}%  噪声 {:.3}%  工频 {:.3}%",
                    quality.score.get() * 100.0,
                    quality.missing_fraction.get() * 100.0,
                    quality.noisy_fraction.get() * 100.0,
                    quality.power_line_interference.get() * 100.0
                ),
            );
        }
        for warning in &q.warnings {
            add(0, format!("{name}质量提示：{}", warning.message));
        }
    }
    add(1, "频段功率".into());
    add(
        0,
        "绝对功率单位 uV²，相对功率为百分比。PSD 完整数值见 JSON 或 CSV。".into(),
    );
    if let Some(s) = &a.features.spectral {
        for (channel, bands) in &s.band_power_by_channel {
            for (band, p) in bands {
                add(
                    0,
                    format!(
                        "{channel}    {band:?}    {:.8} uV²    {:.5}%",
                        p.absolute,
                        p.relative.get() * 100.0
                    ),
                );
            }
        }
    }
    add(1, "处理与溯源".into());
    for step in &a.processed.processing_history {
        add(
            0,
            format!(
                "{} 版本 {}  {}",
                step.operation,
                step.algorithm_version,
                step.parameters
                    .get("configuration")
                    .cloned()
                    .unwrap_or_default()
            ),
        );
    }
    add(
        0,
        format!(
            "特征算法：{}    版本：{}",
            a.features.provenance.algorithm, a.features.provenance.algorithm_version
        ),
    );
    add(
        0,
        format!(
            "报告 Provider：{}    归档格式：{}",
            r.provider, b.schema_version
        ),
    );
    // OOXML/PDF 不能可靠表示 NUL 等控制字符。明确失败，不能静默删字或输出乱码。
    if out.iter().any(|p| {
        p.text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    }) {
        return Err(ExportError::InvalidText);
    }
    Ok(out)
}
