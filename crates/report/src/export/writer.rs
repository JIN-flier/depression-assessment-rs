//! 同目录临时文件 → 写完并 sync → hard_link 原子发布（不覆盖）。
//! create_new 和 hard_link 都在文件系统上判断冲突，没有 exists→write 的竞态。
//! 失败时 RAII 清理临时文件，已有目标绝不改变；不在 UI 线程执行。
use super::{ExportBundle, ExportError, ExportFormat, FontOptions, encode};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::PathBuf,
};

#[derive(Debug, Clone)]
pub struct ExportRequest {
    pub destination: PathBuf,
    pub format: ExportFormat,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportReceipt {
    pub destination: PathBuf,
    pub format: ExportFormat,
    pub bytes_written: u64,
}

pub trait ReportExporter: Send {
    fn export(
        &self,
        bundle: &ExportBundle,
        request: &ExportRequest,
    ) -> Result<ExportReceipt, ExportError>;
}
#[derive(Debug, Default)]
pub struct FileReportExporter {
    pub fonts: FontOptions,
}
impl ReportExporter for FileReportExporter {
    fn export(
        &self,
        bundle: &ExportBundle,
        request: &ExportRequest,
    ) -> Result<ExportReceipt, ExportError> {
        let path = request.format.destination(&request.destination)?;
        // 编码成功前不创建目标文件，缺报告、缺字体等错误不会留下空文件。
        let bytes = encode(bundle, request.format, &self.fonts)?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        let temp = parent.join(format!(".eeg-export-{}.tmp", domain::RecordingId::new()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&temp)?;
        let mut pending = TempFile {
            path: temp,
            file: Some(file),
        };
        // guard 拥有句柄，失败时也先关闭再删除，兼容 Windows 的文件共享规则。
        let file = pending
            .file
            .as_mut()
            .ok_or(ExportError::InvalidDestination)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(pending.file.take());
        fs::hard_link(&pending.path, &path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                ExportError::AlreadyExists
            } else {
                ExportError::Io(e)
            }
        })?;
        Ok(ExportReceipt {
            destination: path,
            format: request.format,
            bytes_written: bytes.len() as u64,
        })
    }
}
struct TempFile {
    path: PathBuf,
    file: Option<File>,
}
impl Drop for TempFile {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}
