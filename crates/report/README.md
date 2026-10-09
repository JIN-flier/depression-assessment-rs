# P9 结构化结果到报告

流水线：`ReportContext → prepare_v1 → NarrationInput → ReportNarrator → DraftNarrative → validate_narrative → ReportDocument`。

`eeg-application` 负责将 P8 分析结果映射到领域上下文并管理状态；`report` 只处理结果投影、模板、校验和文档；`llm` 负责 Provider；Slint 只提交命令和展示验证后的文档。报告不会重新读取样本或运行算法。P10 导出消费同一份 `ReportDocument`，支持 PDF / DOCX / JSON / CSV。

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

报告及分析结果在会话中保存，P10 可显式导出完整本地归档与用户文档。API 配置与 Key 不经过 UI 表单或数据库。

## 测试

```text
cargo test -p llm -p report -p eeg-application
cargo test -p depression-desktop
```

覆盖完整工作流、隐私投影、关联/范围校验、未知或不匹配引用、伪造数字/临床表述、解释与限制语篡改、失败与重试、状态失效、有界后台调度、真实 Slint 回调与报告页绘制。OpenAI 适配器的可选依赖和回环 HTTP 测试见 [llm/README.md](../llm/README.md)。

## P10 导出与完整 V1 闭环

`report::export` 只依赖 `domain`、已有 `serde / serde_json` 和标准库，无新增 Cargo 包。
采用 V1 范围的 PDF / OOXML 编码器；字体解析只读取 TrueType 表和字符映射，
不执行字体指令。各模块独立：

- `archive.rs`：显式无样本 DTO、实体与数值关联验证、共用用户文档段落。
- `csv.rs`：科研长表、完整 JSON pointer、无舍入数值、单位和安全转义标记。
- `pdf.rs` / `font.rs`：A4、度量换行、分页/页码、嵌入中文字体、Unicode 可复制文字。
- `docx.rs`：标准 OOXML ZIP、标题/段落样式、XML 转义、页脚页码。
- `writer.rs`：可注入 `ReportExporter`、内存编码后完整发布、失败清理和回执。

`ExportBundle::new(subject, analysis, validated_document)` 校验后构造归档。
可传 `None` 离线导出 JSON / CSV；PDF / DOCX 必须传入 P9 `ReportDocument`。
任意草稿/反序列化文字不能装入报告字段。文档正文、解释边界、限制语与 UI 使用
同一已验证内容，附录包括匿名导联映射、原始/处理后质量、频段功率和处理溯源。

### 科研归档合同

JSON 顶层 `schema_version = "eeg-v1-export/1"`，包括 `software_version`、`subject`、
`analysis` 与可选 `report`。`analysis` 包含两份录制描述（采集 metadata、导联、
采样率、时长、sample_count、处理历史）、两份质量、完整 features（包括 PSD
频率轴及所有通道功率），以及可解析的 pipeline / quality / features 参数。
`report` 包含完整本地上下文、Provider 名和已校验正文。录制描述没有 `samples`。
JSON 保留本地身份 ID、采集说明和备注，所有输出都只写到用户选择的本地文件。

CSV 为 UTF-8 BOM、RFC 4180 双引号/CRLF 的长表，列为：

```text
schema_version,subject_id,recording_id,path,value,value_type,unit,spreadsheet_escaped
```

`path` 是 RFC 6901 pointer，如 `/analysis/features/spectral/psd_by_channel/Fp1/0`。
频率轴下标与各通道 PSD 下标对应。`value_type` 区分 string / number / boolean /
null / array / object；空容器也有一行。绝对功率单位 `uV^2`，PSD 单位 `uV^2/Hz`，
相对功率和质量评分单位 `ratio`，数值不乘 100；用户 PDF / DOCX 才展示百分比。

危险字符串（跳过前导空白/控制字符后以 `= + - @` 开头）额外添加一个单引号，
`spreadsheet_escaped=true`。科研读取时仅按该标记移除一个单引号，可以精确恢复
原文，包括本来就以单引号开头的普通字符串。数值字段保持数值，不受此转义影响。

### 文件与字体

目标扩展名省略时自动补齐；扩展名与格式冲突则失败。所有输出先在目标同目录
写入随机临时文件并 `sync_all`，再用 `hard_link` 原子发布；目标必须不存在。
已有文件、符号链接或并发导出冲突均不会覆盖；失败会关闭句柄再清理临时文件。
要求目标文件系统支持硬链接（常用 ext4 / NTFS / APFS）；不支持时应选择本地磁盘。
Unix 输出权限为 0600。PSD 总标量数上限 4,000,000，输出上限 128 MiB，PDF
页数上限 4096，字体上限 32 MiB；超限明确报错，不截断科研数据。

PDF 字体可通过 `FileReportExporter.fonts.pdf_font` 注入，或设置 `EEG_REPORT_FONT`：

```bash
EEG_REPORT_FONT=/path/to/chinese-font.ttf cargo run -p depression-desktop --offline
```

未指定时检查 Linux DroidSansFallbackFull / WenQuanYi、Windows SimHei、macOS
Arial Unicode 的常见路径。必须使用可嵌入的 TrueType `.ttf`；TTC / CFF / WOFF
不支持，受限制或仅允许 bitmap 的字体拒绝嵌入。中文完整嵌入 PDF，Latin 使用
PDF 标准 Helvetica。缺字或字体错误明确失败；DOCX / JSON / CSV 不受影响。
DOCX 使用 Arial / Noto Sans CJK SC 并允许阅读器的本地字体回退。

### 验证

```bash
cargo test --workspace --offline
cargo clippy -p report -p eeg-application -p depression-desktop --all-targets --offline -- -D warnings
P10_EXPORT_PREVIEW_DIR=/tmp/p10-export-preview cargo test -p eeg-application --test exporting --offline
python3 scripts/validate_v1_exports.py /tmp/p10-export-preview
```

最后两项从项目根目录运行。独立校验脚本只需 Python 标准库与 Poppler
`pdfinfo / pdftotext`，检查全部 CSV 值与 JSON 的逐值匹配、DOCX ZIP CRC / XML /
正文和 PDF 可提取正文。文件示例只含合成数据，预览产物不提交到仓库。
