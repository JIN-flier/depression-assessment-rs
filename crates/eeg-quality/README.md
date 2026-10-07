# P5：结构化 EEG 信号质量评估

本 crate 实现方案第 30 节 P5，独立读取 P1 的 `domain::EegRecording`，
输出 `domain::SignalQuality`：整体评分、坏通道、逐通道指标和结构化警告。
涵盖平直信号、极端幅值、方差异常、NaN/Inf、工频干扰及高频噪声。

## 解耦边界

| 模块 | 职责 |
| --- | --- |
| `config.rs` | 可序列化检测参数、独立评分策略、资源预算、时间/版本上下文 |
| `analyzer.rs` | 入口校验、算法调度、结果汇总、可复现来源信息 |
| `time_domain.rs` | 缺失比例、平直片段、极端幅值、窗口方差、伪迹并集 |
| `spectral.rs` | 质量专用周期图及工频/高频功率占比；使用 RustFFT |
| `scoring.rs` | 将观测量转成质量分数、坏通道及稳定警告码 |
| `error.rs` | 配置、输入、单位、预算、领域及序列化错误 |

运行时仅依赖 domain、chrono、serde、serde_json、thiserror、rustfft，沿用现有
`samples[channel][sample]` 数据契约。P3/P4 仅作为集成测试的开发依赖。
不依赖 P6 特征、数据库、UI、模型、LLM 或任何外部运行时。
不创建线程，应用层负责在 blocking worker 中调度；可注入
`Box<dyn SignalQualityEvaluator>`。同一实例可并发复用，无共享可变状态。

## 使用方式

```rust
use domain::{EegRecording, SignalQuality};
use eeg_quality::{QualityAnalyzer, QualityConfig, QualityContext, QualityResult};

fn assess(recording: &EegRecording, context: &QualityContext) -> QualityResult<SignalQuality> {
    let mut config = QualityConfig::default();
    config.spectral.line_frequency_hz = Some(50.0); // 60 Hz 地区应显式改为 60.0。
    QualityAnalyzer::new(config)?.assess(recording, context)
}
```

调用方用 `QualityContext::new(timestamp)` 注入时间，并可设置应用版本。
`assess_detailed` 额外返回逐通道 `ChannelMeasurements` 和辅助通道排除列表，
供科研检查、可视化或未来策略使用。算法不会补值、去直流、滤波、删除通道
或修改输入；P4 预处理前后应分别评估，并分别保存结果。

## 输入约定与缺失值

- 仅评估 `ChannelKind::Eeg`。EOG/ECG/EMG/Trigger/Other 保持在输入中，
  不参与质量评分；无 EEG 通道返回错误。坏 EEG 通道仍参与整体评分。
- 幅值测量内部统一为 µV，支持 `uV`、`µV`、`μV`、`mV`、`V`，单位大小写敏感。
  转换仅用于测量，不修改样本；未知单位（例如 ADC counts）拒绝计算，避免错误阈值。
- 重新检查公开领域字段：正且有限的采样率、非空等长矩阵、唯一非空通道名、
  非空单位，以及 `duration_seconds = N/fs`。资源预算在算法执行前检查。
- `EegRecording::new` 和 P3/P4 仍拒绝非有限样本。P5 刻意支持检查公开字段中
  已损坏/被上游更改的 NaN/±Inf，将其视作缺失；不放宽其他模块的正常数据校验。
- 矩阵本身为空或形状错误属于输入错误，不能解释为一个低质量 EEG。
  全缺失 EEG 行则可以得到缺失比例 1、评分 0 的有效质量结果。
- 单个有限样本不足以评估方差，触发 `INSUFFICIENT_FINITE_SAMPLES` 并限制评分，
  即便显式禁用频域检查，也不会将它报告为高质量。

## 时域算法

所有样本比例的分母都是通道总样本数 N，而不是有限样本数。
默认非重叠窗口 2 秒，长度 `round(fs × window_seconds)`，至少 8 个样本。
尾部不足一个窗口仍参加时域检测。

| 检测 | 定义 | 默认参数 |
| --- | --- | --- |
| 缺失 | 非有限样本数 / N | NaN、+Inf、-Inf |
| 平直 | 相邻有限样本差绝对值不超过容差的连续片段 | 0.01 µV，至少 0.5 秒 |
| 极端幅值 | 有限样本相对物理零点的绝对幅值 ≥ 阈值 | 200 µV |
| 方差异常 | 每窗口有限样本的总体方差超出指定上下界 | < 0.01 或 > 10,000 µV² |
| 时域噪声 | 上述平直、极端幅值、方差异常对应样本的并集 / N | 重叠伪迹只计一次 |

平直检测跨窗口边界，缺失或过大跳变会中断片段；至少需要
`max(2, ceil(fs × minimum_duration_seconds))` 个连续样本。
极端幅值检测保留 DC 偏移的影响；没有在质量模块内偷偷去直流。
方差用 f64 Welford 算法；窗口至少有两个有限样本才计算方差，缺失样本不进入
伪迹并集。全局 `variance_uv2` 独立记录通道所有有限样本的总体方差。
本版本的方差异常是绝对窗口阈值，不采用跨通道中位数，以免多数坏通道改变参照。

## 频域算法与证据不足

每个完整且全部有限的非重叠窗口：去均值 → periodic Hann → 补零至下一
2 的幂 → f64 FFT → 单边周期图。DC/Nyquist 不翻倍，其他正频率功率翻倍。
相同窗口长度的周期图先相加，再计算功率比例；这相当于对周期图求平均后取比值，
不能用每个窗口比例的等权平均替代。PSD 的共同归一化、频点间隔和平均因子
在分子分母中消去。方法参考 [SciPy 官方 Welch 文档](https://docs.scipy.org/doc/scipy/reference/generated/scipy.signal.welch.html)，
本实现使用非重叠窗口，仅输出质量功率占比，不输出 P6 特征或完整 PSD。

FFT 使用 `rustfft::FftPlanner<f64>` 规划正向变换，复数类型直接使用库的
`Complex<f64>`，不再维护手写 FFT。每个通道评估中复用 FFT 计划及 scratch
缓冲，窗口间使用 `process_with_scratch`，不会每个窗口重新分配 FFT 工作区。
根据 [RustFFT 官方文档](https://docs.rs/rustfft/6.4.1/rustfft/)，正向输出未归一化，
且按频率升序排列，因此沿用现有单边功率及功率比例计算。

- 分母：`[minimum_hz, Nyquist]` 中 FFT 频点的功率和，默认 minimum_hz = 1 Hz。
- 工频分子：`[line_frequency_hz − half_width, line_frequency_hz + half_width]`，
  默认 50 ± 1 Hz；整个工频带必须严格位于 Nyquist 以下。
- 高频分子：`[high_frequency_start_hz, Nyquist]`，默认从 30 Hz 开始。
  高频指标包含位于该频段的工频能量；两者是质量维度，不是独立相加的频段。
- 频点按闭区间归属；非 FFT 整数频率会产生泄漏，有限长度窗口不等同理想矩形频带。
- 含任何 NaN/Inf 的完整窗口跳过，不填零、不跨缺口拼接；尾部不补成有效窗口。
  `spectral_coverage = 有效完整窗口数 × 窗口长度 / N`。
- 补零只增加频点密度。真实分辨率 `fs / window_samples` 必须不大于
  工频 half_width，否则工频证据不可用。采样率不足、高频带不可观测、无有效窗口
  或覆盖比例低于 0.5，也会产生明确警告和评分限制。
- 两个频域检测均可显式设为 `None` 禁用。禁用不等于测量值为零。
  全零但完整有限的窗口能量为零，功率比例约定为零；平直/低方差规则仍会拒绝它。

详细测量中的两个功率占比为 `Option<UnitInterval>`，`None` 表示禁用或不可测量。
P1 的 `ChannelQuality.power_line_interference` 不是可选字段，所以缺少工频证据时
保留兼容值 0；消费者必须结合警告和 provenance 中的详细测量判断，不能把这个
占位值解释成“已经证明没有干扰”。

## 评分策略

评分是工程质量指标，范围 0–1，越高越好；它不是抑郁风险或临床可信度。
所有默认阈值为可配置工程启发式，应按设备、采集协议和验证数据调整。

每个质量维度用 `q = clamp(1 − observed_fraction / (2 × limit), 0, 1)`，
通道评分取缺失、时域噪声、工频、高频这四个可用维度的最小值。
达到任一比例阈值，或者通道评分严格低于 0.6，均标记坏通道。
默认比例阈值分别为 0.05、0.20、0.20、0.40。

请求的频谱证据不可用/覆盖不足时，通道评分最多为 0.5，必定标记坏通道；
“坏通道”在这里同时包括质量不合格和证据不足，具体原因见逐通道警告。
整体评分为所有 EEG 通道评分的算术平均，不删除已发现的坏通道。
检测值和评分策略独立，修改 `ScoringConfig` 不改变测量结果。

警告使用稳定 `code`，显示层可以自行本地化 `message`。主要代码：
`MISSING_SAMPLES`、`FLATLINE`、`EXTREME_AMPLITUDE`、`VARIANCE_ANOMALY`、
`POWER_LINE_INTERFERENCE`、`HIGH_FREQUENCY_NOISE`、`BAD_CHANNEL`、
`LINE_BAND_UNAVAILABLE`、`HIGH_FREQUENCY_BAND_UNAVAILABLE`、
`ANALYSIS_BAND_UNAVAILABLE`、`NO_VALID_SPECTRAL_WINDOWS`、
`INSUFFICIENT_SPECTRAL_COVERAGE`、`INSUFFICIENT_FINITE_SAMPLES`。
整体结果还包含坏通道汇总、非 EEG 排除和证据不完整警告。

## 复现和资源边界

Provenance 记录应用版本、算法 `eeg-quality/p5-qc-v2`、crate 版本、调用方时间、
完整配置、检测值、实际窗口/FFT/平直最小长度、采样率/样本数、通道单位、
输入生命周期和 P4 processing history。配置及详细结果支持 JSON 往返，
JSON 使用 `float_roundtrip` 保留包括极小功率占比在内的 f64 值；
配置拒绝未知字段，构造器再次验证数值。保持同一 recording、配置、上下文
和平台即可重现完整结果；跨平台浮点结果应按容差比较。

`p5-qc-v2` 将 FFT 后端从手写实现改为 RustFFT；检测规则和评分公式保持一致，
`spectral_method` 标记 RustFFT 后端，库的精确依赖版本由 `Cargo.lock` 固定。
FFT 实现和 CPU 指令路径变化可能导致浮点末位差异，旧结果应按原版本回放。

默认预算：全矩阵 64,000,000 个样本、FFT 长度 16,384、所有 EEG 通道共
1,000,000 个窗口。先校验乘法/FFT 长度及预算，再执行算法。
按通道顺序处理，工作内存为该通道样本掩码、FFT 窗口/功率数组、计划及 scratch 缓冲，
不会复制整个 recording。预算不是整个应用的内存上限。

## 测试与验收

```sh
cargo test -p eeg-quality --offline
cargo clippy -p eeg-quality --all-targets --offline -- -D warnings
cargo test --workspace --offline
```

- 频域核：完整功率比例与独立直接 DFT 比较，覆盖多窗口计划/工作区复用和补零。
- `tests/algorithms.rs`：已知正弦方差、平直片段、伪迹并集、正负幅值阈值、
  高低方差、缺失/全缺失、50/60 Hz、Nyquist 单边权重、跨窗口能量加权、
  单位转换、辅助通道、尾部、低采样率/短记录/稀疏窗口及 f32 极值。
- `tests/contracts.rs`：策略与测量分离、接口注入、并发复用、JSON/参数回放、
  入口损坏数据、非法配置、预算和长度溢出。
- `tests/workflow.rs`：P3 CSV → P5 → P4 notch → P5，验证质量改善、
  输入不变和历史可追溯；覆盖非 2 的幂窗口及固定解析 golden。
- `tests/data/analytic-mixture.json`：独立解析推导的正弦混合参考参数和期望值，
  无身份信息，不从待测算法产生期望结果。
