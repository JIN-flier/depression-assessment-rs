//! P8→P9 适配层：将会话中的既有质量/功率结果转成 Domain ReportContext。
//! Provider 不接触 AnalysisResult 或样本矩阵；不在这里重跑 DSP、PSD 或风险计算。
use crate::{AnalysisResult, AppError, AppResult};
use domain::{
    AssessmentId, EegFeatureSummary, EvidenceSet, IntegratedAssessment, Provenance, RecordingId,
    ReliabilityAssessment, ReliabilityLevel, ReportContext, Severity, Subject, SubjectSummary,
};
use std::{collections::BTreeMap, time::SystemTime};

/// 校验实体关联后才构造报告，防止把另一受试者或另一次分析的事实写入同一文档。
pub fn build_v1_report_context(
    subject: &Subject,
    raw_id: RecordingId,
    analysis: &AnalysisResult,
) -> AppResult<ReportContext> {
    let processed = &analysis.processed;
    if processed.subject_id != subject.id
        || analysis.raw_quality.recording_id != raw_id
        || analysis.processed_quality.recording_id != processed.id
        || analysis.features.recording_id != processed.id
    {
        return Err(AppError::InvalidState("报告输入的受试者或录制关联不一致"));
    }
    let spectral = analysis
        .features
        .spectral
        .as_ref()
        .ok_or(AppError::InvalidState("请先提取频段功率"))?;
    if spectral.band_power_by_channel.is_empty() {
        return Err(AppError::InvalidState("没有可供报告描述的频段功率"));
    }
    let now = SystemTime::now().into();
    // V1 不执行融合/可信度/安全引擎：显式 Unknown、无 risk、conclusion_allowed=false。
    // Low 只是领域模型的保守占位，不对外宣称已经评估了“临床可信度”。
    let mut reliability = ReliabilityAssessment::new(ReliabilityLevel::Low);
    reliability.reasons.push("V1 尚未进行综合可信度评估".into());
    let assessment = IntegratedAssessment::new(
        AssessmentId::new(),
        subject.id,
        None,
        Severity::Unknown,
        reliability,
        EvidenceSet::default(),
        now,
        Provenance::new(env!("CARGO_PKG_VERSION"), "v1-report-context", "1", now)
            .map_err(|e| AppError::service("报告溯源", e))?,
    );
    let mut context = ReportContext::new(SubjectSummary::from(subject), assessment)
        .map_err(|e| AppError::service("报告上下文", e))?;
    let mut values = BTreeMap::from([
        (
            "raw_quality_score".into(),
            analysis.raw_quality.overall_score.get(),
        ),
        (
            "processed_quality_score".into(),
            analysis.processed_quality.overall_score.get(),
        ),
        (
            "raw_bad_channel_count".into(),
            analysis.raw_quality.bad_channels.len() as f64,
        ),
        (
            "processed_bad_channel_count".into(),
            analysis.processed_quality.bad_channels.len() as f64,
        ),
        ("sampling_rate_hz".into(), processed.sampling_rate_hz),
        ("duration_seconds".into(), processed.duration_seconds),
    ]);
    // 按原始导联顺序编号，排除的通道不重新编号。上传使用序号；真实标签只保留在
    // 本地 context.metadata，便于用户与分析页对照，未知/恶意 CSV 标签也不会外发。
    for (index, channel) in processed.channels.iter().enumerate() {
        if let Some(bands) = spectral.band_power_by_channel.get(&channel.label) {
            context
                .metadata
                .insert(format!("channel_{}", index + 1), channel.label.clone());
            for (band, power) in bands {
                let band = match band {
                    domain::FrequencyBand::Delta => "delta",
                    domain::FrequencyBand::Theta => "theta",
                    domain::FrequencyBand::Alpha => "alpha",
                    domain::FrequencyBand::Beta => "beta",
                    domain::FrequencyBand::Gamma => "gamma",
                };
                values.insert(
                    format!("channel_{}.{band}.absolute", index + 1),
                    power.absolute,
                );
                values.insert(
                    format!("channel_{}.{band}.relative", index + 1),
                    power.relative.get(),
                );
            }
        }
    }
    // 若存在找不到导联元数据的频段结果，拒绝悄悄丢掉它们。
    if spectral
        .band_power_by_channel
        .keys()
        .any(|label| !processed.channels.iter().any(|c| &c.label == label))
    {
        return Err(AppError::InvalidState("频段结果包含未知导联"));
    }
    let mut notes = Vec::new();
    if analysis.request.exclude_bad_channels {
        notes.push("按原始质量结果配置了坏导联排除".into());
    }
    if !analysis.raw_quality.warnings.is_empty() || !analysis.raw_quality.bad_channels.is_empty() {
        notes.push("原始信号质量存在警告或坏导联".into());
    }
    context.eeg_features = Some(EegFeatureSummary {
        recording_id: processed.id,
        values,
        notes,
    });
    context.signal_quality = Some(analysis.processed_quality.clone());
    // 原始质量、算法版本与配置留在本地，P10 可导出审计信息，不放入 API DTO。
    context
        .metadata
        .insert("source_recording_id".into(), raw_id.to_string());
    context
        .metadata
        .insert("analysis_request".into(), format!("{:?}", analysis.request));
    context.metadata.insert(
        "feature_provenance".into(),
        format!("{:?}", analysis.features.provenance),
    );
    context
        .metadata
        .insert("raw_quality".into(), format!("{:?}", analysis.raw_quality));
    Ok(context)
}

/// 仅在用户点击生成报告的后台任务中读取 .env/环境变量并创建网络适配器。
/// 启动 EEG 工作台不需要 Key，也不会自动发送请求。
pub(crate) fn configured_report_service() -> AppResult<report::ReportService> {
    #[cfg(feature = "openai")]
    {
        let config = llm::OpenAiConfig::from_env().map_err(|e| AppError::service("LLM 配置", e))?;
        let narrator = llm::OpenAiReportNarrator::new(config)
            .map_err(|e| AppError::service("LLM 初始化", e))?;
        Ok(report::ReportService::new(Box::new(narrator)))
    }
    #[cfg(not(feature = "openai"))]
    {
        Ok(report::ReportService::new(Box::new(
            llm::UnconfiguredNarrator,
        )))
    }
}
