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
use std::{collections::BTreeMap, fmt, sync::Mutex, time::Duration};

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
    /// 用 dotenvy 从当前目录或父目录读取 .env，进程环境变量优先。
    /// 不修改全局环境，因此后台 worker 可安全读取配置；解析错误不保留文件内容。
    pub fn from_env() -> Result<Self, LlmError> {
        let mut dotenv = BTreeMap::new();
        match dotenvy::dotenv_iter() {
            Ok(values) => {
                for value in values {
                    let (key, value) = value.map_err(|_| LlmError::Configuration)?;
                    // 与 dotenvy::dotenv 一致，重复变量使用首次声明的值。
                    dotenv.entry(key).or_insert(value);
                }
            }
            Err(error) if error.not_found() => {}
            Err(_) => return Err(LlmError::Configuration),
        }
        let value = |names: &[&str]| {
            names
                .iter()
                .find_map(|name| std::env::var(name).ok())
                .or_else(|| names.iter().find_map(|name| dotenv.get(*name).cloned()))
        };
        let mut config = Self::new(
            value(&["OPENAI_API_KEY"]).ok_or(LlmError::NotConfigured)?,
            value(&["EEG_LLM_MODEL"]).ok_or(LlmError::NotConfigured)?,
        )?;
        if let Some(base) = value(&["OPENAI_BASE_URL", "OPENAI_API_BASE"]) {
            config = config.with_api_base(base)?;
        }
        if let Some(seconds) = value(&["EEG_LLM_TIMEOUT_SECONDS"]) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, process::Command};

    const VARIABLES: [&str; 5] = [
        "OPENAI_API_KEY",
        "EEG_LLM_MODEL",
        "OPENAI_BASE_URL",
        "OPENAI_API_BASE",
        "EEG_LLM_TIMEOUT_SECONDS",
    ];

    #[test]
    fn dotenv_configuration() {
        // 每种配置在独立子进程中运行，不修改并行测试的环境或读取开发者的真实 .env。
        if let Ok(case) = std::env::var("LLM_DOTENV_TEST_CASE") {
            let before = VARIABLES.map(std::env::var_os);
            let result = OpenAiConfig::from_env();
            assert_eq!(VARIABLES.map(std::env::var_os), before);
            match case.as_str() {
                "file" | "legacy" => {
                    let config = result.unwrap();
                    assert_eq!(config.api_key, "file-key");
                    assert_eq!(config.model, "file-model");
                    assert_eq!(config.api_base.as_deref(), Some("https://file.example/v1"));
                    assert_eq!(config.timeout, Duration::from_secs(42));
                }
                "override" | "env-only" => {
                    let config = result.unwrap();
                    assert_eq!(config.api_key, "env-key");
                    assert_eq!(config.model, "env-model");
                    assert_eq!(config.api_base.as_deref(), Some("https://env.example/v1"));
                    assert_eq!(config.timeout, Duration::from_secs(17));
                }
                "missing" | "missing-model" => {
                    assert_eq!(result.unwrap_err(), LlmError::NotConfigured);
                }
                "malformed" | "invalid-timeout" | "blank-key" => {
                    assert_eq!(result.unwrap_err(), LlmError::Configuration);
                }
                _ => panic!("unknown test case"),
            }
            return;
        }

        let directory = std::env::temp_dir().join(format!("llm-dotenv-{}", std::process::id()));
        let file = "OPENAI_API_KEY='file-key'\nEEG_LLM_MODEL=\"file-model\"\n\
            OPENAI_BASE_URL=https://file.example/v1/\n\
            OPENAI_API_BASE=https://legacy.example/v1\nEEG_LLM_TIMEOUT_SECONDS=42\n\
            OPENAI_API_KEY=ignored-duplicate\n";
        let cases = [
            ("file", Some(file.to_owned())),
            ("override", Some(file.to_owned())),
            (
                "legacy",
                Some("OPENAI_API_KEY=file-key\nEEG_LLM_MODEL=file-model\nOPENAI_API_BASE=https://file.example/v1\nEEG_LLM_TIMEOUT_SECONDS=42\n".to_owned()),
            ),
            ("env-only", None),
            ("missing", None),
            ("missing-model", Some("OPENAI_API_KEY=file-key\n".to_owned())),
            ("malformed", Some("OPENAI_API_KEY='secret-with-unclosed-quote\n".to_owned())),
            ("invalid-timeout", Some(file.replace("SECONDS=42", "SECONDS=0"))),
            ("blank-key", Some(file.replace("'file-key'", "''"))),
        ];
        for (case, contents) in cases {
            let root = directory.join(case);
            let working = root.join("nested");
            fs::create_dir_all(&working).unwrap();
            if let Some(contents) = contents {
                fs::write(root.join(".env"), contents).unwrap();
            }
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "openai::tests::dotenv_configuration",
                    "--nocapture",
                ])
                .current_dir(working)
                .env("LLM_DOTENV_TEST_CASE", case);
            for name in VARIABLES {
                command.env_remove(name);
            }
            if matches!(case, "override" | "env-only") {
                command
                    .env("OPENAI_API_KEY", "env-key")
                    .env("EEG_LLM_MODEL", "env-model")
                    .env("OPENAI_API_BASE", "https://env.example/v1")
                    .env("EEG_LLM_TIMEOUT_SECONDS", "17");
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{case}: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        fs::remove_dir_all(directory).unwrap();
    }
}
