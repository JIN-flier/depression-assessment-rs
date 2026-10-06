# domain：共享领域模型

`domain` 定义抑郁评估工作区各 crate 之间交换的数据结构和局部业务约束。它只依赖通用的时间、序列化、错误和 UUID 库，不依赖 UI、数据库、EEG 算法、模型运行时或 LLM 实现。

本 crate 适合用于：

- 在导入、信号处理、特征、模型、融合和报告模块之间传递类型安全的数据；
- 在进入后续流水线前，用构造函数检查基本的数据形状与取值范围；
- 通过 `serde` 序列化领域结果；
- 构造不包含原始 EEG 样本的 `ReportContext`，作为报告生成器的输入。

它不负责持久化、临床诊断、算法计算或报告文本生成。当前持久化实现请参见 [`storage`](../storage/README.md)。

## 引入依赖

工作区内的 crate 可以使用路径依赖：

```toml
[dependencies]
domain = { path = "../domain" }
```

若调用方需要自行创建 UTC 时间，通常还需要：

```toml
chrono = "0.4"
```

## 快速开始：创建受试者和 EEG 记录

`EegRecording` 使用通道优先的矩阵布局：`samples[channel_index][sample_index]`。每个样本行必须和同下标的 `Channel` 对应。

```rust
use chrono::Utc;
use domain::{
    Channel, ChannelKind, EegRecording, RecordingId, RecordingMetadata,
    RecordingState, Sex, Subject, SubjectId,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let subject = Subject::new(
        SubjectId::new(),
        Some(32),
        Sex::Female,
        Utc::now(),
    )?;

    let recording = EegRecording::new(
        RecordingId::new(),
        subject.id,
        250.0,
        vec![
            Channel::new("Fp1", ChannelKind::Eeg, "uV")?,
            Channel::new("Fp2", ChannelKind::Eeg, "uV")?,
        ],
        vec![
            vec![0.1, 0.2, 0.3, 0.4],
            vec![-0.1, -0.2, -0.3, -0.4],
        ],
        RecordingState::Raw,
        RecordingMetadata::default(),
    )?;

    assert_eq!(recording.sample_count(), 4);
    assert_eq!(recording.eeg_channel_count(), 2);
    assert_eq!(recording.duration_seconds, 4.0 / 250.0);
    Ok(())
}
```

`EegRecording::new` 会检查：

- 采样率有限且大于零；
- 至少存在一个通道；
- 通道数和样本矩阵行数一致；
- 通道标签不重复；
- 所有通道的样本数一致；
- 所有样本均为有限 `f32`。

`Subject::new` 接受匿名 ID、可选年龄、性别和创建时间，年龄上限为 `Subject::MAX_AGE`（125）。领域模型刻意不包含姓名、电话或证件号；需要身份映射时，应由独立且受访问控制的系统管理。

## 构造报告输入

`ReportContext` 是报告叙述器（包括 LLM 报告模块）应接收的聚合对象。它只携带受试者摘要、结构化结果和紧凑 EEG 特征，不携带 `EegRecording::samples` 或采集设备等不必要的原始信息。

```rust
use chrono::Utc;
use domain::{
    AssessmentId, EvidenceDirection, EvidenceItem, EvidenceKind, EvidenceSet,
    IntegratedAssessment, Provenance, ReliabilityAssessment, ReliabilityLevel,
    ReportContext, Severity, Sex, Subject, SubjectId, SubjectSummary,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let now = Utc::now();
    let subject = Subject::new(SubjectId::new(), None, Sex::Unknown, now)?;

    let mut evidence = EvidenceSet::default();
    evidence.push(EvidenceItem::new(
        EvidenceKind::Scale,
        "phq9-result-id",
        "量表结果可用",
        EvidenceDirection::Neutral,
    )?);

    let provenance = Provenance::new("0.1.0", "fusion", "fusion-v1", now)?;
    let assessment = IntegratedAssessment::new(
        AssessmentId::new(),
        subject.id,
        None,
        Severity::Unknown,
        ReliabilityAssessment::new(ReliabilityLevel::Low),
        evidence,
        now,
        provenance,
    );

    let context = ReportContext::new(SubjectSummary::from(&subject), assessment)?;
    assert!(!context.integrated_assessment.conclusion_allowed);
    Ok(())
}
```

`ReportContext::new` 会拒绝受试者 ID 与综合评估不一致的数据。`IntegratedAssessment::new` 默认将 `conclusion_allowed` 设为 `false`；只有独立的安全模块完成规则判断后，才应允许强结论。`domain` 本身不会给出诊断或修改这一安全状态。

## 公共模型概览

crate 根模块重导出了所有公共类型，调用方通常可直接 `use domain::*`，也可以按需导入具体类型。

| 模块 | 主要类型 | 用途 |
| --- | --- | --- |
| `identifiers` | `SubjectId`、`VisitId`、`RecordingId`、`ScaleResultId`、`ModelResultId`、`AssessmentId` | 基于 UUID 的强类型匿名 ID，避免混用实体 ID |
| `subject` / `visit` | `Subject`、`Sex`、`Visit` | 受试者与纵向访视关联 |
| `eeg` | `EegRecording`、`Channel`、`RecordingMetadata`、`ProcessingStep` | 标准 EEG 数据、采集信息和处理历史 |
| `scale` | `ScaleResult` | 量表总分、维度分数和完整度 |
| `quality` | `SignalQuality`、`ChannelQuality` | 整体及逐通道质量结果 |
| `features` | `EegFeatures`、`SpectralFeatures`、`BandPower` | 频谱以及可扩展的空间、连接和复杂度特征 |
| `model` | `ModelResult`、`ModelCompatibility`、`ExplainabilityResult` | 推理、兼容性、不确定性和解释结果 |
| `evidence` | `EvidenceItem`、`EvidenceSet` | 按 EEG、量表、模型和纵向来源归类的证据 |
| `assessment` | `IntegratedAssessment`、`ReliabilityAssessment`、`Severity` | 融合后的结构化评估和可靠性 |
| `report` | `SubjectSummary`、`EegFeatureSummary`、`ReportContext` | 隐私最小化的报告输入 |
| `types` / `error` | `UnitInterval`、`Provenance`、`Warning`、`Metadata`、`DomainError` | 通用值对象、追溯信息和校验错误 |

## 关键约束

### 比例和概率

使用 `UnitInterval::new(field, value)` 构造 `[0, 1]` 内的有限值：

```rust
use domain::UnitInterval;

let probability = UnitInterval::new("probability", 0.82)?;
assert_eq!(probability.get(), 0.82);
# Ok::<(), domain::DomainError>(())
```

`UnitInterval` 的反序列化也会执行相同校验。`ZERO` 和 `ONE` 可用于明确的边界值。

### 可追溯性

算法输出应携带 `Provenance`，记录软件版本、算法名、算法版本、参数和生成时间。参数与其他扩展字段使用 `Metadata = BTreeMap<String, String>`，序列化顺序稳定。

### 模型兼容性和不确定性

不兼容模型使用 `ModelCompatibility::Incompatible { reasons }` 显式表达，而不是继续静默推理。`ModelResult::with_uncertainty` 只接受有限且非负的熵和 OOD 分数。

### 信号质量和频谱

- `SignalQuality::new` 要求每个 `bad_channels` 条目都存在于 `channel_quality` 中，且不得重复；
- `SpectralFeatures::new` 要求每个通道的 PSD 长度等于频率轴长度，并拒绝非有限数值；
- `ScaleResult::new`、`EvidenceItem::new`、`Provenance::new` 等构造函数会拒绝必填空字符串。

## 序列化与演进

公共模型实现了 `serde::Serialize` / `Deserialize`，可用于 JSON 或其他 serde 格式。枚举通常以 `snake_case` 编码；强类型 ID 透明编码为 UUID。

新增的集合字段广泛使用 `#[serde(default)]`，便于读取缺少这些字段的旧数据，但当前 crate 尚未承诺跨任意版本的迁移策略。长期保存的数据应记录应用和算法版本，并在升级时通过迁移测试验证。

构造函数负责当前已实现的局部校验，但多数结构体字段是公开的，派生的反序列化也不一定调用构造函数。调用方直接修改字段或读取不可信数据后，仍需维护相同约束；不要把本 crate 当作完整的输入验证或安全边界。`UnitInterval` 是例外，它在反序列化时会重新校验。

## 错误处理

需要校验的构造函数返回 `Result<_, DomainError>`。错误可按变体匹配，也可直接向上传播：

```rust
use domain::{DomainError, UnitInterval};

let result = UnitInterval::new("confidence", 1.5);
assert!(matches!(result, Err(DomainError::OutOfRange { .. })));
```

## 测试

```bash
cargo test -p domain
cargo clippy -p domain --all-targets -- -D warnings
cargo fmt --all -- --check
```

`tests/domain_workflow.rs` 覆盖从受试者和 EEG 数据到特征、证据、综合评估及 `ReportContext` 的 JSON 往返，并检查报告上下文不会泄露原始样本或设备信息。
