//! CSV schema: each row is a sample instant, each column is one channel.

use crate::{EegIoError, EegIoResult, EegReader, ImportContext, common::*, error::invalid};
use domain::{Channel, ChannelKind, EegRecording};
use std::io::Read;

#[derive(Debug, Clone)]
pub struct CsvOptions {
    /// Required because CSV has no standard sampling-rate metadata.
    pub sampling_rate_hz: f64,
    pub delimiter: u8,
    pub has_headers: bool,
    /// Empty means headers define EEG labels with amplitudes in `unit`.
    /// Headerless data requires one descriptor per column. Supplied labels must
    /// match headers, preventing accidental channel reordering.
    pub channels: Vec<Channel>,
    pub unit: String,
}

impl CsvOptions {
    pub fn new(sampling_rate_hz: f64) -> Self {
        Self {
            sampling_rate_hz,
            delimiter: b',',
            has_headers: true,
            channels: vec![],
            unit: "uV".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CsvReader {
    pub options: CsvOptions,
}

impl CsvReader {
    pub fn new(options: CsvOptions) -> Self {
        Self { options }
    }
}

impl EegReader for CsvReader {
    fn read(&self, input: &mut dyn Read, context: &ImportContext) -> EegIoResult<EegRecording> {
        let options = &self.options;
        sampling_rate(options.sampling_rate_hz)?;
        if matches!(options.delimiter, 0 | b'\r' | b'\n' | b'"') {
            return Err(EegIoError::Configuration("invalid CSV delimiter".into()));
        }
        let bytes = bounded_bytes(input, context.limits.max_input_bytes)?;
        let mut reader = csv::ReaderBuilder::new()
            .delimiter(options.delimiter)
            .has_headers(options.has_headers)
            .trim(csv::Trim::All)
            .from_reader(bytes.as_slice());
        let mut channels = options.channels.clone();
        if options.has_headers {
            let headers = reader.headers()?;
            if channels.is_empty() {
                channels = headers
                    .iter()
                    .map(|label| Channel::new(label, ChannelKind::Eeg, &options.unit))
                    .collect::<Result<_, _>>()?;
            } else if headers.len() != channels.len()
                || headers
                    .iter()
                    .zip(&channels)
                    .any(|(label, c)| label != c.label.trim())
            {
                return Err(invalid(
                    "CSV",
                    "headers do not match configured channel order",
                ));
            }
        } else if channels.is_empty() {
            return Err(EegIoError::Configuration(
                "headerless CSV requires channel descriptors".into(),
            ));
        }
        validate_channels(&channels)?;
        context.limits.shape(channels.len(), 1)?;
        let canonical = channels
            .iter()
            .map(canonical_channel)
            .collect::<EegIoResult<Vec<_>>>()?;
        let mut samples = vec![Vec::new(); channels.len()];
        for (row, record) in reader.records().enumerate() {
            let record = record?;
            if record.len() != channels.len() {
                return Err(invalid(
                    "CSV",
                    format!("wrong column count at row {}", row + 1),
                ));
            }
            context.limits.shape(channels.len(), row + 1)?;
            for (column, text) in record.iter().enumerate() {
                let location = format!("data row {}, channel {}", row + 1, channels[column].label);
                let value: f64 = text
                    .parse()
                    .map_err(|_| invalid("CSV", format!("invalid numeric sample at {location}")))?;
                samples[column].push(sample(value, canonical[column].1, "CSV", location)?);
            }
        }
        let mut metadata = context.metadata.clone();
        source_channel_metadata(&mut metadata, &channels);
        metadata
            .extra
            .insert("csv.delimiter".into(), options.delimiter.to_string());
        metadata
            .extra
            .insert("csv.has_headers".into(), options.has_headers.to_string());
        finish(
            context,
            "csv",
            options.sampling_rate_hz,
            canonical.into_iter().map(|(c, _)| c).collect(),
            samples,
            metadata,
        )
    }
}
