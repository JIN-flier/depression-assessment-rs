//! EDF's fixed-width header and little-endian, signed 16-bit data codec.
//!
//! The codec follows https://www.edfplus.info/specs/edf.html. Parsing here keeps
//! calibration, exact record-length validation, and multi-rate rejection visible
//! and independently testable, without coupling the domain to a file library.

use crate::{EegIoError, EegIoResult, EegReader, ImportContext, common::*, error::invalid};
use domain::{Channel, ChannelKind, EegRecording};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
};

/// EDF/continuous EDF+ reader. Annotation channels are excluded from samples.
/// Discontinuous EDF+D and 24-bit BDF cannot fit the current uniformly sampled
/// domain model and are explicitly rejected. No implicit resampling occurs.
#[derive(Debug, Clone, Default)]
pub struct EdfReader {
    /// Source signal indices, in desired output order. `None` imports every
    /// ordinary signal. Select a common-rate subset for multi-rate EDF files.
    pub channel_indices: Option<Vec<usize>>,
    /// Explicit acquisition knowledge can override conservative kind inference.
    pub channel_kinds: BTreeMap<usize, ChannelKind>,
}

struct Signal {
    channel: Channel,
    factor: f64,
    physical_min: f64,
    physical_max: f64,
    digital_min: i32,
    digital_max: i32,
    count: usize,
    annotation: bool,
    prefilter: String,
    source_unit: String,
}

impl EegReader for EdfReader {
    fn read(&self, input: &mut dyn Read, context: &ImportContext) -> EegIoResult<EegRecording> {
        let bytes = bounded_bytes(input, context.limits.max_input_bytes)?;
        if bytes.len() < 256 {
            return Err(invalid("EDF", "truncated fixed header"));
        }
        if field(&bytes, 0, 8)? != "0" {
            return Err(EegIoError::Unsupported(
                "EDF version (BDF is not EDF)".into(),
            ));
        }
        let reserved = field(&bytes, 192, 44)?;
        if reserved.starts_with("EDF+D") {
            return Err(EegIoError::Unsupported(
                "discontinuous EDF+D requires a segmented time axis".into(),
            ));
        }
        let continuous_plus = reserved.starts_with("EDF+C");
        let ns: usize = number(&bytes, 252, 4)?;
        if ns == 0 {
            return Err(invalid("EDF", "no signals"));
        }
        if ns > context.limits.max_channels {
            return Err(EegIoError::LimitExceeded("source channel count"));
        }
        let header_size: usize = number(&bytes, 184, 8)?;
        if header_size != 256 + ns * 256 || bytes.len() < header_size {
            return Err(invalid("EDF", "incorrect or truncated signal header"));
        }
        let declared_records: i64 = number(&bytes, 236, 8)?;
        if declared_records < -1 {
            return Err(invalid("EDF", "invalid record count"));
        }
        let duration: f64 = number(&bytes, 244, 8)?;
        if !duration.is_finite() || duration <= 0.0 {
            return Err(invalid(
                "EDF",
                "record duration must be positive and finite",
            ));
        }

        let mut signals = Vec::with_capacity(ns);
        for index in 0..ns {
            // EDF is field-major, not a sequence of 256-byte channel structs.
            let get = |offset, width| field(&bytes, 256 + ns * offset + index * width, width);
            let label = get(0, 16)?;
            let annotation = label == "EDF Annotations";
            let unit = get(96, 8)?;
            let count: usize = get(216, 8)?
                .parse()
                .map_err(|_| invalid("EDF", "invalid samples per record"))?;
            if count == 0 {
                return Err(invalid("EDF", "zero samples per record"));
            }
            // Annotation calibration fields are not meaningful in EDF+ and can
            // be blank. They must never be treated as physiological values.
            let (channel, factor, pmin, pmax, dmin, dmax) = if annotation {
                (
                    Channel::new(label, ChannelKind::Other, "annotation")?,
                    1.0,
                    0.0,
                    1.0,
                    0,
                    1,
                )
            } else {
                let kind = self
                    .channel_kinds
                    .get(&index)
                    .cloned()
                    .unwrap_or_else(|| infer_kind(label, unit));
                let original = Channel::new(label, kind, unit)?;
                // Only selected signals need compatible EEG voltage units.
                let selected = self
                    .channel_indices
                    .as_ref()
                    .is_none_or(|indices| indices.contains(&index));
                let (channel, factor) = if selected {
                    canonical_channel(&original)?
                } else {
                    (original, 1.0)
                };
                let parse_f = |offset| -> EegIoResult<f64> {
                    get(offset, 8)?
                        .parse()
                        .map_err(|_| invalid("EDF", "invalid physical calibration"))
                };
                let parse_i = |offset| -> EegIoResult<i32> {
                    get(offset, 8)?
                        .parse()
                        .map_err(|_| invalid("EDF", "invalid digital calibration"))
                };
                let (pmin, pmax, dmin, dmax) =
                    (parse_f(104)?, parse_f(112)?, parse_i(120)?, parse_i(128)?);
                // Negative physical slope is valid (polarity inversion); equal
                // extrema, non-finite values, and invalid ADC limits are not.
                if !pmin.is_finite()
                    || !pmax.is_finite()
                    || pmin == pmax
                    || dmin >= dmax
                    || dmin < -32768
                    || dmax > 32767
                {
                    return Err(invalid("EDF", format!("invalid calibration for {label}")));
                }
                (channel, factor, pmin, pmax, dmin, dmax)
            };
            signals.push(Signal {
                channel,
                factor,
                physical_min: pmin,
                physical_max: pmax,
                digital_min: dmin,
                digital_max: dmax,
                count,
                annotation,
                prefilter: get(136, 80)?.into(),
                source_unit: unit.into(),
            });
        }
        let indices = self
            .channel_indices
            .clone()
            .unwrap_or_else(|| (0..ns).filter(|&i| !signals[i].annotation).collect());
        if indices.is_empty() {
            return Err(invalid("EDF", "no ordinary signals selected"));
        }
        let mut unique = BTreeSet::new();
        for &index in &indices {
            if index >= ns || signals[index].annotation || !unique.insert(index) {
                return Err(EegIoError::Configuration(
                    "invalid, duplicate, or annotation signal selection".into(),
                ));
            }
        }
        if self
            .channel_kinds
            .keys()
            .any(|&i| i >= ns || signals[i].annotation)
        {
            return Err(EegIoError::Configuration(
                "invalid signal kind override".into(),
            ));
        }
        let samples_per_record = signals[indices[0]].count;
        if indices
            .iter()
            .any(|&i| signals[i].count != samples_per_record)
        {
            return Err(EegIoError::Unsupported(
                "selected EDF signals have different sampling rates; select a common-rate subset"
                    .into(),
            ));
        }
        let record_bytes = signals
            .iter()
            .try_fold(0usize, |total, s| {
                total.checked_add(s.count.checked_mul(2)?)
            })
            .ok_or(EegIoError::LimitExceeded("EDF record size overflow"))?;
        let payload = &bytes[header_size..];
        if payload.len() % record_bytes != 0 {
            return Err(invalid("EDF", "truncated data record or trailing bytes"));
        }
        let records = payload.len() / record_bytes;
        if declared_records != -1 && declared_records as u64 != records as u64 {
            return Err(invalid(
                "EDF",
                "declared record count does not match payload",
            ));
        }
        let sample_count = records
            .checked_mul(samples_per_record)
            .ok_or(EegIoError::LimitExceeded("EDF sample count overflow"))?;
        context.limits.shape(indices.len(), sample_count)?;
        let channels: Vec<_> = indices
            .iter()
            .map(|&i| signals[i].channel.clone())
            .collect();
        validate_channels(&channels)?;
        let mut samples = vec![Vec::with_capacity(sample_count); indices.len()];
        let mut offsets = Vec::with_capacity(ns);
        let mut offset = 0;
        for signal in &signals {
            offsets.push(offset);
            offset += signal.count * 2;
        }
        let annotation_index = signals.iter().position(|s| s.annotation);
        if continuous_plus && annotation_index.is_none() {
            return Err(invalid("EDF", "EDF+C lacks timekeeping annotations"));
        }
        let mut first_onset = 0.0;
        for (record_index, record) in payload.chunks_exact(record_bytes).enumerate() {
            if continuous_plus && let Some(i) = annotation_index {
                let annotation = &record[offsets[i]..offsets[i] + signals[i].count * 2];
                let onset_end = annotation
                    .iter()
                    .position(|&b| b == 0x14)
                    .ok_or_else(|| invalid("EDF", "missing timekeeping annotation"))?;
                let text = std::str::from_utf8(&annotation[..onset_end])
                    .map_err(|_| invalid("EDF", "invalid annotation onset"))?;
                if !text.starts_with('+') || annotation.get(onset_end + 1) != Some(&0x14) {
                    return Err(invalid("EDF", "invalid empty timekeeping TAL"));
                }
                let onset: f64 = text
                    .parse()
                    .map_err(|_| invalid("EDF", "invalid annotation onset"))?;
                if record_index == 0 {
                    first_onset = onset;
                }
                if !onset.is_finite()
                    || !(0.0..1.0).contains(&first_onset)
                    || (onset - (first_onset + record_index as f64 * duration)).abs() > 1e-6
                {
                    return Err(invalid("EDF", "EDF+C timekeeping is not contiguous"));
                }
            }
            for (output, &index) in indices.iter().enumerate() {
                let signal = &signals[index];
                let data = &record[offsets[index]..offsets[index] + signal.count * 2];
                for pair in data.as_chunks::<2>().0 {
                    let digital = i16::from_le_bytes([pair[0], pair[1]]) as i32;
                    if digital < signal.digital_min || digital > signal.digital_max {
                        return Err(invalid(
                            "EDF",
                            format!("sample outside ADC range in {}", signal.channel.label),
                        ));
                    }
                    let physical = signal.physical_min
                        + (digital - signal.digital_min) as f64
                            * (signal.physical_max - signal.physical_min)
                            / (signal.digital_max - signal.digital_min) as f64;
                    samples[output].push(sample(
                        physical,
                        signal.factor,
                        "EDF",
                        &signal.channel.label,
                    )?);
                }
            }
        }
        let mut metadata = context.metadata.clone();
        // EDF acquisition time is local without a timezone. Preserve it as such
        // rather than invent a UTC timestamp. Identifying header text is omitted.
        metadata
            .extra
            .insert("edf.start_date_local".into(), field(&bytes, 168, 8)?.into());
        metadata
            .extra
            .insert("edf.start_time_local".into(), field(&bytes, 176, 8)?.into());
        metadata.extra.insert(
            "edf.first_record_offset_seconds".into(),
            first_onset.to_string(),
        );
        metadata.extra.insert(
            "edf.annotation_signals_omitted".into(),
            signals.iter().filter(|s| s.annotation).count().to_string(),
        );
        for (output, &index) in indices.iter().enumerate() {
            metadata.extra.insert(
                format!("edf.channel.{output}.source_index"),
                index.to_string(),
            );
            metadata.extra.insert(
                format!("edf.channel.{output}.prefilter"),
                signals[index].prefilter.clone(),
            );
            metadata.extra.insert(
                format!("eeg_io.channel.{output}.source_unit"),
                signals[index].source_unit.clone(),
            );
            for (name, value) in [
                ("physical_min", signals[index].physical_min),
                ("physical_max", signals[index].physical_max),
                ("digital_min", signals[index].digital_min as f64),
                ("digital_max", signals[index].digital_max as f64),
            ] {
                metadata
                    .extra
                    .insert(format!("edf.channel.{output}.{name}"), value.to_string());
            }
        }
        finish(
            context,
            "edf",
            samples_per_record as f64 / duration,
            channels,
            samples,
            metadata,
        )
    }
}

fn infer_kind(label: &str, unit: &str) -> ChannelKind {
    let label = label.to_ascii_uppercase();
    if label.starts_with("EOG") {
        ChannelKind::Eog
    } else if label.starts_with("ECG") || label.starts_with("EKG") {
        ChannelKind::Ecg
    } else if label.starts_with("EMG") {
        ChannelKind::Emg
    } else if label.starts_with("TRIG") || label.starts_with("STATUS") {
        ChannelKind::Trigger
    } else if label.starts_with("EEG")
        || electrode_label(&label)
        || matches!(unit, "V" | "mV" | "uV" | "nV")
    {
        ChannelKind::Eeg
    } else {
        ChannelKind::Other
    }
}

fn electrode_label(label: &str) -> bool {
    let electrode = label.split('-').next().unwrap_or(label).trim();
    [
        "FP", "AF", "FC", "FT", "CP", "TP", "PO", "F", "C", "T", "P", "O", "A", "M",
    ]
    .iter()
    .any(|prefix| {
        electrode.strip_prefix(prefix).is_some_and(|suffix| {
            suffix == "Z" || (!suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()))
        })
    })
}

fn field(bytes: &[u8], offset: usize, width: usize) -> EegIoResult<&str> {
    let data = bytes
        .get(offset..offset + width)
        .ok_or_else(|| invalid("EDF", "truncated header field"))?;
    if !data.iter().all(|b| (32..=126).contains(b)) {
        return Err(invalid("EDF", "header field is not printable ASCII"));
    }
    std::str::from_utf8(data)
        .map(str::trim)
        .map_err(|_| invalid("EDF", "invalid header encoding"))
}

fn number<T: std::str::FromStr>(bytes: &[u8], offset: usize, width: usize) -> EegIoResult<T> {
    field(bytes, offset, width)?
        .parse()
        .map_err(|_| invalid("EDF", format!("invalid numeric header at byte {offset}")))
}
