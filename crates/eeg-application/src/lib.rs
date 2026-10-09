//! P8 应用层：命令 → 工作流服务 → 不可变快照/事件。
//!
//! 本 crate 不依赖 Slint，不使用全局数据库或 UI 状态。可直接在无显示服务器的
//! 测试、CLI 或未来其他界面中运行；导入/DSP/质量/特征/绘图仍由 P2–P7 负责。
#![forbid(unsafe_code)]

mod controller;
mod engine;
mod error;
mod forms;
mod model;
mod rendering;
mod reporting;
mod service;

pub use controller::{AppController, AppEvent, CommandDispatcher};
pub use domain::{
    ChannelKind, FrequencyBand, RecordingId, RecordingMetadata, Sex, SignalQuality, SubjectId,
};
pub use eeg_features::FeatureConfig;
pub use eeg_quality::QualityConfig;
pub use eeg_signal::PipelineConfig;
// 桌面只消费这些 UI 无关的 DTO，绘图不必重新访问样本或计算特征。
pub use eeg_visualization::{
    BandPowerMeasure, PlotPoint, PsdPlot, PsdScale, TopomapCoverage, TopomapPlot, WaveformPlot,
};
pub use engine::{CoreEngine, EegEngine};
pub use error::{AppError, AppResult};
pub use forms::{ImportForm, ProcessingForm, SubjectForm};
pub use llm::{LlmError, NarrationInput, ReportNarrator};
pub use model::*;
pub use report::{ReportDocument, ReportService};
pub use reporting::build_v1_report_context;
pub use service::{ApplicationService, ProjectFactory, ProjectRepository, RedbProjectFactory};
