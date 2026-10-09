# P9 LLM Provider

`ReportNarrator` 是独立 Provider 接口。上层只提交 `NarrationInput`，返回值是尚未信任的 Narrative JSON；必须经过 `report` 的校验才能展示。这里没有 EEG、量表、模型、风险或可信度计算。

## 可选依赖与功能开关

`serde` / `serde_json` 复用了工程已有的版本。SDK 依赖已在 `crates/llm/Cargo.toml` 中声明为可选依赖，由 `openai` 功能同时启用依赖与适配器。默认构建能运行全部 EEG 功能与离线报告测试；点击报告生成会明确提示未配置 LLM。

当前 **crates/llm/Cargo.toml 的 `[dependencies]`** 配置为：

```toml
async-openai = { version = "=0.42.1", features = ["chat-completion"], optional = true }
tokio = { version = "1.50", features = ["rt", "time", "net"], optional = true }
```

同一文件的 `[features]` 将可选依赖关联到功能开关：

```toml
openai = ["dep:async-openai", "dep:tokio"]
```

只下载包或运行 `cargo test --workspace` 不会自动启用此功能；测试和运行时需要显式传入 `--features openai`。其他两个 crate 已接好 feature 转发，不需要额外直接依赖 SDK：

```text
cargo test -p llm --features openai
cargo test -p eeg-application --features openai
cargo run -p depression-desktop --features openai
```

SDK 的 `chat-completion` feature 与 `types::chat` 路径基于 [async-openai 0.42.1 文档](https://docs.rs/async-openai/0.42.1/async_openai/types/chat/index.html)。请求使用严格 `json_schema`，不是普通文本响应。

## 运行配置

从启动桌面程序的环境传入：

| 环境变量 | 要求 |
|---|---|
| `OPENAI_API_KEY` | 必填；仅内存使用，不写入项目、命令、快照或错误文本 |
| `EEG_LLM_MODEL` | 必填；选择有权限且支持严格 JSON Schema 的模型，不硬编码模型名 |
| `OPENAI_API_BASE` | 可选；默认 OpenAI；自定义使用 HTTPS，测试允许 localhost / 127.0.0.1 HTTP |
| `EEG_LLM_TIMEOUT_SECONDS` | 可选；默认 60 秒，有效范围 1–300 秒 |

打开项目 → 导入 EEG → 完成分析 → 报告 → 生成报告。只有点击生成才创建适配器并发送请求。开启 feature 后也不影响未配置 Key 的 EEG 工作流；配置缺失时报告生成明确失败。修改环境变量通常需要重新启动程序。

不自动降级 Provider、不自动重试、不自动换模型。超时覆盖整次异步请求；拒绝、截断、空内容、HTTP/SDK 失败分别返回类型化错误，用户可以显式重试。服务端错误正文可能回显请求或 Key，因此错误映射不保留原文。`store=false` 是请求选项，不是对所有兼容服务的数据保留承诺。

## 异步与后台线程

同步 `ReportNarrator::narrate` 是应用 worker 的边界。`OpenAiReportNarrator` 复用私有 Tokio runtime，内部使用 async-openai 异步请求；不会在 Slint 线程等待。已有 Tokio 宿主用 `narrate_async().await`；同步入口检测嵌套 runtime 并返回配置错误。

API 输入只含匿名事实、固定解释范围、固定局限性。Subject/Recording ID、年龄性别、备注、设备、路径、原始导联标签、样本、PSD 数组、任意 metadata 和 warning 原文均不上传。

V1 使用事实占位符和受控连接语组织中文段落。系统原文的解释范围与局限性逐字保留，事实值在校验后本地填入。`CONNECTIVES` 由 Prompt 和 validator 共用；Provider 无法在正文自行填写数字或新增临床结论。详细合同见 [report/README.md](../report/README.md)。

## 测试

默认：`cargo test -p llm` 验证 DTO、Prompt、严格 Schema、输入长度和未配置状态。

启用功能：`cargo test -p llm --features openai` 验证真实 SDK 的回环 HTTP 请求、Schema、拒绝、截断、空内容、超时、HTTP 错误和配置脱敏。不需要真实 Key，不会调用付费 API。真实远端调用需要你自己的运行配置。
