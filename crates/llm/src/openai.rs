//! async-openai 0.42.1 适配器。SDK 类型仅在本模块出现，上层只看到 ReportNarrator。
use crate::{
    LlmError, NarrationInput, ReportNarrator, SYSTEM_PROMPT, narrative_schema, user_prompt,
};
use async_openai::{
    Client,
    config::OpenAIConfig,
    types::chat::{
        ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestUserMessageArgs,
        CreateChatCompletionRequestArgs, FinishReason, ResponseFormat, ResponseFormatJsonSchema,
    },
};
use std::{fmt, sync::Mutex, time::Duration};

/// Key 只来自运行时配置，不放入 AppCommand/AppSnapshot、数据库、Debug 或日志。
/// 模型需支持 json_schema；不隐式换模型或降级到无法约束形状的普通文本。
pub struct OpenAiConfig {
    api_key: String,
    model: String,
    api_base: Option<String>,
    timeout: Duration,
    max_completion_tokens: u32,
}
impl fmt::Debug for OpenAiConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAiConfig")
            .field("api_key", &"[REDACTED]")
            .field("model", &"[CONFIGURED]")
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}
impl OpenAiConfig {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Result<Self, LlmError> {
        let config = Self {
            api_key: api_key.into(),
            model: model.into(),
            api_base: None,
            timeout: Duration::from_secs(60),
            max_completion_tokens: 4096,
        };
        if config.api_key.trim().is_empty() || config.model.trim().is_empty() {
            return Err(LlmError::Configuration);
        }
        Ok(config)
    }
    pub fn from_env() -> Result<Self, LlmError> {
        let mut config = Self::new(
            std::env::var("OPENAI_API_KEY").map_err(|_| LlmError::NotConfigured)?,
            std::env::var("EEG_LLM_MODEL").map_err(|_| LlmError::NotConfigured)?,
        )?;
        if let Ok(base) = std::env::var("OPENAI_API_BASE") {
            config = config.with_api_base(base)?;
        }
        if let Ok(seconds) = std::env::var("EEG_LLM_TIMEOUT_SECONDS") {
            config = config.with_timeout(Duration::from_secs(
                seconds.parse().map_err(|_| LlmError::Configuration)?,
            ))?;
        }
        Ok(config)
    }
    /// 允许 HTTPS 兼容服务和 localhost 测试服务；禁止把 Key 发送至明文远端。
    pub fn with_api_base(mut self, base: impl Into<String>) -> Result<Self, LlmError> {
        let base = base.into();
        let authority = base
            .split_once("://")
            .map(|(_, rest)| rest.split('/').next().unwrap_or(""));
        let local = authority.is_some_and(|host| {
            host == "localhost"
                || host.starts_with("localhost:")
                || host == "127.0.0.1"
                || host.starts_with("127.0.0.1:")
        });
        if !(base.starts_with("https://") || (base.starts_with("http://") && local))
            || authority.is_none_or(|a| a.is_empty() || a.contains('@'))
            || base.contains('?')
            || base.contains('#')
            || base.chars().any(char::is_whitespace)
        {
            return Err(LlmError::Configuration);
        }
        self.api_base = Some(base.trim_end_matches('/').into());
        Ok(self)
    }
    pub fn with_timeout(mut self, timeout: Duration) -> Result<Self, LlmError> {
        if timeout.is_zero() || timeout > Duration::from_secs(300) {
            return Err(LlmError::Configuration);
        }
        self.timeout = timeout;
        Ok(self)
    }
}

/// 同步 Runtime 首次使用时才创建并复用，纯异步宿主不会创建/销毁嵌套 Runtime。
/// 同步入口仅在 P8 的 eeg-workflow worker 上阻塞，
/// UI 继续消费进度事件。超时覆盖整次请求；失败由用户显式重试，避免重复计费。
pub struct OpenAiReportNarrator {
    client: Client<OpenAIConfig>,
    model: String,
    timeout: Duration,
    max_completion_tokens: u32,
    runtime: Mutex<Option<tokio::runtime::Runtime>>,
}
impl OpenAiReportNarrator {
    pub fn new(config: OpenAiConfig) -> Result<Self, LlmError> {
        let mut sdk = OpenAIConfig::new().with_api_key(config.api_key);
        if let Some(base) = config.api_base {
            sdk = sdk.with_api_base(base);
        }
        Ok(Self {
            client: Client::with_config(sdk),
            model: config.model,
            timeout: config.timeout,
            max_completion_tokens: config.max_completion_tokens,
            runtime: Mutex::new(None),
        })
    }
    /// 已有 Tokio runtime 的宿主可直接 await；仍需交给 report validator 后再显示。
    pub async fn narrate_async(&self, input: &NarrationInput) -> Result<String, LlmError> {
        let request = CreateChatCompletionRequestArgs::default()
            .model(&self.model)
            .store(false)
            .max_completion_tokens(self.max_completion_tokens)
            .messages(vec![
                ChatCompletionRequestSystemMessageArgs::default()
                    .content(SYSTEM_PROMPT)
                    .build()
                    .map_err(|_| LlmError::Configuration)?
                    .into(),
                ChatCompletionRequestUserMessageArgs::default()
                    .content(user_prompt(input)?)
                    .build()
                    .map_err(|_| LlmError::Configuration)?
                    .into(),
            ])
            .response_format(ResponseFormat::JsonSchema {
                json_schema: ResponseFormatJsonSchema {
                    name: "eeg_v1_narrative".into(),
                    description: None,
                    schema: narrative_schema(),
                    strict: Some(true),
                },
            })
            .build()
            .map_err(|_| LlmError::Configuration)?;
        let response = tokio::time::timeout(self.timeout, self.client.chat().create(request))
            .await
            .map_err(|_| LlmError::Timeout)?
            .map_err(|_| LlmError::Request)?;
        if response.choices.len() != 1 {
            return Err(LlmError::EmptyResponse);
        }
        let choice = &response.choices[0];
        if choice.message.refusal.is_some() {
            return Err(LlmError::Refused);
        }
        if choice.finish_reason != Some(FinishReason::Stop) {
            return Err(LlmError::Incomplete);
        }
        let content = choice
            .message
            .content
            .as_deref()
            .filter(|text| !text.trim().is_empty())
            .ok_or(LlmError::EmptyResponse)?;
        if content.len() > 64 * 1024 {
            return Err(LlmError::Incomplete);
        }
        Ok(content.into())
    }
}
impl ReportNarrator for OpenAiReportNarrator {
    fn narrate(&self, input: &NarrationInput) -> Result<String, LlmError> {
        // 明确失败，避免在现有异步 runtime 中嵌套 block_on 导致 panic。
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(LlmError::Configuration);
        }
        let mut runtime = self.runtime.lock().map_err(|_| LlmError::Configuration)?;
        if runtime.is_none() {
            *runtime = Some(
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|_| LlmError::Configuration)?,
            );
        }
        runtime
            .as_ref()
            .ok_or(LlmError::Configuration)?
            .block_on(self.narrate_async(input))
    }
    fn provider_name(&self) -> &'static str {
        "openai"
    }
}
