#![allow(dead_code)]
use chrono::{TimeZone, Utc};
use domain::{
    Channel, ChannelKind, EegRecording, RecordingId, RecordingMetadata, RecordingState, SubjectId,
};
use eeg_quality::{QualityAnalyzer, QualityConfig, QualityContext};
use std::f64::consts::TAU;

pub fn context() -> QualityContext {
    let mut context = QualityContext::new(Utc.with_ymd_and_hms(2026, 10, 6, 0, 0, 0).unwrap());
    context.software_version = "quality-test-application".into();
    context
}
pub fn recording(rate: f64, rows: Vec<Vec<f32>>) -> EegRecording {
    let channels = (0..rows.len())
        .map(|i| Channel::new(format!("Ch{i}"), ChannelKind::Eeg, "uV").unwrap())
        .collect();
    EegRecording::new(
        RecordingId::new(),
        SubjectId::new(),
        rate,
        channels,
        rows,
        RecordingState::Raw,
        RecordingMetadata::default(),
    )
    .unwrap()
}
pub fn sine(rate: f64, frequency: f64, amplitude: f64, count: usize) -> Vec<f32> {
    (0..count)
        .map(|n| (amplitude * (TAU * frequency * n as f64 / rate).sin()) as f32)
        .collect()
}
pub fn analyzer() -> QualityAnalyzer {
    QualityAnalyzer::new(QualityConfig::default()).unwrap()
}
pub fn time_only() -> QualityConfig {
    let mut config = QualityConfig::default();
    config.spectral.line_frequency_hz = None;
    config.spectral.high_frequency_start_hz = None;
    config
}
pub fn has_warning(result: &domain::ChannelQuality, code: &str) -> bool {
    result.warnings.iter().any(|warning| warning.code == code)
}
pub fn close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() < tolerance,
        "actual {actual}, expected {expected}, tolerance {tolerance}"
    );
}
