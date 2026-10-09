//! P10 会话→导出适配器。仅投影成功分析，绝不读取样本、重跑算法或调用 LLM。
//! report 的归档 DTO 不依赖本 crate；算法配置通过通用 Serialize 合同传入。
use crate::{AppError, AppResult, AppSnapshot};
use report::export::{AnalysisArchive, AnalysisParameters, ExportBundle, RecordingDescriptor};

pub fn build_export_bundle(snapshot: &AppSnapshot) -> AppResult<ExportBundle> {
    let raw = snapshot
        .raw
        .as_deref()
        .ok_or(AppError::InvalidState("请先加载 EEG 录制"))?;
    let analysis = snapshot
        .analysis
        .as_deref()
        .ok_or(AppError::InvalidState("请先完成 EEG 分析"))?;
    let subject = snapshot
        .subjects
        .iter()
        .find(|s| Some(s.id) == snapshot.selected_subject)
        .ok_or(AppError::InvalidState("请先选择受试者"))?;
    let parameters = AnalysisParameters::new(
        &analysis.request.pipeline,
        &analysis.request.quality,
        &analysis.request.features,
        analysis.request.exclude_bad_channels,
    )
    .map_err(|e| AppError::service("准备导出参数", e))?;
    let archive = AnalysisArchive {
        raw: RecordingDescriptor::from(raw),
        processed: RecordingDescriptor::from(analysis.processed.as_ref()),
        raw_quality: analysis.raw_quality.clone(),
        processed_quality: analysis.processed_quality.clone(),
        features: analysis.features.clone(),
        parameters,
    };
    ExportBundle::new(subject.clone(), archive, snapshot.report.as_deref())
        .map_err(|e| AppError::service("准备导出归档", e))
}
