//! MATLAB Level-5 numeric matrix adapter. File syntax stays in `matfile`;
//! orientation, resource limits, and conversion stay in this import boundary.

use crate::{EegIoError, EegIoResult, EegReader, ImportContext, common::*, error::invalid};
use domain::{Channel, EegRecording};
use flate2::read::ZlibDecoder;
use matfile::{MatFile, NumericData};
use std::io::Read;

#[derive(Debug, Clone, Copy)]
pub enum MatLayout {
    ChannelsBySamples,
    SamplesByChannels,
}

#[derive(Debug, Clone)]
pub enum MatSamplingRate {
    Hertz(f64),
    /// A real numeric 1x1 variable in the same MAT file.
    Variable(String),
}

#[derive(Debug, Clone)]
pub struct MatOptions {
    pub data_variable: String,
    pub sampling_rate: MatSamplingRate,
    pub layout: MatLayout,
    /// Explicit labels, types, and source units, in matrix channel order.
    /// Level-5 strings/structs have no universal EEG metadata schema.
    pub channels: Vec<Channel>,
}

#[derive(Debug, Clone)]
pub struct MatReader {
    pub options: MatOptions,
}

impl MatReader {
    pub fn new(options: MatOptions) -> Self {
        Self { options }
    }
}

impl EegReader for MatReader {
    fn read(&self, input: &mut dyn Read, context: &ImportContext) -> EegIoResult<EegRecording> {
        let options = &self.options;
        if options.data_variable.trim().is_empty() {
            return Err(EegIoError::Configuration(
                "MAT data variable must be specified".into(),
            ));
        }
        validate_channels(&options.channels)?;
        context.limits.shape(options.channels.len(), 1)?;
        let bytes = bounded_bytes(input, context.limits.max_input_bytes)?;
        let (normalized, numeric_count) = prepare_mat(&bytes, context.limits.max_input_bytes)?;
        let file = MatFile::parse(normalized.as_slice())?;
        if file.arrays().len() != numeric_count {
            return Err(invalid("MAT", "numeric matrix could not be fully decoded"));
        }
        let variable = |name: &str| -> EegIoResult<&matfile::Array> {
            let mut matches = file.arrays().iter().filter(|array| array.name() == name);
            let array = matches
                .next()
                .ok_or_else(|| invalid("MAT", format!("missing real numeric variable {name:?}")))?;
            if matches.next().is_some() {
                return Err(invalid("MAT", format!("duplicate variable {name:?}")));
            }
            Ok(array)
        };
        let rate = match &options.sampling_rate {
            MatSamplingRate::Hertz(rate) => *rate,
            MatSamplingRate::Variable(name) => {
                let array = variable(name)?;
                if array.size() != &[1, 1] {
                    return Err(invalid(
                        "MAT",
                        "sampling rate variable must be a 1x1 scalar",
                    ));
                }
                numeric_values(array.data())?
                    .next()
                    .ok_or_else(|| invalid("MAT", "empty sampling rate"))?
            }
        };
        sampling_rate(rate)?;
        let array = variable(&options.data_variable)?;
        if array.size().len() != 2 {
            return Err(EegIoError::Unsupported(
                "MAT EEG data must be a two-dimensional numeric matrix".into(),
            ));
        }
        let rows = array.size()[0];
        let columns = array.size()[1];
        let (channel_count, sample_count) = match options.layout {
            MatLayout::ChannelsBySamples => (rows, columns),
            MatLayout::SamplesByChannels => (columns, rows),
        };
        if channel_count != options.channels.len() {
            return Err(invalid(
                "MAT",
                "configured channels do not match the matrix channel dimension",
            ));
        }
        context.limits.shape(channel_count, sample_count)?;
        let canonical = options
            .channels
            .iter()
            .map(canonical_channel)
            .collect::<EegIoResult<Vec<_>>>()?;
        let values = numeric_values(array.data())?;
        if values.len() != channel_count * sample_count {
            return Err(invalid(
                "MAT",
                "matrix dimensions do not match numeric payload",
            ));
        }
        let mut samples = vec![vec![0.0; sample_count]; channel_count];
        for (index, value) in values.enumerate() {
            // MATLAB is column-major: the first dimension varies fastest.
            // A non-square matrix test protects both paths against transposition.
            let (channel, instant) = match options.layout {
                MatLayout::ChannelsBySamples => (index % rows, index / rows),
                MatLayout::SamplesByChannels => (index / rows, index % rows),
            };
            samples[channel][instant] = sample(
                value,
                canonical[channel].1,
                "MAT",
                format!("channel {channel}, sample {instant}"),
            )?;
        }
        let mut metadata = context.metadata.clone();
        source_channel_metadata(&mut metadata, &options.channels);
        metadata
            .extra
            .insert("mat.data_variable".into(), options.data_variable.clone());
        metadata
            .extra
            .insert("mat.layout".into(), format!("{:?}", options.layout));
        finish(
            context,
            "mat",
            rate,
            canonical.into_iter().map(|(c, _)| c).collect(),
            samples,
            metadata,
        )
    }
}

/// Borrow numeric values without another full matrix allocation. Complex input
/// is rejected even when its imaginary values are all zero; EEG is real-valued.
fn numeric_values(data: &NumericData) -> EegIoResult<Box<dyn ExactSizeIterator<Item = f64> + '_>> {
    macro_rules! values {
        ($($variant:ident),+) => {
            match data {
                $(NumericData::$variant { real, imag } => {
                    if imag.is_some() { return Err(EegIoError::Unsupported("complex MAT EEG data".into())); }
                    Ok(Box::new(real.iter().map(|&value| value as f64)))
                }),+
            }
        };
    }
    values!(
        Int8, UInt8, Int16, UInt16, Int32, UInt32, Int64, UInt64, Single, Double
    )
}

/// `matfile` accepts a parsed prefix and decompresses without a budget. Validate
/// complete top-level framing and inflate with a budget before handing it only
/// ordinary matrix elements. This prevents both truncated-tail acceptance and
/// compressed expansion beyond the caller's limit. Nested compression is invalid.
fn prepare_mat(bytes: &[u8], limit: usize) -> EegIoResult<(Vec<u8>, usize)> {
    if bytes.starts_with(b"\x89HDF") || bytes.starts_with(b"MATLAB 7.3") {
        return Err(EegIoError::Unsupported(
            "MAT v7.3/HDF5; export MATLAB -v7 or -v6".into(),
        ));
    }
    if bytes.len() < 128 {
        return Err(invalid(
            "MAT",
            "truncated Level-5 header (v4 is unsupported)",
        ));
    }
    let little = match &bytes[126..128] {
        b"IM" => true,
        b"MI" => false,
        _ => {
            return Err(EegIoError::Unsupported(
                "MAT format; expected Level-5 v6/v7 numeric arrays".into(),
            ));
        }
    };
    let version = if little {
        u16::from_le_bytes([bytes[124], bytes[125]])
    } else {
        u16::from_be_bytes([bytes[124], bytes[125]])
    };
    if version != 0x100 {
        return Err(EegIoError::Unsupported("MAT header version".into()));
    }
    let mut result = bytes[..128].to_vec();
    let mut numeric_count = 0;
    let mut remaining = &bytes[128..];
    while !remaining.is_empty() {
        let tag = element(remaining, little)?;
        if tag.kind == 15 {
            let budget = limit.saturating_sub(result.len());
            let mut decoder = ZlibDecoder::new(tag.data);
            let expanded = bounded_bytes(&mut decoder, budget)?;
            if decoder.total_in() != tag.data.len() as u64 {
                return Err(invalid("MAT", "trailing compressed bytes"));
            }
            let inner = element(&expanded, little)?;
            if inner.kind != 14 || inner.consumed != expanded.len() {
                return Err(invalid(
                    "MAT",
                    "compressed element must contain exactly one matrix",
                ));
            }
            if validate_matrix(inner.data, little)? {
                let offset = result.len();
                result.extend_from_slice(&expanded);
                promote_int32_class(&mut result[offset..], little);
                numeric_count += 1;
            }
        } else if tag.kind == 14 {
            let numeric = validate_matrix(tag.data, little)?;
            if result
                .len()
                .checked_add(tag.consumed)
                .is_none_or(|n| n > limit)
            {
                return Err(EegIoError::LimitExceeded("decompressed MAT bytes"));
            }
            if numeric {
                let offset = result.len();
                result.extend_from_slice(&remaining[..tag.consumed]);
                promote_int32_class(&mut result[offset..], little);
                numeric_count += 1;
            }
        } else {
            return Err(invalid("MAT", "unexpected top-level data element"));
        }
        remaining = &remaining[tag.consumed..];
    }
    Ok((result, numeric_count))
}

/// matfile 0.5.0's `ArrayType::numeric_data_type` maps mxINT32_CLASS to miUINT32
/// (upstream src/parse.rs), rejecting normal signed int32 payloads. Promote only
/// the declared numeric class to int64 in the already validated temporary copy;
/// matfile then performs a lossless signed widening, preserving negative values.
/// Source bytes and floating-point arrays are untouched. This workaround belongs
/// here at the dependency boundary, never in domain or downstream algorithms.
fn promote_int32_class(matrix: &mut [u8], little: bool) {
    let flags = word(&matrix[16..20], little);
    if flags & 0xff == 12 {
        let widened = (flags & !0xff) | 14;
        let bytes = if little {
            widened.to_le_bytes()
        } else {
            widened.to_be_bytes()
        };
        matrix[16..20].copy_from_slice(&bytes);
    }
}

struct Element<'a> {
    kind: u32,
    data: &'a [u8],
    consumed: usize,
}

fn word(bytes: &[u8], little: bool) -> u32 {
    let data = [bytes[0], bytes[1], bytes[2], bytes[3]];
    if little {
        u32::from_le_bytes(data)
    } else {
        u32::from_be_bytes(data)
    }
}

/// Level-5 elements use either an 8-byte regular tag or an 8-byte packed small
/// element (1..4 bytes). Compressed elements are uniquely not padded to 8 bytes.
fn element(bytes: &[u8], little: bool) -> EegIoResult<Element<'_>> {
    if bytes.len() < 8 {
        return Err(invalid("MAT", "truncated element tag"));
    }
    let first = word(bytes, little);
    let small_len = (first >> 16) as usize;
    if small_len > 0 {
        if small_len > 4 {
            return Err(invalid("MAT", "invalid small element length"));
        }
        return Ok(Element {
            kind: first & 0xffff,
            data: &bytes[4..4 + small_len],
            consumed: 8,
        });
    }
    let len = word(&bytes[4..8], little) as usize;
    let padded = if first == 15 {
        len
    } else {
        len.checked_add(7)
            .ok_or_else(|| invalid("MAT", "element length overflow"))?
            / 8
            * 8
    };
    let consumed = 8usize
        .checked_add(padded)
        .ok_or_else(|| invalid("MAT", "element length overflow"))?;
    if consumed > bytes.len() {
        return Err(invalid("MAT", "truncated element payload/padding"));
    }
    Ok(Element {
        kind: first,
        data: &bytes[8..8 + len],
        consumed,
    })
}

fn validate_matrix(bytes: &[u8], little: bool) -> EegIoResult<bool> {
    let flags = element(bytes, little)?;
    if flags.kind != 6 || flags.data.len() != 8 {
        return Err(invalid("MAT", "invalid array flags"));
    }
    let class = word(flags.data, little) & 0xff;
    if !(1..=15).contains(&class) {
        return Err(invalid("MAT", "invalid matrix class"));
    }
    let dims = element(&bytes[flags.consumed..], little)?;
    if dims.kind != 5 || dims.data.len() < 8 || dims.data.len() % 4 != 0 {
        return Err(invalid("MAT", "invalid matrix dimensions"));
    }
    let mut count = 1usize;
    for dimension in dims.data.as_chunks::<4>().0 {
        let dimension = word(dimension, little) as i32;
        if dimension < 0 {
            return Err(invalid("MAT", "negative matrix dimension"));
        }
        count = count
            .checked_mul(dimension as usize)
            .ok_or_else(|| invalid("MAT", "dimension product overflow"))?;
        // matfile multiplies dimensions in i32. Bound each intermediate product
        // as well, including malicious dimensions ending in a zero dimension.
        if count > i32::MAX as usize {
            return Err(invalid(
                "MAT",
                "dimension product exceeds Level-5 parser range",
            ));
        }
    }
    let mut offset = flags.consumed + dims.consumed;
    let name = element(&bytes[offset..], little)?;
    if !matches!(name.kind, 1 | 16) {
        return Err(invalid("MAT", "invalid variable name"));
    }
    offset += name.consumed;
    // Struct/cell/char/sparse auxiliary variables are omitted from EEG parsing.
    // Numeric matrices must have complete, dimension-consistent payloads so
    // matfile cannot silently skip a malformed numeric array.
    if (6..=15).contains(&class) {
        let complex = word(flags.data, little) & 0x800 != 0;
        for _ in 0..if complex { 2 } else { 1 } {
            let values = element(&bytes[offset..], little)?;
            let width = match values.kind {
                1 | 2 => 1,
                3 | 4 => 2,
                5..=7 => 4,
                9 | 12 | 13 => 8,
                _ => return Err(invalid("MAT", "invalid numeric element type")),
            };
            if count.checked_mul(width) != Some(values.data.len()) {
                return Err(invalid("MAT", "numeric payload does not match dimensions"));
            }
            offset += values.consumed;
        }
        if offset != bytes.len() {
            return Err(invalid("MAT", "trailing matrix bytes"));
        }
    }
    Ok((6..=15).contains(&class))
}
