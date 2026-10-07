# P6 EEG 基础频谱特征

本 crate 完成方案第 30 节 P6：Welch PSD、Delta/Theta/Alpha/Beta
绝对功率与相对功率，输出已有 `domain::EegFeatures`。它不生成诊断或风险等级。

## 模块边界

| 文件 | 职责 |
|---|---|
| `config.rs` | 可序列化的算法参数、频段定义、资源限制、注入时间与版本 |
| `validation.rs` | 验证公开领域字段、单位与通道选择，推导采样点和检查预算 |
| `welch.rs` | 周期 Hann、去趋势、FFT、单边 PSD 与窗口平均 |
| `band_power.rs` | 频率 bin 归属与积分约定 |
| `extractor.rs` | 协调算法、组装领域结果、记录 provenance |
| `error.rs` | 可匹配的 P6 专属错误 |

运行时不依赖 `eeg-io`、`eeg-signal`、`eeg-quality`、storage、Slint、模型或 LLM。
P3–P5 是 dev-dependencies，仅用于无 UI 的工作流测试。
应用层通过 `EegFeatureExtractor: Send + Sync` 注入服务，替换算法不影响领域模型。
`SpectralExtractor` 的配置在构造时验证且不暴露可变引用；每次调用拥有自己的 FFT
计划及工作区，可从多个后台线程共享提取器。同步计算应在 blocking worker 上调度。

## 使用方法

```rust,ignore
use eeg_features::{FeatureConfig, FeatureContext, SpectralExtractor};

let mut config = FeatureConfig::default();
// 可由应用层显式把 P5 的 bad_channels 转为排除列表。
// config.excluded_channels = quality.bad_channels.clone();
let extractor = SpectralExtractor::new(config)?;
let context = FeatureContext::new(generated_at);
let detailed = extractor.extract_detailed(&recording, &context)?;
let features = detailed.features; // domain::EegFeatures，可供存储/报告/可视化使用
let audit = detailed.audit;       // 实际窗数、通道选择、功率分母等
```

仅需领域结果时使用 `extract(&recording, &context)`，不必处理详细包装结构。
`audit` 和完整配置也写入 `features.provenance.parameters`，单独保存领域结果不会丢失审计信息。

## Welch 定义

输入采用 P1 的 `[channel][sample]` 结构。选中 EEG 的 `V`、`mV`、`uV`、`µV`、`μV`
统一在内部以 f64 转换为微伏，原始 f32 数组与单位均不修改。

默认参数：

| 参数 | 默认值/规则 |
|---|---|
| 窗长 | 2 秒，`L = round(fs * window_seconds)`，至少 3 点 |
| 重叠 | 50%，`O = floor(L * overlap_fraction)` |
| 步长 | `H = L - O`，必须大于 0 |
| 窗函数 | 周期 Hann：`w[n] = 0.5 - 0.5*cos(2πn/L)` |
| 去趋势 | 逐完整窗减去该窗加窗前的算术均值，可配置 `Detrend::None` |
| FFT 长度 | 默认 `N = L`，rustfft 支持非二次幂；显式 N 必须 >= L |
| 窗平均 | 所有完整窗的 PSD 算术平均 |
| 尾段 | 最后完整窗之后的样本不参与，记录 `trailing_samples` |

短于一个完整窗的录制返回 `TooShort`，不隐式缩窗。显式 N>L 时仅对有效窗做 FFT
零填充；不对录制缺失值补零，也不补齐未完成窗。

对每个完整窗的未归一化前向 FFT `X[k]`：

```text
U = sum(w[n]^2)
PSD_window[k] = c[k] * |X[k]|² / (fs * U)
PSD[k] = mean(PSD_window[k])
f[k] = k * fs / N, k = 0 .. floor(N/2)
```

`c[0]=1`；偶数 N 的 Nyquist bin `c[N/2]=1`；其他正频率 `c[k]=2`。
奇数 N 最后一个 bin 仍乘 2，且其频率小于 Nyquist。
PSD 单位 `uV²/Hz`。整个单边 PSD 的 `sum(PSD)*df` 等于各窗去趋势后信号的
Hann 加权均方能量平均值，不要求它等于任意非平稳信号的全录制无窗方差。
这组 Welch 选择与 [SciPy Welch 文档](https://docs.scipy.org/doc/scipy/reference/generated/scipy.signal.welch.html)
中的周期 Hann、去均值、单边 density、mean 约定一致；运行时完全使用 Rust。

`SpectralAudit.frequency_resolution_hz` 是网格间距 `df=fs/N`；真实分辨能力由
窗长决定（约 `fs/L`），零填充只加密网格。科学分析时应同时查看 L 和 N。

## 频段与相对功率

| 频段 | 默认范围（Hz） |
|---|---|
| Delta | `[0.5, 4)` |
| Theta | `[4, 8)` |
| Alpha | `[8, 13)` |
| Beta | `[13, 30)` |
| 相对功率分母 | `[0.5, 30)` |

频段使用可序列化的 `BandDefinition`，可调整边界或显式添加领域中的 Gamma。
这些是工程默认范围，不是临床阈值。

```text
absolute(band) = sum(PSD[k] where f[k] belongs to band) * df
relative(band) = absolute(band) / absolute(relative_power_range)
```

采用 bin 中心归属的矩形积分：下界包含、上界不包含；当上界恰等于 Nyquist 时
额外包含该 bin。相邻范围不会重复计入边界 bin。Hann 的谱泄漏会使恰在频段边界
的正弦功率分布到邻近频段；此时不能将整个峰的功率都解释为所属频段的功率。
不使用梯形端点半权，以保持离散单边谱的 DC/Nyquist 能量和 Parseval 恒等式。
粗网格时窄频段的积分具有离散误差，只有零填充不会增加原始信号信息。

频段不得重复、重叠或超出分母范围，可只输出频段子集；分母不会随输出子集改变。
分母范围高于 Nyquist 返回 `UnavailableRange`，不会截断频段；任何请求范围没有
可归属 bin 返回 `UnresolvedRange`，不会生成看似有效的零功率。
若选中通道的分母为零，相对功率约定为零，并列入 `zero_power_channels`。
该列表仅描述所选分母范围的能量，不代表质量结论或整个信号必然为零。

## 输入与可追溯性

- 自动排除非 EEG 通道；仅按配置显式排除 EEG 通道，不读取或修改质量结果。
- 排除列表中不存在的通道返回 `UnknownChannel`，全部 EEG 被排除返回 `NoEegChannels`。
- 验证采样率、形状、标签、时长及所有行的有限性；即使被排除的行存在 NaN/Inf
  也返回 `Domain(NonFiniteSample)`，符合 P1 的完整录制契约。缺失数据修复属于独立策略。
- 未知配置字段直接拒绝；非法数值配置在构造提取器时拒绝。
- 所有输入样本数、FFT 长度、总窗数、PSD 输出数量和估算 FFT 操作量均有可配置预算。
- 默认仅填充 `spectral`，spatial/connectivity/complexity 保持 None。
- `Provenance` 记录注入的软件版本/时间、`p6-welch-v1` 算法版本、完整配置、实际
  窗参数、单位、FFT 后端、积分约定、输入通道/采样率/样本数/录制状态和预处理历史。
- 固定输入、配置和 context 可在同一运行时重复得到相同完整结果。
  不同 CPU/FFT 后端可能存在浮点微差，科研比对应使用数值容差。
- JSON 启用 `float_roundtrip`，避免审计结果回读产生浮点精度变化。

## 测试与验收

依赖 sync 后，在 workspace 根目录执行：

```bash
cargo test -p eeg-features
cargo test -p domain -p eeg-io -p eeg-signal -p eeg-quality -p eeg-features
cargo clippy -p eeg-features --all-targets -- -D warnings
cargo fmt --all -- --check
```

测试覆盖：独立直接 DFT 对照（含重叠、去趋势、奇偶长度、零填充）、已知正弦
PSD/功率、解析混合波 golden fixture、Parseval、窗功率平均、DC/Nyquist、零能量、
单位等价、尾段、通道选择、配置/输入错误、资源预算、JSON 回放、并发接口，
以及 P3→P5→P4→P6 无 UI 工作流。Golden 使用解析 `A²/2`，独立于生产算法。

使用已准备的 `rustfft = 6.4.1`；不自动执行依赖下载或 sync。
