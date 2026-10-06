#![allow(dead_code)]
use chrono::{TimeZone, Utc};
use domain::{
    Channel, ChannelKind, EegRecording, RecordingId, RecordingMetadata, RecordingState, SubjectId,
};
use eeg_signal::{PipelineConfig, ProcessingContext, SignalPipeline, SignalStep};
use std::f64::consts::TAU;

pub fn context() -> ProcessingContext {
    let mut context = ProcessingContext::new(Utc.with_ymd_and_hms(2026, 10, 6, 0, 0, 0).unwrap());
    context.software_version = "test-application-1.0".into();
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
pub fn pipeline(steps: Vec<SignalStep>) -> SignalPipeline {
    SignalPipeline::new(PipelineConfig {
        steps,
        ..PipelineConfig::default()
    })
    .unwrap()
}
pub fn sine(rate: f64, frequency: f64, count: usize) -> Vec<f32> {
    (0..count)
        .map(|n| (TAU * frequency * n as f64 / rate).sin() as f32)
        .collect()
}
/// Independent sinusoidal projection: measures gain/phase at a known frequency,
/// without sharing FFT/filter/resampler code with the implementation under test.
pub fn component(row: &[f32], rate: f64, frequency: f64, start: usize, end: usize) -> (f64, f64) {
    let (mut sin, mut cos) = (0.0, 0.0);
    for (n, &value) in row.iter().enumerate().take(end).skip(start) {
        let angle = TAU * frequency * n as f64 / rate;
        sin += f64::from(value) * angle.sin();
        cos += f64::from(value) * angle.cos();
    }
    let scale = 2.0 / (end - start) as f64;
    (sin * scale, cos * scale)
}
pub fn amplitude(row: &[f32], rate: f64, frequency: f64, start: usize, end: usize) -> f64 {
    let (sin, cos) = component(row, rate, frequency, start, end);
    sin.hypot(cos)
}
