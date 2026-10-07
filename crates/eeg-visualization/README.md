# P7 EEG 可视化

按方案第 30 节完成 Waveform / PSD / Topomap，提供可直接查看的 SVG 图像。
不改动 P1–P6 的分析接口，完整 Slint 工作流留在 P8。

## 模块边界

```text
domain::EegRecording ──→ waveform ──┐
                                  │
domain::EegFeatures ───→ psd ───────┼──→ 显示 DTO ──→ PlotRenderer ──→ SVG
                  └──→ topomap ────┘
                         ↑
                  ElectrodeLayout
```

| 模块 | 职责 |
|---|---|
| `config.rs` | 时间/频率范围、通道选择、变换、缺失策略、预算 |
| `data.rs` | UI 和图像后端无关的显示结构，支持 JSON 回放 |
| `validation.rs` | 校验公开领域字段、形状、单位、通道和资源预算 |
| `service.rs` | 波形包络、PSD 显示变换、统一 `EegVisualizer` 接口 |
| `layout.rs` | 自定义坐标和明确标记为示意的 19 导联布局 |
| `topomap.rs` | 凸包/头圆覆盖策略、固定 IDW(p=2) 插值 |
| `svg.rs` | 坐标投影、坐标轴、通道面板、色标、XML 转义 |
| `error.rs` | 可匹配错误；失败不返回部分图形 |

运行时只依赖 `domain` 和工程已有的 `serde`、`thiserror`。P3–P6 仅为
dev-dependencies，用于工作流测试和示例。服务与渲染器都是 `Send + Sync`
的 object-safe 接口，可独立替换、注入测试替身；不会调用数据库、Slint、
模型、LLM 或质量分析服务，也不会重新计算 PSD 或风险等级。

## 使用

```rust,ignore
use eeg_visualization::*;

let service: Box<dyn EegVisualizer> = Box::new(VisualizationService::default());
let renderer: Box<dyn PlotRenderer> = Box::new(SvgRenderer::default());

let waveform = service.waveform(&recording, &WaveformRequest {
    start_seconds: 2.0,
    end_seconds: 6.0,
    channels: vec!["Fp1".into(), "Fp2".into()],
    max_points_per_channel: 1000,
})?;
let psd = service.psd(&features, &PsdRequest::default())?;
let map = service.topomap(
    &features,
    &ElectrodeLayout::schematic_10_20(),
    &TopomapRequest::default(),
)?;

let svg = renderer.render(&EegPlot::Waveform(waveform), SvgOptions::default())?;
// 文件保存、缓存及 Slint 图像加载由应用层负责。
std::fs::write("waveform.svg", svg)?;
```

默认 SVG 为 960×640，单通道面板至少 90 像素，支持最多 6 个曲线面板。
更多通道请增加高度或按视口选择通道（例如 19 通道可指定 960×1900）。
所有面板共用幅度范围，避免不同通道各自缩放产生比较误导。
SVG 自包含，无外部字体/脚本/网络资源。尺寸可为宽 320–8192、高 240–8192。
原始 ID、单位、轴范围、通道标签、floor 数量及地形图说明随图像保留。

应用层应将准备和渲染调度到后台线程，再通过事件把 SVG/DTO 交给 UI。
原始与预处理波形使用同一接口，但应用层需明确选择对应的录制。
将相应 `recording_id` 与绘图请求作为缓存键，避免显示其他录制的结果。

## 波形约定

- 显示窗口为 `[start, end)`，样本时间始终为原索引除以采样率。
  末端超出录制时截断；窗口无采样点返回 `EmptyRange`。
- 空通道列表按录制顺序选择所有 EEG；显式列表保留用户顺序，拒绝重复或未知通道。
  非 EEG 不会隐式进入波形图。
- `V`、`mV`、`uV`、`µV`、`μV` 仅在显示 DTO 中统一为 uV，未知单位报错。
- 小窗口保留所有样本，大窗口保留首尾并将内部样本分桶，各桶保留 min/max
  的原始索引，再按时间排序。点数不超过请求上限（至少 4）。桶大小相差至多 1。
  与等间隔抽点相比，该方法能保留桶内最强正负尖峰。
- 包络是显示降采样，不是滤波/重采样，不能用于后续科研或模型输入。
  同一桶内其他细节可能丢失，应缩小视窗检查；原始数组从不修改。
- 重新验证采样率、标签、形状、时长与全部输入有限值；NaN/Inf 会报错。
  缺失信号修复与质量筛选应由独立策略处理，图形模块不会伪造有效数据。

## PSD 约定

- 只读取 `EegFeatures.spectral`，保留原 P6 频率 bin，不执行 FFT、重新积分或插值。
  频率必须非负且严格递增，PSD 必须有限、非负且长度一致。
- 显示频率范围为闭区间 `[min_hz, max_hz]`，越界时只选择已有 bin，选不到返回错误。
- `Linear` 显示 uV²/Hz；`Decibel` 显示
  `10 * log10(max(PSD, floor) / (1 uV²/Hz))`。
  默认 floor 为 `1e-12 uV²/Hz`，为零功率提供有限显示值。
- 每条曲线同时保存 `raw_uv2_per_hz` 和 `floored_bins`，floor 不回写特征。
  图像显示 dB 参考单位和裁剪数量，避免把显示下限误读为原始功率。
- 未指定通道时使用领域 BTreeMap 排序；需要采集顺序时传入 P6
  `SpectralAudit.included_channels`。P6 的单位契约为 uV²/Hz 和 uV²；
  若 provenance 明确声明不同单位则拒绝，而非用错误坐标轴标注。

## 地形图约定

- 数据源为已计算的频段功率，可选择任一已有频段、绝对功率 uV² 或相对功率 ratio。
  不通过原始波形计算新空间特征。缺失频段报错，不补零。
- 自定义 `ElectrodeLayout` 使用归一化单位圆：x<0 为受试者左侧，y>0 为鼻尖方向，
  从头顶向下观察。SVG 同时绘制电极、标签、头圆和鼻尖。
- `schematic_10_20()` 是常用 19 导联的**二维示意坐标**，不是准确的标准三维投影、
  真实受试者头形或源定位。图像明确显示 `Schematic coordinates`。
  有实测空间数据时传入独立 layout，并由调用者完成相应投影。
- 精确匹配标签，**不自动猜测参考导联、别名或未知坐标**。
  默认缺失坐标报错。显式选择 `MissingPositionPolicy::Skip` 时，跳过标签保留在
  DTO、SVG 描述和图像提示中。P5 的坏通道可由应用层用 `channels` 显式筛除。
- 至少需要三个不同、非共线电极；重复标签/坐标、圆外坐标、非有限坐标均拒绝。
  共线判定采用归一化坐标下叉积 `1e-12` 容差。
- 固定 IDW(p=2)：`w_i = 1 / distance_i²`，按归一化权重求平均。
  查询位于电极 `1e-12` 距离内时取该电极原值；常量数据保持常量。
  算法采用距离和数值缩放，避免近电极权重或大功率求和溢出。
- 正方形网格按像素中心采样，`TopomapCoverage::ElectrodeHull`（库默认值）
  只在头圆和所选电极凸包内产生值，凸包外为 `None`/透明。
  `HeadCircle` 在整个头圆内进行 IDW 展示插值，圆外仍为 `None`；桌面应用默认使用此策略。
  覆盖策略随 DTO 传递，原始电极值与固定色标不变；旧 JSON 缺少该字段时兼容为凸包策略。
  单元格是离散显示近似，色块边缘精度由 resolution 决定。
  若分辨率过低、有效凸包过小导致全部像素为空，返回 `EmptyRange`。
- 色标取电极原始 min/max，常量图使用中间色且保留 min=max。
  蓝→青绿→黄表示数值大小，不表示临床风险。
  地形图是展示插值，不能解释为连接性、诊断结果或源定位。

## 预算、回放与错误

默认限制：256 通道/坐标、6400 万输入数值、100 万输出曲线点、262144 个网格单元、
1600 万次插值操作估算（单元数×电极数）。预算在大型分配前检查，整数乘法检查溢出。
曲线渲染器独立验证 DTO 数值、时间顺序、PSD 与原始功率的对应关系和网格掩膜，
避免反序列化或直接修改公开字段绕过输入验证。
范围、缺失数据、几何、预算和数值错误均可通过 `VisualizationError` 匹配。

显示 DTO 可用工程已有 serde_json 保存/回放。图形本身不生成新的分析 provenance，
来源 ID 和显示变换保存在 DTO，原分析参数与历史仍留在 P6 领域结果中。
插值/包络的实现版本为本 crate 版本 `0.1.0`；持久化缓存时应同时保存该版本。
固定 DTO、配置和版本生成确定性 SVG；不同平台浮点数值比对使用容差。

## 测试与示例

```bash
cargo test -p eeg-visualization --offline
cargo clippy -p eeg-visualization --all-targets --offline -- -D warnings
cargo fmt --all -- --check
cargo test -p domain -p eeg-io -p eeg-signal -p eeg-quality -p eeg-features -p eeg-visualization --offline
cargo run -p eeg-visualization --example render_eeg --offline -- /tmp/eeg-preview
```

示例生成 19 通道合成 EEG，输出 `waveform.svg`、`psd.svg`、`topomap.svg`，
可直接在浏览器查看。它不需要文件导入、数据库、桌面 UI 或 LLM。

测试包含手算时间窗口、正负尖峰与端点、单位等价、闭区间 PSD/dB/floor、
独立 IDW 对照、凸包掩膜、常量/大数值、未知电极与显式跳过、共线/重复坐标、
非法公开字段、资源预算、XML 转义、序列化回放、并发及完整 P3→P5→P4→P6→P7
工作流。工作流中的 10 Hz 峰和 `A²/2` 功率使用独立解析值验证。

**无需新增第三方 Cargo 包，也没有自动下载依赖。** 本 crate 复用工程已有版本；
Cargo Workspace 的 `crates/*` 自动发现该 crate，lockfile 只增加它的本地包记录。
未来添加 Plotters/像素后端时，可单独实现 `PlotRenderer`；届时所需的新包由你手动添加。
