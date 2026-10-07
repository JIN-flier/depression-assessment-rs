use domain::{
    EegFeatures, EegRecording, FrequencyBand, RecordingId, Sex, SignalQuality, Subject, SubjectId,
};
use eeg_features::FeatureConfig;
use eeg_io::{CsvOptions, MatOptions};
use eeg_quality::QualityConfig;
use eeg_signal::PipelineConfig;
use std::{path::PathBuf, sync::Arc};
use storage::StoredRecordingMetadata;

#[derive(Debug, Clone)]
pub enum ImportFormat {
    Edf,
    Csv(CsvOptions),
    Mat(MatOptions),
}

#[derive(Debug, Clone)]
pub struct SubjectDraft {
    pub age: Option<u8>,
    pub sex: Sex,
    pub notes: String,
}

/// 图形配置仅影响显示；切换通道、时间窗、幅度或频段不会重跑 DSP/FFT。
#[derive(Debug, Clone, PartialEq)]
pub struct ViewRequest {
    /// None = 默认按录制顺序显示所有 EEG 通道；Some 则精确使用列表。
    /// Some(Vec::new()) = 全部取消勾选，不能与“显示所有”混淆。
    pub channels: Option<Vec<String>>,
    pub start_seconds: f64,
    pub window_seconds: f64,
    /// 每个波形面板的 ±uV 范围；超限部分裁剪，仅影响图像。
    pub amplitude_uv: f64,
    pub processed: bool,
    pub band: FrequencyBand,
    pub relative_power: bool,
}
impl Default for ViewRequest {
    fn default() -> Self {
        Self {
            channels: None,
            start_seconds: 0.0,
            window_seconds: 5.0,
            amplitude_uv: 100.0,
            processed: false,
            band: FrequencyBand::Alpha,
            relative_power: true,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct AnalysisRequest {
    pub pipeline: PipelineConfig,
    pub quality: QualityConfig,
    pub features: FeatureConfig,
    /// 默认不自主删除坏导联；勾选时把原始质量结果显式传给特征配置。
    pub exclude_bad_channels: bool,
}

/// P9 可直接消费此结构化边界。保留原始/处理后质量及所有 provenance，
/// 处理后的样本拥有独立 ID，保存它不会覆盖原始采集。
#[derive(Debug, Clone)]
pub struct AnalysisResult {
    pub raw_quality: SignalQuality,
    pub processed_quality: SignalQuality,
    pub processed: Arc<EegRecording>,
    pub features: EegFeatures,
    pub request: AnalysisRequest,
}

#[derive(Debug, Clone, Default)]
pub struct PlotData {
    /// 仅传递 P7 的物理量 DTO；应用层不依赖 Slint、像素或 SVG 后端。
    pub waveform: Option<eeg_visualization::WaveformPlot>,
    pub psd: Option<eeg_visualization::PsdPlot>,
    pub topomap: Option<eeg_visualization::TopomapPlot>,
    /// 地形图不可用不能导致已成功的特征/PSD 丢失。
    pub warnings: Vec<String>,
}

/// 小对象/Arc 快照跨线程传递，UI 不会克隆大型原始矩阵。
#[derive(Debug, Clone, Default)]
pub struct AppSnapshot {
    pub project_path: Option<PathBuf>,
    pub subjects: Vec<Subject>,
    pub selected_subject: Option<SubjectId>,
    pub recordings: Vec<StoredRecordingMetadata>,
    pub raw: Option<Arc<EegRecording>>,
    pub quality: Option<Arc<SignalQuality>>,
    pub analysis: Option<Arc<AnalysisResult>>,
    pub saved_processed: Option<RecordingId>,
    pub view: ViewRequest,
    pub plots: Arc<PlotData>,
}

/// 所有磁盘操作与计算都经此边界提交；UI 只读结果。
#[derive(Debug, Clone)]
pub enum AppCommand {
    OpenProject(PathBuf),
    CreateSubject(SubjectDraft),
    UpdateSubject {
        id: SubjectId,
        draft: SubjectDraft,
    },
    DeleteSubject(SubjectId),
    SelectSubject(SubjectId),
    ImportEeg {
        path: PathBuf,
        format: ImportFormat,
        metadata: domain::RecordingMetadata,
    },
    LoadRecording(RecordingId),
    DeleteRecording(RecordingId),
    AssessQuality(QualityConfig),
    Analyze(AnalysisRequest),
    SetView(ViewRequest),
    SaveProcessed,
}
