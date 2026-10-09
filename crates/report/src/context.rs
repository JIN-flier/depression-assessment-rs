//! 对已有结果做确定性投影，绝不从原始矩阵重算任何 EEG 指标。
use crate::ReportError;
use domain::{ReportContext, Severity};
use llm::NarrationInput;
use std::collections::BTreeMap;

pub const V1_INTERPRETATION: &str = "本报告仅描述 EEG 信号质量和频段功率，不据此判断是否抑郁，也不提供严重度、临床风险、综合可信度或治疗结论。";

/// V1 的解释边界是产品能力限制，不是 P18/P19/P20 算法生成的临床结论。
/// 可变 metadata、备注、provenance 参数和自由文本 warnings 一律不进入 API。
/// 固定限制语由本地规则构造，LLM 不得删除、改写或额外生成建议。
pub fn prepare_v1(context: &ReportContext) -> Result<NarrationInput, ReportError> {
    let assessment = &context.integrated_assessment;
    if context.subject.subject_id != assessment.subject_id {
        return Err(ReportError::InvalidContext);
    }
    if assessment.severity != Severity::Unknown
        || assessment.conclusion_allowed
        || assessment.risk.is_some()
        || !context.scale_results.is_empty()
        || !context.model_results.is_empty()
        || !assessment.evidence.is_empty()
    {
        return Err(ReportError::UnsupportedAssessment);
    }
    let quality = context
        .signal_quality
        .as_ref()
        .ok_or(ReportError::InvalidContext)?;
    let features = context
        .eeg_features
        .as_ref()
        .ok_or(ReportError::InvalidContext)?;
    if quality.recording_id != features.recording_id || features.values.is_empty() {
        return Err(ReportError::InvalidContext);
    }
    let mut facts = BTreeMap::new();
    for (id, value) in &features.values {
        // 公共 ReportContext 的 values 是开放字典；进入 API 时收窄到 V1 的已知键。
        // 防止自定义键名中混入身份信息或 Prompt 指令。
        let (caption, unit, fraction) = caption(id).ok_or(ReportError::InvalidContext)?;
        if !value.is_finite() || *value < 0.0 || (fraction && *value > 1.0) {
            return Err(ReportError::InvalidContext);
        }
        let display = if fraction {
            format!("{caption}：{:.3}%", value * 100.0)
        } else {
            format!("{caption}：{value:.6}{unit}")
        };
        facts.insert(id.clone(), display);
    }
    // 处理后质量必须与摘要一致；防止使用原始质量给处理后特征配上错误标签。
    if features.values.get("processed_quality_score") != Some(&quality.overall_score.get()) {
        return Err(ReportError::InvalidContext);
    }
    let mut limitations = vec![
        "EEG 频段功率不是抑郁诊断指标，本阶段未进行量表、模型推理或多模态融合。".into(),
        "质量评分采用工程启发式规则，不能等同于临床可靠性或诊断可信度。".into(),
        "相对功率的分母取自本次特征配置，各频段之和未必等于百分之百。".into(),
        "通道以匿名序号表示；原始导联名和处理参数请查看本地分析结果。".into(),
    ];
    if !quality.bad_channels.is_empty()
        || !quality.warnings.is_empty()
        || quality
            .channel_quality
            .values()
            .any(|q| !q.warnings.is_empty())
    {
        limitations.push("质量评价包含坏导联或警告，解释指标前应复核本地质量结果。".into());
    }
    // 对任意上游自由文本只报告其存在，不外发原文。完整原文仍在本地文档上下文。
    if !context.limitations.is_empty()
        || !assessment.limitations.is_empty()
        || !assessment.warnings.is_empty()
        || !features.notes.is_empty()
    {
        limitations.push("本地分析还包含附加说明或局限性，请结合分析结果查看。".into());
    }
    let input = NarrationInput {
        facts,
        interpretation: V1_INTERPRETATION.into(),
        limitations,
    };
    llm::user_prompt(&input).map_err(|_| ReportError::TooLarge)?;
    Ok(input)
}

/// 接受少量固定记录事实和匿名 channel_<正整数>.<band>.<absolute|relative>。
/// 导联名称无论是否看起来“正常”都不发出；CSV 标签本身可能是身份或注入文本。
fn caption(id: &str) -> Option<(String, &'static str, bool)> {
    let fixed = match id {
        "raw_quality_score" => Some(("原始信号质量评分", "", true)),
        "processed_quality_score" => Some(("处理后信号质量评分", "", true)),
        "raw_bad_channel_count" => Some(("原始信号坏导联数", " 个", false)),
        "processed_bad_channel_count" => Some(("处理后信号坏导联数", " 个", false)),
        "sampling_rate_hz" => Some(("处理后采样率", " Hz", false)),
        "duration_seconds" => Some(("处理后记录时长", " s", false)),
        _ => None,
    };
    if let Some((label, unit, fraction)) = fixed {
        return Some((label.into(), unit, fraction));
    }
    let mut parts = id.split('.');
    let channel = parts.next()?.strip_prefix("channel_")?;
    let index: usize = channel.parse().ok()?;
    if index == 0 || index > 4096 || channel != index.to_string() {
        return None;
    }
    let band = match parts.next()? {
        "delta" => "Delta",
        "theta" => "Theta",
        "alpha" => "Alpha",
        "beta" => "Beta",
        "gamma" => "Gamma",
        _ => return None,
    };
    let (kind, unit, fraction) = match parts.next()? {
        "absolute" => ("绝对功率", " uV²", false),
        "relative" => ("相对功率", "", true),
        _ => return None,
    };
    if parts.next().is_some() {
        return None;
    }
    Some((format!("通道 {index} 的 {band} {kind}"), unit, fraction))
}
