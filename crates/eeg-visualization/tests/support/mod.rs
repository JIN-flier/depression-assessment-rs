#![allow(dead_code)]
use chrono::{TimeZone, Utc};
use domain::*;
use std::collections::BTreeMap;

pub fn recording(rate: f64, rows: &[(&str, Vec<f32>)]) -> EegRecording {
    EegRecording::new(
        RecordingId::new(),
        SubjectId::new(),
        rate,
        rows.iter()
            .map(|(label, _)| Channel::new(*label, ChannelKind::Eeg, "uV").unwrap())
            .collect(),
        rows.iter().map(|(_, row)| row.clone()).collect(),
        RecordingState::Raw,
        RecordingMetadata::default(),
    )
    .unwrap()
}

pub fn features() -> EegFeatures {
    let psd = BTreeMap::from([
        ("Fp1".into(), vec![0.0, 1.0, 10.0, 100.0]),
        ("Fp2".into(), vec![1.0, 2.0, 3.0, 4.0]),
        ("Cz".into(), vec![2.0; 4]),
    ]);
    let bands = [("Fp1", 2.0, 0.2), ("Fp2", 6.0, 0.6), ("Cz", 10.0, 1.0)]
        .into_iter()
        .map(|(label, absolute, relative)| {
            (
                label.into(),
                BTreeMap::from([(
                    FrequencyBand::Alpha,
                    BandPower {
                        absolute,
                        relative: UnitInterval::new("test", relative).unwrap(),
                    },
                )]),
            )
        })
        .collect();
    EegFeatures::spectral(
        RecordingId::new(),
        SpectralFeatures::new(vec![0.0, 1.0, 10.0, 20.0], psd, bands).unwrap(),
        Provenance::new(
            "test",
            "fixture",
            "1",
            Utc.with_ymd_and_hms(2026, 10, 7, 0, 0, 0).unwrap(),
        )
        .unwrap(),
    )
}

pub fn triangle() -> eeg_visualization::ElectrodeLayout {
    use eeg_visualization::*;
    ElectrodeLayout {
        name: "Measured test triangle".into(),
        schematic: false,
        positions: vec![
            ElectrodePosition {
                channel: "Fp1".into(),
                x: -0.5,
                y: 0.0,
            },
            ElectrodePosition {
                channel: "Fp2".into(),
                x: 0.5,
                y: 0.0,
            },
            ElectrodePosition {
                channel: "Cz".into(),
                x: 0.0,
                y: 0.5,
            },
        ],
    }
}

pub fn sine(rate: f64, frequency: f64, amplitude: f64, count: usize) -> Vec<f32> {
    (0..count)
        .map(|i| (amplitude * (std::f64::consts::TAU * frequency * i as f64 / rate).sin()) as f32)
        .collect()
}

pub fn close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() <= 1e-10 * expected.abs().max(1.0),
        "{actual} vs {expected}"
    );
}
