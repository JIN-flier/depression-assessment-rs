# P4：可复现 EEG 预处理

本 crate 实现方案第 30 节 P4：去直流、带通、工频陷波、抗混叠重采样。
它只接受并返回 P1 的 `domain::EegRecording`，沿用 `[channel][sample]`
形式的 `Vec<Vec<f32>>`。运行时不依赖文件适配器、数据库、Slint、质量评价、
模型或 LLM。P3 导入和 P2 存储仅在集成测试中作为开发依赖。

## 模块边界

| 文件 | 职责 |
| --- | --- |
| `config.rs` | 可序列化配方、滤波相位、处理预算、调用方注入的执行上下文 |
| `pipeline.rs` | 全流程预检查、按顺序调用算法、状态与审计历史；提供 `EegProcessor` 接口 |
| `dc.rs` | 每个 EEG 通道独立计算并去除均值 |
| `filter.rs` | 私有 biquad 系数、每通道状态、因果/双向滤波与边界处理 |
| `resample.rs` | 私有 windowed-sinc 内核、抗混叠、模拟/触发通道策略 |
| `error.rs` | 本模块的配置、领域、长度、预算、数值错误 |

应用层可以注入 `Box<dyn EegProcessor>`，并在后台 blocking worker 调用。
crate 本身不创建线程或 runtime，因此测试和未来调度方式不受 UI 影响。
每次调用创建自己的滤波状态，同一个 pipeline 可以跨线程重复使用。

## 调用示例

```rust
use chrono::Utc;
use domain::EegRecording;
use eeg_signal::{PipelineConfig, ProcessingContext, SignalPipeline, SignalStep};

fn preprocess(raw: &EegRecording) -> Result<EegRecording, eeg_signal::SignalError> {
    let pipeline = SignalPipeline::new(PipelineConfig {
        steps: vec![
            SignalStep::DcRemoval {},
            SignalStep::bandpass(1.0, 45.0),
            SignalStep::notch(50.0),
            SignalStep::resample(250.0),
        ],
        ..PipelineConfig::default()
    })?;
    let mut context = ProcessingContext::new(Utc::now());
    context.software_version = "desktop-0.1.0".into();
    pipeline.process(raw, &context)
}
```

示例滤波参数要求执行该步骤时的采样率大于 100 Hz。这里只是显式配方示例，
默认配方为空；系统不会自动替用户决定处理参数。步骤顺序严格遵循配方。
先降采样到 80 Hz 再执行 50 Hz 陷波会在全流程预检查阶段失败。

## 算法与数值约定

### 去直流与滤波

- 去直流：每个 EEG 通道用 `f64` 累加均值，再从每个样本减去均值。
- 带通：串联二阶 Butterworth 高通和二阶 Butterworth 低通，Q = 1/√2。
  这是明确的 HP2 + LP2 串联，不能按理想矩形通带理解。
- 陷波：RBJ 二阶 notch，频率和 Q 显式保存，便捷构造器默认 Q = 30。
  支持 50/60 Hz 及其他低于当前 Nyquist 的中心频率，不自动猜测工频。
- 系数根据 [W3C Audio EQ Cookbook](https://www.w3.org/TR/audio-eq-cookbook/)
  实现。内部采用 `f64` direct form II transposed；使用 Jury 条件拒绝退化系数。
- `Causal`：单向、存在相位延迟；用首个输入的稳态初始化，至少需要 1 个样本。
- `ZeroPhase`：默认前后向各滤波一次，使用 24 个样本的奇反射扩展，裁掉边界。
  至少需要 25 个样本；不会对短数据悄悄切换算法。双向处理取消相位延迟，
  幅频响应为单向响应的平方，所以单边截止点约为 -6 dB。
- 有限长度记录边界仍存在瞬态，尤其是低截止频率或高 Q 情况。
  `ZeroPhase` 是离线处理，不能用于实时流；它不承诺与 SciPy `filtfilt` 默认值等同。
- 滤波和去直流只作用于 `ChannelKind::Eeg`，其他通道原样保留。

### 重采样

- 所有通道共同改变长度和采样率，以保持矩阵对齐。
- 模拟通道：对称 Hann 窗截断 sinc，直接按输出采样时刻计算，无累积时间步进误差。
  降采样前的低通截止为 `0.5 × min(源采样率, 目标采样率) × rolloff`。
  下采样时核半径扩展为 `ceil(half_width / min(1, 目标率/源率))`。
  默认 `half_width = 32`、`rolloff = 0.9`；半长度限制在 8–128，rolloff 在 0.5–0.99。
  有限核有过渡带，接近目标 Nyquist 的成分会衰减。
- 边界使用偶反射扩展，每个输出相位归一化权重，以保持常量幅值。
  支持非整数采样率比例、上采样及只有一个样本的记录。
- `Trigger`：零阶保持，事件码不会变成插值产生的分数。
  降采样可能漏掉短于输出采样间隔的触发脉冲；需要保留所有事件时，
  应另存事件表，不能依赖本模块的样本行重采样。
- 输出长度为 `round(N × 目标率 / 源率)`，结果小于 1 时返回错误。
  时长重新计算为 `输出长度 / 目标率`，相对原时长最多有半个目标样本的舍入误差。
- 采样率相同是样本的精确恒等操作，不额外低通；历史仍记录这次请求及 `identity=true`。
- [SciPy 重采样文档](https://docs.scipy.org/doc/scipy/reference/generated/scipy.signal.resample_poly.html)
  说明了低通与边界策略的重要性。本实现为直接 windowed-sinc，未使用 SciPy，
  也不宣称与其 polyphase 实现逐样本一致。

## 数据、错误与处理预算

`process` 不修改输入，成功返回新 recording；失败不返回部分结果或部分历史。
成功保留 recording/subject ID、通道顺序、单位、采集 metadata 和既有历史，
追加新步骤；非空配方将状态置为 `Preprocessed`。调用方负责另存原始数据，
本模块不决定文件名和存储覆盖策略。

由于领域字段公开，执行边界重新检查矩阵、通道、采样率、有限样本和时长；
无样本、NaN/Inf、ragged matrix、非法频率、数值溢出均返回 `SignalError`。
不补值、不截幅、不自动删除通道。坏通道检测与质量判断属于 P5。

默认预算：每个中间矩阵最多 64,000,000 个样本；sinc 半径最多 65,536；
每次重采样最多 500,000,000 次内核计算（按所有通道保守估计）。
预算可配置，形状/核/工作量在算法执行前检查。预算并非整个进程的内存上限：
原始数据、返回数据、临时 `f64` 通道缓冲会同时占用内存，应用层应按机器容量配置。

## 处理历史和回放

每个 `ProcessingStep` 记录：

- 语义操作名、具体算法版本；
- 完整 `configuration` JSON（包括相位、频率、Q、核参数）；
- 应用版本、`eeg-signal` crate 版本；
- 输入/输出采样率、样本数、通道范围、边界策略；
- 调用方注入的 `applied_at`，不在算法中读取系统时间。

`PipelineConfig` 支持 JSON 保存和加载，拒绝未知字段，加载后通过
`SignalPipeline::new` 校验参数。对本版本生成的历史，可从各步骤的
`parameters["configuration"]` 反序列化出 `SignalStep`，按原顺序重新构建配方。
回放应从同一原始 recording 开始，并先核对算法版本；不能对已处理结果重复执行。
相同运行平台、输入、配方和上下文得到相同结果（含历史）。跨平台浮点数学
不保证逐位相同，科研比较应使用合理数值容差。

## 测试和验收

```sh
cargo test -p eeg-signal --offline
cargo clippy -p eeg-signal --all-targets --offline -- -D warnings
cargo test --workspace --offline
```

- `tests/algorithms.rs`：均值、独立理论带通响应、双向相位、通道状态隔离、
  50/60 Hz 陷波、辅助通道保留、抗混叠、上下采样、非整数比例、常量/单样本。
- `tests/pipeline.rs`：接口注入、JSON/历史回放、输入不变、并发复用、
  中间采样率/长度、非法数据、极端数值、预算和生命周期。
- `tests/workflow.rs`：无 UI 的 P3 CSV 导入 → P4 完整 pipeline → P2 持久化，
  数据库重新打开后验证样本与元数据/历史；独立解析推导的 golden impulse 测试。
- `tests/data/notch-impulse.json`：固定解析参考数据，包含推导，不从待测算法生成期望值。

目前没有扩展到 P5 质量评分、P6 特征、UI 或模型推理；这些模块可以独立消费
返回的 recording 与 processing history。
