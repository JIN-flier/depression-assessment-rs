# P8–P10：EEG 与报告应用层

本 crate 将 P2–P7 接成可测试的 V1 EEG 工作流。它不依赖 Slint，UI 不能绕过
命令边界访问数据库或算法。P9 自然语言报告和 P10 四格式文件导出均已接入。

```text
Slint callback → AppCommand → AppController（有界后台 worker）
                                    ↓
                             ApplicationService
                    ┌───────────────┴────────────────┐
              ProjectFactory / Repository        EegEngine
                  P2 storage                 P3/P4/P5/P6/P7
                    └───────────────┬────────────────┘
                                AppEvent
                                    ↓
                             AppSnapshot → UI
```

## 模块边界

- `model.rs`：命令、显示请求、结构化分析结果、不可变快照。
- `forms.rs`：共享表单解析；错误数字不会静默变成默认值。
- `service.rs`：受试者/录制管理、状态切换、持久化与工作流编排。
- `engine.rs`：独立的导入、质量、预处理、特征服务适配器；可注入替身。
- `rendering.rs`：调用 P7 准备波形/PSD/地形图 DTO；不依赖 SVG 或 Slint 像素后端。
  桌面 `raster.rs` 负责直接绘制 SharedPixelBuffer，其他宿主可使用自己的渲染器。
- `controller.rs`：单个后台 worker、非阻塞提交、阶段事件、重复点击与关闭保护。
- `reporting.rs`：既有分析→ReportContext；匿名通道与实体关联校验，不重算 EEG。
- `error.rs`：应用错误和底层 source 链。

`ProjectFactory`、`ProjectRepository`、`EegEngine`、`ReportNarrator` 都是可替换接口。数据库、算法
和 UI 没有反向依赖；应用服务可以直接在 CLI 或测试中执行。

## 状态与数据规则

1. 打开项目 → 新建/选择受试者 → 导入/加载录制 → 独立质量评价或完整分析。
2. 完整分析执行原始质量 → 预处理 → 处理后质量 → Welch PSD/频段功率 → 显示。
3. 分析始终从当前加载录制出发，不对上次分析输出重复滤波。
4. 派生录制拥有独立 ID 和 `source_recording_id`，保存不会覆盖原始采集；重复保存幂等。
5. 切换受试者、录制或项目清除旧结果；计算/显示失败不发布部分候选分析。
6. 时间窗、通道、幅度、显示频段/功率种类变化只重建图形，不执行 DSP 或 FFT。
   `ViewRequest.channels` 为 `None` 时选择所有 EEG，`Some(vec![])` 精确隐藏全部；
    非空 `Some` 列表保留传入顺序。8 通道是桌面视区的行数，而非应用层选择上限。
   波形显示 DTO 每通道最多保留 8192 个包络点（不超过 P7 总点数预算）；
   地形图展示网格为 256×256，采样/插值不改变特征功率或原始样本。
7. 默认保留坏导联。勾选排除时只从特征提取排除原始质量判定的坏导联，实际选择写入 P6 provenance。
8. 地形图使用全部参与分析的导联、19 导联二维示意坐标；未知位置显式跳过并提示。
   缺少至少三个不共线的已知电极等问题不会阻断特征或 PSD。图形不是源定位。
9. 原始/派生采样数据与采集元信息保存到 P2。质量、特征与结构化分析快照当前保存在
   会话内存中；关闭/切换后重新分析。P10 可以通过 JSON / CSV 导出完整结果归档。
10. `GenerateReport` 只消费同一次成功分析的原始/处理后质量与特征，经 `report` 校验后
    原子发布文档。Provider/校验失败保留上次有效报告；更换数据、成功重分析、质量评价
    或修改当前受试者资料清除文档。视图与保存派生录制不会清除报告。

可用 `ApplicationService::with_report_service` 注入任意 Provider。网络配置只在后台首次生成时
初始化，Key 不经过命令或快照。详见 [LLM 依赖配置](../llm/README.md) 和 [报告合同](../report/README.md)。

控制器将大型样本与分析结果以 `Arc` 传递，UI 不复制矩阵。任务串行执行，忙碌到
终态事件被消费才解除，防止过期快照覆盖下一操作。销毁控制器后拒绝残留 dispatcher
提交，并唤醒空闲 worker；进行中的算法完成后退出，不在 UI 主线程 join。

## 依赖与验证

应用层只连接本地模块；测试复用已有 serde_json。可选 SDK 与运行时依赖由 `openai` 功能启用。

```bash
cargo test -p eeg-application --offline
cargo clippy -p eeg-application --all-targets --offline -- -D warnings
```

测试覆盖真实 redb + CSV 工作流、EDF/MAT/CSV 统一命令边界、已知 10 Hz 信号的
Alpha 主功率、降采样与处理历史、原始/派生持久化、状态清理、错误恢复、显示不重算、
未知电极降级、过短信号、表单错误，以及注入阻塞任务证明控制器提交不阻塞 UI。
`tests/reporting.rs` 另外覆盖真实分析到报告、输入隐私、实体关联、失败/重试、报告失效、
报告生成不重算/不修改 EEG，以及阻塞 Provider 下非阻塞提交与 Busy 门控。

## P10 导出边界

`ExportReport(ExportRequest)` 与其他任务共用后台 worker、Busy 门控和终态事件。
`exporting.rs` 把当前 subject / raw / 成功分析映射成无样本 `ExportBundle`，
不执行算法、数据库查询或网络请求。JSON / CSV 在分析后即可导出；PDF / DOCX
另外要求当前已校验报告。可通过 `with_report_exporter` 替换文件后端，测试不需
依赖 UI 或文件选择器。输出合同与字体配置见 [report/README.md](../report/README.md)。

只有文件完整发布后才更新 `last_export` 回执，格式错误、字体错误、磁盘错误或
已有文件冲突保留原始样本、分析、正文、绘图和旧回执；重试可选择新文件名。
切换数据或成功重新分析会清除回执，单纯改变视图或保存派生录制保留它。
应用层无需添加 Cargo 包。

`tests/exporting.rs` 覆盖四格式真实文件、完整 JSON 指标/参数等值、CSV 公式转义、
陈旧正文与跨实体拒绝、缺字体/无报告失败、写入失败清理、并发禁止覆盖、权限、
成功重试，以及注入阻塞 exporter 证明调度不会阻塞 UI。
