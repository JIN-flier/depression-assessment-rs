//! P9 Provider 边界：只接收已脱敏的结构化事实，返回尚未信任的 Narrative JSON。
//!
//! 本模块不计算 EEG、风险或可信度，也不承担输出校验。SDK 和 Tokio 仅存在于
//! `openai` 可选适配器；上层、离线测试和其他 Provider 都只依赖此处的 trait。
#![forbid(unsafe_code)]

#[cfg(feature = "openai")]
mod openai;
mod prompt;
#[cfg(feature = "openai")]
pub use openai::{OpenAiConfig, OpenAiReportNarrator};
pub use prompt::{CONNECTIVES, SYSTEM_PROMPT, narrative_schema, user_prompt};

use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, error::Error, fmt};

/// 没有 Subject ID、Recording ID、原始矩阵、文件路径或任意 metadata 的 API DTO。
/// display 是确定性格式化的事实；LLM 必须通过占位符引用它，不能重算或改数字。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NarrationInput {
    pub facts: BTreeMap<String, String>,
    pub interpretation: String,
    pub limitations: Vec<String>,
}

/// 返回值始终是“不可信候选”。只有 report crate 校验后才能展示。
///
/// 此同步入口专供应用的后台 worker；OpenAI 实现内部运行异步网络请求，另提供
/// narrate_async 供已有 Tokio 宿主使用。不要在 UI 线程或 Tokio task 中调用同步入口。
/// 替换 Provider 不影响 EEG 算法、报告校验器和 UI。
pub trait ReportNarrator: Send {
    fn narrate(&self, input: &NarrationInput) -> Result<String, LlmError>;
    /// 稳定的 Provider 名称，不包含 API Key、URL 或服务端返回文本。
    fn provider_name(&self) -> &'static str;
}

/// 仅保存可展示的类别，不保存可能回显 Key、Prompt、身份信息的 SDK 错误正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmError {
    NotConfigured,
    Configuration,
    Request,
    Timeout,
    Refused,
    Incomplete,
    EmptyResponse,
    InputTooLarge,
}
impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotConfigured => {
                "未配置 LLM；请按 crates/llm/README.md 添加依赖并启用 openai 功能"
            }
            Self::Configuration => "LLM 配置无效，请检查环境变量",
            Self::Request => "LLM 请求失败，请检查网络、模型权限及 API 配置后重试",
            Self::Timeout => "LLM 请求超时，可重试",
            Self::Refused => "LLM 拒绝生成报告",
            Self::Incomplete => "LLM 输出被截断或未正常完成",
            Self::EmptyResponse => "LLM 未返回报告内容",
            Self::InputTooLarge => "结构化报告输入超过长度限制",
        })
    }
}
impl Error for LlmError {}

/// 默认构建保持 EEG 功能可用；不把离线模板冒充为真实 LLM 结果。
pub struct UnconfiguredNarrator;
impl ReportNarrator for UnconfiguredNarrator {
    fn narrate(&self, _: &NarrationInput) -> Result<String, LlmError> {
        Err(LlmError::NotConfigured)
    }
    fn provider_name(&self) -> &'static str {
        "unconfigured"
    }
}
