# P9 结构化结果到报告

流水线：`ReportContext → prepare_v1 → NarrationInput → ReportNarrator → DraftNarrative → validate_narrative → ReportDocument`。

`eeg-application` 负责将 P8 分析结果映射到领域上下文并管理状态；`report` 只处理结果投影、模板、校验和文档；`llm` 负责 Provider；Slint 只提交命令和展示验证后的文档。报告不会重新读取样本或运行算法。P10 的导出尚未实现，可以继续消费 `ReportDocument`。

## V1 输入与隐私

`build_v1_report_context` 校验受试者、原始录制、处理后录制、质量和特征的关联。原始/处理后质量和频段功率来自同一次成功分析，波形通道选择不改变报告事实。综合评估固定为 `Unknown`、无 risk、`conclusion_allowed=false`；Low 只是领域结构的保守占位，正文不宣称完成了临床可信度评估。

本地文档保留 `ReportContext`、匿名通道序号到真实导联的映射、原始质量结果、分析配置和算法溯源。上传投影只包含固定键、有限非负数值和固定局限性。任意自由文本和标识均不上传。处理后的质量与特征必须具有同一 Recording ID，摘要质量分值必须与结构化质量结果相等。

`channel_1` 等序号按处理后录制的导联顺序映射，排除通道不会重排编号。质量警告、原始坏导联、特征排除等情况通过固定限制语保留存在性；完整详情仍可在本地分析页查看。

## Narrative 合同

Provider 返回且只返回以下 JSON（解释和局限性使用输入原文）：

```json
{
  "summary": {
    "text": "已有分析结果如下：{{processed_quality_score}}。",
    "fact_ids": ["processed_quality_score"]
  },
  "description": {
    "text": "测量结果如下：{{channel_1.alpha.relative}}。",
    "fact_ids": ["channel_1.alpha.relative"]
  },
  "interpretation": "逐字复制输入 interpretation",
  "limitations": ["按顺序逐字复制输入 limitations"]
}
```

原始草稿和可展示 `ValidatedNarrative` 是不同类型；后者字段私有，只有 validator 能构造。Validator 检查严格 JSON（拒绝额外/重复字段与 Markdown 包裹）、段落非空、长度上限、已知事实、正文引用与声明一致、声明无重复、解释与局限性逐字一致。数值、单位及事实标签由本地模板填入，不接受 Provider 手写数字或新检查结果。

仅靠“疾病关键词过滤”无法完整验证任意自然语言的含义，故 V1 采用受控连接语。LLM 可以选择、排序事实并组织段落，事实之外仅允许共用 `CONNECTIVES` 表的词语和标点，不开放正常异常、诊断、严重度、风险、可信度、治疗或自由建议。这是 P9 的明确能力边界；后续开放自由表达需要独立扩展校验策略。

## 状态与失败

`GenerateReport` 必须已有成功分析。超时、网络错误、拒绝、内容截断或校验失败不发布候选，也不清除上次有效报告；重试成功后原子替换。新项目、新受试者、新录制、成功重新分析、成功质量评价或当前受试者资料修改都会清除旧文档。显示通道/时间窗/频段切换及保存处理后录制保持原文档。失败的分析和失败的资料修改保持最后成功状态。

报告及分析结果目前只在会话中保存，尚未实现 P10 的持久化报告/导出。API 配置与 Key 不经过 UI 表单或数据库。

## 测试

```text
cargo test -p llm -p report -p eeg-application
cargo test -p depression-desktop
```

覆盖完整工作流、隐私投影、关联/范围校验、未知或不匹配引用、伪造数字/临床表述、解释与限制语篡改、失败与重试、状态失效、有界后台调度、真实 Slint 回调与报告页绘制。OpenAI 适配器的可选依赖和回环 HTTP 测试见 [llm/README.md](../llm/README.md)。
