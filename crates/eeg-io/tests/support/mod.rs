//! Independent synthetic file builders used for boundary and corruption tests.

#![allow(dead_code)]

use domain::{Channel, ChannelKind, RecordingId, SubjectId};
use eeg_io::{ImportContext, MatLayout, MatOptions, MatReader, MatSamplingRate};
use flate2::{Compression, write::ZlibEncoder};
use std::io::Write;

pub fn context() -> ImportContext {
    ImportContext::new(RecordingId::new(), SubjectId::new())
}

pub fn channels() -> Vec<Channel> {
    ["Fp1", "Fp2"]
        .into_iter()
        .map(|label| Channel::new(label, ChannelKind::Eeg, "uV").unwrap())
        .collect()
}

pub fn mat_reader() -> MatReader {
    MatReader::new(MatOptions {
        data_variable: "eeg".into(),
        sampling_rate: MatSamplingRate::Variable("fs".into()),
        layout: MatLayout::ChannelsBySamples,
        channels: channels(),
    })
}

pub fn put_field(bytes: &mut [u8], offset: usize, width: usize, text: &str) {
    assert!(text.len() <= width);
    bytes[offset..offset + width].fill(b' ');
    bytes[offset..offset + text.len()].copy_from_slice(text.as_bytes());
}

pub struct TestSignal<'a> {
    pub label: &'a str,
    pub unit: &'a str,
    pub physical_min: f64,
    pub physical_max: f64,
    pub digital_min: i32,
    pub digital_max: i32,
    pub count: usize,
}

impl<'a> TestSignal<'a> {
    pub fn eeg(label: &'a str, count: usize) -> Self {
        Self {
            label,
            count,
            unit: "uV",
            physical_min: -32768.0,
            physical_max: 32767.0,
            digital_min: -32768,
            digital_max: 32767,
        }
    }
}

pub fn edf(signals: &[TestSignal<'_>], records: i32, data: &[i16], reserved: &str) -> Vec<u8> {
    let n = signals.len();
    let mut bytes = vec![b' '; 256 + n * 256];
    for (offset, width, text) in [
        (0, 8, "0".into()),
        (168, 8, "01.01.26".into()),
        (176, 8, "00.00.00".into()),
        (184, 8, bytes.len().to_string()),
        (192, 44, reserved.into()),
        (236, 8, records.to_string()),
        (244, 8, "1".into()),
        (252, 4, n.to_string()),
    ] {
        put_field(&mut bytes, offset, width, &text);
    }
    for (index, s) in signals.iter().enumerate() {
        for (offset, width, text) in [
            (0, 16, s.label.into()),
            (96, 8, s.unit.into()),
            (104, 8, s.physical_min.to_string()),
            (112, 8, s.physical_max.to_string()),
            (120, 8, s.digital_min.to_string()),
            (128, 8, s.digital_max.to_string()),
            (216, 8, s.count.to_string()),
        ] {
            put_field(&mut bytes, 256 + n * offset + index * width, width, &text);
        }
    }
    for value in data {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

pub fn word(value: u32, little: bool) -> [u8; 4] {
    if little {
        value.to_le_bytes()
    } else {
        value.to_be_bytes()
    }
}

pub fn element(kind: u32, data: &[u8], little: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&word(kind, little));
    bytes.extend_from_slice(&word(data.len() as u32, little));
    bytes.extend_from_slice(data);
    if kind != 15 {
        bytes.resize(bytes.len().next_multiple_of(8), 0);
    }
    bytes
}

pub fn matrix(name: &str, dims: &[i32], values: &[f64], little: bool, complex: bool) -> Vec<u8> {
    let mut flags = Vec::from(word(6 | if complex { 0x800 } else { 0 }, little));
    flags.extend_from_slice(&word(0, little));
    let mut data = element(6, &flags, little);
    let dimensions: Vec<_> = dims.iter().flat_map(|&v| word(v as u32, little)).collect();
    data.extend(element(5, &dimensions, little));
    data.extend(element(1, name.as_bytes(), little));
    let real: Vec<_> = values
        .iter()
        .flat_map(|&v| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        })
        .collect();
    data.extend(element(9, &real, little));
    if complex {
        data.extend(element(9, &real, little));
    }
    element(14, &data, little)
}

pub fn mat(arrays: &[Vec<u8>], little: bool, compressed: bool) -> Vec<u8> {
    let mut bytes = vec![b' '; 116];
    let description = b"MATLAB 5.0 MAT-file,";
    bytes[..description.len()].copy_from_slice(description);
    bytes.extend([0; 8]);
    bytes.extend(if little {
        [0, 1, b'I', b'M']
    } else {
        [1, 0, b'M', b'I']
    });
    for array in arrays {
        if compressed {
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(array).unwrap();
            bytes.extend(element(15, &encoder.finish().unwrap(), little));
        } else {
            bytes.extend(array);
        }
    }
    bytes
}

pub fn data_mat(dims: &[i32], values: &[f64], little: bool, complex: bool) -> Vec<u8> {
    mat(
        &[
            matrix("eeg", dims, values, little, complex),
            matrix("fs", &[1, 1], &[2.0], little, false),
        ],
        little,
        false,
    )
}
