//! RFC 4180 长表：每个 JSON 叶子一行，以 RFC 6901 pointer 保留层次和数组下标。
//! 同一 schema 包含 PSD、频段功率、两份质量、参数、溯源及可选正文。
//! 字符串防公式注入；数字仍是原始数值。科研端按 value_type 还原字符串时，
//! 根据 spreadsheet_escaped 列去掉额外添加的一个单引号即可，无需猜测原始内容。
use super::{ExportBundle, ExportError, MAX_OUTPUT_BYTES};
use serde_json::Value;

pub(super) fn encode(b: &ExportBundle) -> Result<Vec<u8>, ExportError> {
    let value = serde_json::to_value(b).map_err(ExportError::Serialization)?;
    // BOM 让 Excel 正确识别中文 UTF-8。CRLF 和总是加双引号统一处理逗号/换行。
    let mut out = String::from(
        "\u{feff}schema_version,subject_id,recording_id,path,value,value_type,unit,spreadsheet_escaped\r\n",
    );
    let prefix = [
        b.schema_version.to_string(),
        b.subject.id.to_string(),
        b.analysis.processed.id.to_string(),
    ];
    flatten(&value, "", &prefix, &mut out)?;
    Ok(out.into_bytes())
}
fn flatten(
    v: &Value,
    path: &str,
    prefix: &[String; 3],
    out: &mut String,
) -> Result<(), ExportError> {
    match v {
        Value::Object(map) if !map.is_empty() => {
            for (key, v) in map {
                let key = key.replace('~', "~0").replace('/', "~1");
                flatten(v, &format!("{path}/{key}"), prefix, out)?;
            }
        }
        Value::Array(array) if !array.is_empty() => {
            for (index, v) in array.iter().enumerate() {
                flatten(v, &format!("{path}/{index}"), prefix, out)?;
            }
        }
        _ => {
            let (text, kind) = match v {
                Value::String(s) => (s.clone(), "string"),
                Value::Number(n) => (n.to_string(), "number"),
                Value::Bool(b) => (b.to_string(), "boolean"),
                Value::Null => ("null".into(), "null"),
                Value::Array(_) => ("[]".into(), "array"),
                Value::Object(_) => ("{}".into(), "object"),
            };
            let unit = if path.contains("/psd_by_channel/") {
                "uV^2/Hz"
            } else if path.contains("/frequencies_hz/") || path.ends_with("/sampling_rate_hz") {
                "Hz"
            } else if path.contains("/band_power_by_channel/") && path.ends_with("/absolute") {
                "uV^2"
            } else if path.ends_with("/relative") || path.ends_with("/overall_score") {
                "ratio"
            } else if path.ends_with("/duration_seconds") {
                "s"
            } else {
                ""
            };
            let escaped = if kind == "string" && dangerous(&text) {
                "true"
            } else {
                "false"
            };
            let fields = [
                &prefix[0], &prefix[1], &prefix[2], path, &text, kind, unit, escaped,
            ];
            for (i, field) in fields.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push('"');
                if (i != 4 || kind == "string") && dangerous(field) {
                    out.push('\'');
                }
                out.push_str(&field.replace('"', "\"\""));
                out.push('"');
            }
            out.push_str("\r\n");
            if out.len() > MAX_OUTPUT_BYTES {
                return Err(ExportError::TooLarge);
            }
        }
    }
    Ok(())
}
fn dangerous(s: &str) -> bool {
    s.trim_start_matches(|c: char| c.is_whitespace() || c.is_control())
        .starts_with(['=', '+', '-', '@'])
}
