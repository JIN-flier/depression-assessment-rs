//! 独立、fail-closed 的 V1 Narrative Validator。
//!
//! 任意自然语言的临床含义无法靠关键词完整验证，因此 P9 明确采用受控连接语：
//! LLM 可以安排事实引用与段落，不能自由添加判断；所有测量文字来自本地事实。
//! 这比仅禁止几个疾病关键词更严格。后续若开放自由表达，应替换独立 validator，
//! 不能删除这层边界后直接展示 Provider 响应。
use crate::ReportError;
use llm::{CONNECTIVES, NarrationInput};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// 未校验的传输模型，与可展示模型分开，禁止通过类型混用跳过校验。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NarrativeSection {
    pub text: String,
    pub fact_ids: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftNarrative {
    pub summary: NarrativeSection,
    pub description: NarrativeSection,
    pub interpretation: String,
    pub limitations: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedNarrative {
    summary: String,
    description: String,
    interpretation: String,
    limitations: Vec<String>,
}
impl ValidatedNarrative {
    pub fn summary(&self) -> &str {
        &self.summary
    }
    pub fn description(&self) -> &str {
        &self.description
    }
    pub fn interpretation(&self) -> &str {
        &self.interpretation
    }
    pub fn limitations(&self) -> &[String] {
        &self.limitations
    }
}

pub fn validate_narrative(
    input: &NarrationInput,
    response: &str,
) -> Result<ValidatedNarrative, ReportError> {
    if response.len() > 64 * 1024 {
        return Err(ReportError::TooLarge);
    }
    // 不从 Markdown 或多段响应中猜测 JSON；格式错误明确失败，允许调用方重试。
    let draft: DraftNarrative =
        serde_json::from_str(response).map_err(|_| ReportError::InvalidJson)?;
    if draft.interpretation != input.interpretation {
        return Err(ReportError::ChangedInterpretation);
    }
    if draft.limitations != input.limitations {
        return Err(ReportError::ChangedLimitations);
    }
    let summary = render_section(input, &draft.summary)?;
    let description = render_section(input, &draft.description)?;
    Ok(ValidatedNarrative {
        summary,
        description,
        interpretation: draft.interpretation,
        limitations: draft.limitations,
    })
}

fn render_section(
    input: &NarrationInput,
    section: &NarrativeSection,
) -> Result<String, ReportError> {
    if section.text.trim().is_empty() || section.fact_ids.is_empty() {
        return Err(ReportError::InvalidNarrative);
    }
    if section.text.len() > 16 * 1024 || section.fact_ids.len() > 1024 {
        return Err(ReportError::TooLarge);
    }
    let declared: BTreeSet<&str> = section.fact_ids.iter().map(String::as_str).collect();
    if declared.len() != section.fact_ids.len() {
        return Err(ReportError::UnreferencedFact);
    }
    if declared.iter().any(|id| !input.facts.contains_key(*id)) {
        return Err(ReportError::UnknownFact);
    }
    let mut used = BTreeSet::new();
    let mut rest = section.text.as_str();
    let mut rendered = String::new();
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("{{") {
            let (id, tail) = after
                .split_once("}}")
                .ok_or(ReportError::InvalidNarrative)?;
            let fact = input.facts.get(id).ok_or(ReportError::UnknownFact)?;
            used.insert(id);
            rendered.push_str(fact);
            rest = tail;
        } else {
            let connective = CONNECTIVES
                .iter()
                .find(|text| rest.starts_with(**text))
                .ok_or(ReportError::UnsafeWording)?;
            rendered.push_str(connective);
            rest = &rest[connective.len()..];
        }
    }
    if used != declared {
        return Err(ReportError::UnreferencedFact);
    }
    if rendered.len() > 64 * 1024 {
        return Err(ReportError::TooLarge);
    }
    Ok(rendered)
}
