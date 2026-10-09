//! P10 导出边界。只消费已经计算的领域结果，不依赖 UI、数据库、EEG 算法或网络。
//!
//! JSON/CSV 是有版本的完整本地归档；PDF/DOCX 使用同一份已校验正文和指标附录。
//! 编码和落盘分离，宿主可以替换 ReportExporter，也可以只 encode 到内存。
mod archive;
mod csv;
mod docx;
mod font;
mod pdf;
mod writer;

pub use archive::{AnalysisArchive, AnalysisParameters, ExportBundle, RecordingDescriptor};
use std::{error::Error, fmt, path::Path};
pub use writer::{ExportReceipt, ExportRequest, FileReportExporter, ReportExporter};

/// 输出格式由命令显式决定，不能根据用户文件名悄悄改变内容。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Pdf,
    Docx,
    Json,
    Csv,
}
impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Docx => "docx",
            Self::Json => "json",
            Self::Csv => "csv",
        }
    }
    pub fn needs_narrative(self) -> bool {
        matches!(self, Self::Pdf | Self::Docx)
    }
    /// 补齐省略的扩展名；已有但不匹配的扩展名拒绝，避免覆盖源 EEG CSV。
    pub fn destination(self, path: &Path) -> Result<std::path::PathBuf, ExportError> {
        if path.file_name().is_none() || path.as_os_str().is_empty() {
            return Err(ExportError::InvalidDestination);
        }
        match path.extension() {
            None => Ok(path.with_extension(self.extension())),
            Some(ext) if ext.eq_ignore_ascii_case(self.extension()) => Ok(path.to_owned()),
            _ => Err(ExportError::InvalidDestination),
        }
    }
}

/// UTF-8 数据与 OOXML 不需要额外包。PDF 使用完整嵌入的 TrueType 中文字体，
/// FontOptions 可指定字体文件进行确定性测试，桌面宿主默认从本地环境加载。
#[derive(Debug, Clone, Default)]
pub struct FontOptions {
    pub pdf_font: Option<std::path::PathBuf>,
}

/// 内存编码不会访问目标文件；任何格式错误发生在发布文件之前。
pub fn encode(
    bundle: &ExportBundle,
    format: ExportFormat,
    fonts: &FontOptions,
) -> Result<Vec<u8>, ExportError> {
    bundle.validate()?;
    if format.needs_narrative() && bundle.report.is_none() {
        return Err(ExportError::MissingReport);
    }
    let bytes = match format {
        ExportFormat::Json => {
            serde_json::to_vec_pretty(bundle).map_err(ExportError::Serialization)?
        }
        ExportFormat::Csv => csv::encode(bundle)?,
        ExportFormat::Docx => docx::encode(&archive::paragraphs(bundle)?)?,
        ExportFormat::Pdf => pdf::encode(&archive::paragraphs(bundle)?, fonts)?,
    };
    if bytes.len() > MAX_OUTPUT_BYTES {
        return Err(ExportError::TooLarge);
    }
    Ok(bytes)
}
pub(crate) const MAX_OUTPUT_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug)]
pub enum ExportError {
    InvalidDestination,
    AlreadyExists,
    InvalidArchive,
    MissingReport,
    TooLarge,
    FontUnavailable,
    InvalidFont,
    MissingGlyph(char),
    InvalidText,
    Serialization(serde_json::Error),
    Io(std::io::Error),
}
impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDestination => f.write_str("请选择文件名及与导出格式一致的扩展名"),
            Self::AlreadyExists => {
                f.write_str("目标文件已存在，请选择新的文件名；导出不会覆盖已有文件")
            }
            Self::InvalidArchive => f.write_str("导出结果的实体关联、指标或结构不一致"),
            Self::MissingReport => f.write_str("PDF / DOCX 导出需要先生成已校验的报告"),
            Self::TooLarge => f.write_str("导出内容超过大小限制"),
            Self::FontUnavailable => f.write_str(
                "未找到 PDF 中文 TrueType 字体，请设置 EEG_REPORT_FONT 为可嵌入的 .ttf 文件",
            ),
            Self::InvalidFont => {
                f.write_str("PDF 字体无效或不支持嵌入，请使用可嵌入的 TrueType .ttf 字体")
            }
            Self::MissingGlyph(ch) => write!(
                f,
                "PDF 字体缺少字符 U+{:04X}，请更换 EEG_REPORT_FONT",
                *ch as u32
            ),
            Self::InvalidText => f.write_str("文档包含无法显示的控制字符"),
            Self::Serialization(e) => write!(f, "归档编码失败：{e}"),
            Self::Io(e) => write!(f, "导出文件操作失败：{e}"),
        }
    }
}
impl Error for ExportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Serialization(e) => Some(e),
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}
impl From<std::io::Error> for ExportError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
