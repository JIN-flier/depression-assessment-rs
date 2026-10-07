//! 服务适配器：应用只编排顺序，算法实现仍由各独立 crate 拥有。
use crate::*;
use domain::{EegRecording, SignalQuality};
use eeg_features::{FeatureContext, SpectralExtractor};
use eeg_io::{CsvReader, EdfReader, EegReader, ImportContext, MatReader};
use eeg_quality::{QualityAnalyzer, QualityContext};
use eeg_signal::{ProcessingContext, SignalPipeline};
use std::{path::Path, sync::Arc, time::SystemTime};

/// 可注入慢任务/失败替身或替换整个分析策略，不要求 UI 知道算法/数据库。
pub trait EegEngine: Send {
    fn import(
        &self,
        path: &Path,
        format: &ImportFormat,
        context: &ImportContext,
    ) -> AppResult<EegRecording>;
    fn quality(&self, input: &EegRecording, config: &QualityConfig) -> AppResult<SignalQuality>;
    fn analyze(
        &self,
        input: &EegRecording,
        request: &AnalysisRequest,
        progress: &mut dyn FnMut(&'static str),
    ) -> AppResult<AnalysisResult>;
    fn plots(
        &self,
        raw: &EegRecording,
        analysis: Option<&AnalysisResult>,
        view: &ViewRequest,
    ) -> AppResult<PlotData>;
}

#[derive(Default)]
pub struct CoreEngine;
impl EegEngine for CoreEngine {
    fn import(
        &self,
        path: &Path,
        format: &ImportFormat,
        context: &ImportContext,
    ) -> AppResult<EegRecording> {
        let reader: Box<dyn EegReader> = match format {
            ImportFormat::Edf => Box::new(EdfReader::default()),
            ImportFormat::Csv(options) => Box::new(CsvReader::new(options.clone())),
            ImportFormat::Mat(options) => Box::new(MatReader::new(options.clone())),
        };
        reader
            .read_path(path, context)
            .map_err(|e| AppError::service("EEG 导入", e))
    }
    fn quality(&self, input: &EegRecording, config: &QualityConfig) -> AppResult<SignalQuality> {
        let analyzer =
            QualityAnalyzer::new(config.clone()).map_err(|e| AppError::service("质量配置", e))?;
        analyzer
            .assess(input, &QualityContext::new(SystemTime::now().into()))
            .map_err(|e| AppError::service("质量评价", e))
    }
    fn analyze(
        &self,
        input: &EegRecording,
        request: &AnalysisRequest,
        progress: &mut dyn FnMut(&'static str),
    ) -> AppResult<AnalysisResult> {
        progress("原始信号质量评价");
        let raw_quality = self.quality(input, &request.quality)?;
        progress("预处理：DC / Bandpass / Notch / Resample");
        let pipeline = SignalPipeline::new(request.pipeline.clone())
            .map_err(|e| AppError::service("预处理配置", e))?;
        let mut processed = pipeline
            .process(input, &ProcessingContext::new(SystemTime::now().into()))
            .map_err(|e| AppError::service("预处理", e))?;
        // P4 不拥有实体身份；在应用层明确原始/派生身份，保护存储里的原始数据。
        processed.id = RecordingId::new();
        processed
            .metadata
            .extra
            .insert("source_recording_id".into(), input.id.to_string());
        progress("处理后信号质量评价");
        let processed_quality = self.quality(&processed, &request.quality)?;
        let mut features_config = request.features.clone();
        if request.exclude_bad_channels {
            for label in &raw_quality.bad_channels {
                if !features_config.excluded_channels.contains(label) {
                    features_config.excluded_channels.push(label.clone());
                }
            }
        }
        progress("Welch PSD / 频段绝对与相对功率");
        let extractor = SpectralExtractor::new(features_config)
            .map_err(|e| AppError::service("特征配置", e))?;
        let features = extractor
            .extract(&processed, &FeatureContext::new(SystemTime::now().into()))
            .map_err(|e| AppError::service("特征提取", e))?;
        Ok(AnalysisResult {
            raw_quality,
            processed_quality,
            processed: Arc::new(processed),
            features,
            request: request.clone(),
        })
    }
    fn plots(
        &self,
        raw: &EegRecording,
        analysis: Option<&AnalysisResult>,
        view: &ViewRequest,
    ) -> AppResult<PlotData> {
        crate::rendering::render(raw, analysis, view)
    }
}
