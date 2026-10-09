//! P9–P10 报告编排与导出：ReportContext → 白名单事实 → Provider → 独立校验 → ReportDocument。
//!
//! 此 crate 不依赖 EEG 算法、Slint、数据库、OpenAI SDK 或 Tokio。输入包含计算完成的
//! 结果；输出仅在成功校验后构造。P10 消费同一个文档及既有结构化分析进行四格式导出。
#![forbid(unsafe_code)]
mod context;
pub mod export;
mod validation;
pub use context::{V1_INTERPRETATION, prepare_v1};
pub use validation::{DraftNarrative, NarrativeSection, ValidatedNarrative, validate_narrative};

use domain::ReportContext;
use llm::ReportNarrator;
use std::{error::Error, fmt};

/// 完整上下文留在本地，用于审计和后续导出；API 仅收到 prepare_v1 的投影。
/// narrative 私有，调用者无法将尚未校验的 Provider 响应作为文档发布。
#[derive(Debug, Clone)]
pub struct ReportDocument {
    context: ReportContext,
    narrative: ValidatedNarrative,
    provider: &'static str,
}
impl ReportDocument {
    pub fn context(&self) -> &ReportContext {
        &self.context
    }
    pub fn narrative(&self) -> &ValidatedNarrative {
        &self.narrative
    }
    pub fn provider(&self) -> &'static str {
        self.provider
    }
    /// 纯文字投影，只有校验后的内容可以进入 UI；不执行算法或文件 IO。
    pub fn plain_text(&self) -> String {
        format!(
            "EEG 分析报告\n\n概要\n{}\n\n指标描述\n{}\n\n解释边界\n{}\n\n局限性\n{}",
            self.narrative.summary(),
            self.narrative.description(),
            self.narrative.interpretation(),
            self.narrative.limitations().join("\n")
        )
    }
}

/// 不缓存 Prompt、不在失败时回退到未校验文本，也不修改原始结构化结果。
/// 可注入任意实现 ReportNarrator 的 Provider；应用无需知道 SDK 类型。
pub struct ReportService {
    narrator: Box<dyn ReportNarrator>,
}
impl ReportService {
    pub fn new(narrator: Box<dyn ReportNarrator>) -> Self {
        Self { narrator }
    }
    pub fn generate(&self, context: ReportContext) -> Result<ReportDocument, ReportError> {
        let input = prepare_v1(&context)?;
        let response = self
            .narrator
            .narrate(&input)
            .map_err(ReportError::Provider)?;
        let narrative = validate_narrative(&input, &response)?;
        Ok(ReportDocument {
            context,
            narrative,
            provider: self.narrator.provider_name(),
        })
    }
}

/// 校验错误不包含原始响应、字段内容或敏感标识，适合直接显示在 UI。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportError {
    InvalidContext,
    UnsupportedAssessment,
    InvalidJson,
    InvalidNarrative,
    UnknownFact,
    UnreferencedFact,
    ChangedInterpretation,
    ChangedLimitations,
    UnsafeWording,
    TooLarge,
    Provider(llm::LlmError),
}
impl fmt::Display for ReportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidContext => "报告上下文缺失、不一致或包含无效指标",
            Self::UnsupportedAssessment => "P9 仅支持 V1 EEG 描述，不支持临床结论或多模态评估",
            Self::InvalidJson => "LLM 报告不是有效的 Narrative JSON",
            Self::InvalidNarrative => "LLM 报告段落为空或结构无效",
            Self::UnknownFact => "LLM 报告引用了输入中不存在的事实",
            Self::UnreferencedFact => "LLM 报告的事实声明与正文引用不一致",
            Self::ChangedInterpretation => "LLM 改写了系统确定的解释边界",
            Self::ChangedLimitations => "LLM 改写或遗漏了系统确定的局限性",
            Self::UnsafeWording => "LLM 报告包含未经允许的表述或自行填写的数字",
            Self::TooLarge => "LLM 报告输入或输出超过长度限制",
            Self::Provider(error) => return error.fmt(f),
        })
    }
}
impl Error for ReportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        if let Self::Provider(error) = self {
            Some(error)
        } else {
            None
        }
    }
}
