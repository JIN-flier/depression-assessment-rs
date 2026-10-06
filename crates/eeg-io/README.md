# P3：EEG 文件导入

本 crate 实现方案第 30 节的 P3：EDF / MAT / CSV → `domain::EegRecording`。
保留 P1 的 `samples[channel][sample]`、`Vec<Vec<f32>>` 数据契约，P2 存储可直接接收输出。

## 模块边界

```text
Application 提供匿名 ID、采集元数据和格式选项
    ↓
EegReader（可注入、Send + Sync、支持 dyn trait）
    ├── CsvReader：CSV 列映射、数值读取
    ├── EdfReader：EDF 头部、数据记录、ADC 校准
    └── MatReader：MAT 数值变量、列主序与矩阵方向
    ↓
common：资源限制、采样率/通道校验、单位转换
    ↓
domain::EegRecording
    ↓
Application 可选择保存到 RecordingRepository 或交给后续 P4/P5
```

运行时依赖只有 domain、csv、matfile、flate2、thiserror；storage 仅为测试依赖。
没有数据库访问、UI 回调、滤波、质量评估、模型或 LLM 逻辑。新格式只需实现 `EegReader`。
EDF 的固定字段编解码直接按官方规范实现，校准和长度校验可独立测试；MAT 的数值解码使用 `matfile`，文件完整性和有界解压在适配层完成。

`read` 接受任意 `Read`，可用于内存、文件及其他输入流；`read_path` 提供文件便利入口。
接口是同步的，P8 接入桌面时应放在后台线程或 `spawn_blocking`，避免阻塞 Slint 主线程。
应用应根据用户选择创建适配器，因为 CSV/MAT 无统一 EEG 元数据规范，单凭扩展名无法确定采样率、通道或矩阵方向。

## 使用方式

```rust
use domain::{RecordingId, SubjectId};
use eeg_io::{CsvOptions, CsvReader, EegReader, ImportContext};

let context = ImportContext::new(RecordingId::new(), SubjectId::new());
let reader = CsvReader::new(CsvOptions::new(250.0));
let recording = reader.read_path(std::path::Path::new("recording.csv"), &context)?;
// 应用可以独立执行 repository.create_recording(&recording)。
```

CSV 默认使用逗号、首行通道名称、每行一个采样时刻、幅值单位 `uV`：

```csv
Fp1,Fp2
1.5,-2.0
3.0,-4.5
```

采样率必须显式提供。可配置分隔符、是否存在表头、每列的 `Channel` 描述和默认单位。
无表头时必须配置通道；同时配置通道和表头时，名称和顺序必须一致。
时间戳、注释列不会自动丢弃：输入应为等间隔通道样本矩阵，其他 CSV schema 需通过额外适配器显式处理。
支持标准引号、UTF-8 BOM、CRLF、空白和科学计数法；空样本、缺失值、NaN/Inf、超出 f32 范围和不等长行返回错误。

MAT 示例：

```rust
use domain::{Channel, ChannelKind};
use eeg_io::{MatLayout, MatOptions, MatReader, MatSamplingRate};

let reader = MatReader::new(MatOptions {
    data_variable: "eeg".into(),
    sampling_rate: MatSamplingRate::Variable("fs".into()),
    layout: MatLayout::ChannelsBySamples,
    channels: vec![
        Channel::new("Fp1", ChannelKind::Eeg, "uV")?,
        Channel::new("Fp2", ChannelKind::Eeg, "uV")?,
    ],
});
```

支持 MATLAB Level-5（常见 `-v6` / `-v7`）实数二维数值矩阵，大/小端、zlib 压缩与未压缩元素混合、small data element，以及所有整数/浮点数值类型。
`ChannelsBySamples` 对应 `[channel, sample]`；`SamplesByChannels` 对应 `[sample, channel]`。
适配器按 MATLAB 列主序读取，不通过维度大小猜测方向。
采样率可用 `Hertz(250.0)`，也可从同文件的实数 `1×1` 数值变量中读取。
通道名称、类型、单位由调用方提供；struct/cell/char/sparse 辅助变量不作为 EEG 数值变量读取。
重复变量、复数、三维数据、缺失变量、维度不匹配、损坏尾部会报错。
MAT v4、v7.3/HDF5 和 EEGLAB struct schema 不在此适配器的支持范围，可导出独立的实数二维矩阵为 `-v7` 后导入。

`matfile 0.5.0` 对 signed int32 的类型映射存在问题：适配层仅在验证后的临时副本中将声明类型提升到 int64，使库执行保留符号的无损扩展。
原始文件不修改；专门测试压缩 signed int32 的负数、大端和小端输入。

EDF 示例：

```rust
use eeg_io::EdfReader;

let all_ordinary_signals = EdfReader::default();
let selected = EdfReader {
    channel_indices: Some(vec![0, 2]),
    ..Default::default()
};
```

支持 EDF 和连续 EDF+C，按有符号小端 int16 读取，每个数据记录分别追加到各通道。
每通道独立校准：

```text
physical = physical_min
         + (digital - digital_min)
         × (physical_max - physical_min) / (digital_max - digital_min)
```

支持偏置、增益、负物理斜率、记录数 `-1` 时从完整数据记录推导数量。
检查头部长度、记录大小、声明记录数、ADC 范围、重复通道和非有限校准参数。
默认保留所有普通信号；`channel_indices` 指定源信号索引及输出顺序，`channel_kinds` 可覆盖通道类型推断。
若选中信号采样率不同，返回明确错误；调用方可选择具有相同采样率的子集。
EDF+C 的时间标记必须连续；`EDF Annotations` 通道不加入样本矩阵，省略数量和首记录时间偏移记入元数据，事件正文不导入。
不支持 EDF+D 或 BDF：当前领域模型没有断续时间轴，也不使用 24 位 EDF 样本。不会在 P3 静默拼接断续记录或自动重采样。

## 统一输出与追溯

- EEG 统一转为 `uV`；支持 `V`、`mV`、`uV`、`µV`、`μV`、`nV`。未知 EEG 单位报错。
- 非 EEG 的非电压单位（例如 trigger `code`、温度）按原值保留；可识别的电压单位统一转为 `uV`。
- 输出保持 `RecordingState::Raw`，processing history 为空。物理校准属于文件解码，P4 的滤波/重采样尚未发生。
- recording/subject ID、device 等来自 `ImportContext`；不从文件中的身份字段推导或复制患者信息。
- `source_format`、导入器版本、源单位、CSV schema、MAT 变量/方向和 EDF 校准/prefilter 均记录在 metadata 中。
- EDF 时间没有时区：原始日期/时间保存在 `edf.start_date_local`、`edf.start_time_local`，不擅自转换成 UTC；调用方可通过 `metadata.recorded_at` 提供已确认的时间。
- 错误使用 `EegIoError` 区分 IO、配置、损坏数据、未支持格式、资源限制及领域校验；生产代码没有 `unwrap`/`expect`。

默认输入/解压后的 MAT 字节上限为 256 MiB，源/输出通道最多 1024，每通道最多 1000 万样本，总样本最多 6400 万。
可在 `context.limits` 中调整。每个适配器在建立输出矩阵前检查维度；CSV 在每行追加前检查。
当前导入器将有界输入放入内存，实际峰值还包括解析副本、数值数组与输出矩阵；这些限制并非进程内存的精确上限。

## 测试与验收

```bash
cargo test -p eeg-io
cargo test --workspace
cargo clippy -p eeg-io --all-targets -- -D warnings
cargo fmt --all -- --check
```

`tests/csv_import.rs`、`edf_import.rs`、`mat_import.rs` 覆盖各格式正常数值、校准、矩阵方向、配置错误、损坏输入和资源限制。
`tests/import_workflow.rs` 验证 dyn reader 接口、同一信号跨三种格式的数值一致性、文件入口和 P2 存储重开后的完整恢复。
固定样本位于 `tests/data`，全部为匿名合成数据：2 通道 × 4 样本，采样率 2 Hz，期望数据为 `[1,2,3,4]` 和 `[-1,-2,-3,-4]`。
EDF、未压缩 MAT、压缩 MAT、CSV 均随仓库保存，测试无需 Python。
可选开发脚本 `python3 crates/eeg-io/tests/data/generate_fixtures.py` 仅用于按格式规范重新生成样本，不属于应用运行时。

格式参考：[EDF 官方规范](https://www.edfplus.info/specs/edf.html)、[EDF+ 官方规范](https://www.edfplus.info/specs/edfplus.html)、[matfile 数值接口](https://docs.rs/matfile/0.5.0/matfile/enum.NumericData.html)。
