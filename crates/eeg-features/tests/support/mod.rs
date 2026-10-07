#![allow(dead_code)]
use chrono::{TimeZone, Utc};
use domain::{
    Channel, ChannelKind, EegRecording, RecordingId, RecordingMetadata, RecordingState, SubjectId,
};
use eeg_features::{FeatureConfig, FeatureContext, SpectralExtractor};
use std::f64::consts::TAU;

pub fn context() -> FeatureContext {
    let mut context = FeatureContext::new(Utc.with_ymd_and_hms(2026, 10, 7, 0, 0, 0).unwrap());
    context.software_version = "feature-test-app".into();
    context
}

pub fn recording(rate: f64, rows: Vec<Vec<f32>>) -> EegRecording {
    let channels = (0..rows.len())
        .map(|n| Channel::new(format!("Ch{n}"), ChannelKind::Eeg, "uV").unwrap())
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

pub fn extractor() -> SpectralExtractor {
    SpectralExtractor::new(FeatureConfig::default()).unwrap()
}

pub fn close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "actual {actual}, expected {expected}, tolerance {tolerance}"
    );
}
