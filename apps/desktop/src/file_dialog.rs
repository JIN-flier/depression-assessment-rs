//! 系统文件弹窗适配器：与 Slint、EEG 算法和存储完全解耦。
//!
//! 系统弹窗的阻塞等待运行在独立线程。使用 Command 参数/环境变量传值，
//! 不把用户路径拼成 shell 脚本；选中、取消、启动失败是三种不同结果。
#[cfg(target_os = "linux")]
use std::io;
use std::{
    path::PathBuf,
    process::{Command, Output},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender, TryRecvError},
    },
    thread,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogPurpose {
    ProjectDirectory,
    EegFile,
    /// 保存格式随请求固定；UI 修改下拉框不能改变已打开弹窗的任务。
    Export(eeg_application::ExportFormat),
}

#[derive(Debug, Clone)]
pub struct DialogRequest {
    pub purpose: DialogPurpose,
    pub initial_directory: Option<PathBuf>,
}

/// None 表示用户取消，不能被当成错误或空路径提交给应用服务。
/// 测试注入此接口即可覆盖实际 UI 回调，不需要弹出真实 OS 窗口。
pub trait NativeFileDialog: Send + Sync {
    fn select(&self, request: &DialogRequest) -> Result<Option<PathBuf>, String>;
}

#[derive(Default)]
pub struct SystemFileDialog;
impl NativeFileDialog for SystemFileDialog {
    fn select(&self, request: &DialogRequest) -> Result<Option<PathBuf>, String> {
        system_select(request)
    }
}

#[derive(Debug)]
pub struct DialogEvent {
    pub purpose: DialogPurpose,
    pub result: Result<Option<PathBuf>, String>,
}

/// 独立于 EEG worker 的调度边界。直到 UI 消费终态前都保持 pending，
/// 从而避免快速点击同时打开多个系统弹窗。
pub struct DialogController {
    provider: Arc<dyn NativeFileDialog>,
    sender: Sender<DialogEvent>,
    events: Receiver<DialogEvent>,
    pending: Arc<AtomicBool>,
}
impl DialogController {
    pub fn new(provider: Arc<dyn NativeFileDialog>) -> Self {
        let (sender, events) = mpsc::channel();
        Self {
            provider,
            sender,
            events,
            pending: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn request(&self, request: DialogRequest) -> Result<(), String> {
        if self
            .pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err("文件选择弹窗已打开".into());
        }
        let provider = self.provider.clone();
        let sender = self.sender.clone();
        if let Err(error) = thread::Builder::new()
            .name("native-file-dialog".into())
            .spawn(move || {
                // 注入实现或平台库意外 panic 时仍发出终态，避免 UI 永久禁用。
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    provider.select(&request)
                }))
                .unwrap_or_else(|_| Err("系统文件选择器意外退出".into()));
                let _ = sender.send(DialogEvent {
                    purpose: request.purpose,
                    result,
                });
            })
        {
            self.pending.store(false, Ordering::Release);
            return Err(format!("无法启动文件选择任务：{error}"));
        }
        Ok(())
    }
    pub fn try_next_event(&self) -> Option<DialogEvent> {
        match self.events.try_recv() {
            Ok(event) => {
                self.pending.store(false, Ordering::Release);
                Some(event)
            }
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }
}

/// 扩展名只用于设置读取器，不推断采样率、通道顺序或振幅单位。
pub fn eeg_format_index(path: &std::path::Path) -> Result<i32, String> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("edf") => Ok(0),
        Some("csv") => Ok(1),
        Some("mat") => Ok(2),
        _ => Err("请选择 EDF、CSV 或 MAT 格式的 EEG 文件".into()),
    }
}

fn title(purpose: DialogPurpose) -> &'static str {
    match purpose {
        DialogPurpose::ProjectDirectory => "选择 EEG 项目目录",
        DialogPurpose::EegFile => "选择 EEG 文件",
        DialogPurpose::Export(_) => "保存 EEG 分析结果",
    }
}

#[cfg(target_os = "linux")]
fn zenity_command(request: &DialogRequest) -> Command {
    let mut command = Command::new("zenity");
    command.args(["--file-selection", "--title", title(request.purpose)]);
    match request.purpose {
        DialogPurpose::ProjectDirectory => {
            command.arg("--directory");
        }
        DialogPurpose::EegFile => {
            command.arg("--file-filter=EEG 文件 | *.edf *.EDF *.csv *.CSV *.mat *.MAT");
            command.arg("--file-filter=所有文件 | *");
        }
        DialogPurpose::Export(format) => {
            command.arg("--save");
            command.arg(format!(
                "--file-filter={} 文件 | *.{}",
                format.extension().to_uppercase(),
                format.extension()
            ));
        }
    }
    if let Some(directory) = &request.initial_directory {
        // PathBuf::join("") 保留目录末尾分隔符，避免选择器将目录名当成文件名。
        command.arg("--filename").arg(match request.purpose {
            DialogPurpose::Export(format) => {
                directory.join(format!("eeg-report.{}", format.extension()))
            }
            _ => directory.join(""),
        });
    }
    command
}

#[cfg(target_os = "linux")]
fn system_select(request: &DialogRequest) -> Result<Option<PathBuf>, String> {
    let output = match zenity_command(request).output() {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut command = Command::new("kdialog");
            command.arg(match request.purpose {
                DialogPurpose::ProjectDirectory => "--getexistingdirectory",
                DialogPurpose::EegFile => "--getopenfilename",
                DialogPurpose::Export(_) => "--getsavefilename",
            });
            command.arg(
                request
                    .initial_directory
                    .as_deref()
                    .unwrap_or_else(|| std::path::Path::new(".")),
            );
            match request.purpose {
                DialogPurpose::EegFile => {
                    command.arg("EEG 文件 (*.edf *.EDF *.csv *.CSV *.mat *.MAT)");
                }
                DialogPurpose::Export(format) => {
                    command.arg(format!(
                        "{} 文件 (*.{})",
                        format.extension(),
                        format.extension()
                    ));
                }
                _ => {}
            }
            command.args(["--title", title(request.purpose)]);
            command.output().map_err(|e| {
                if e.kind() == io::ErrorKind::NotFound {
                    "未找到系统文件选择器，请安装 Zenity 或 KDialog".into()
                } else {
                    format!("无法打开系统文件选择器：{e}")
                }
            })?
        }
        Err(error) => return Err(format!("无法打开系统文件选择器：{error}")),
    };
    decode_output(output, Some(1), false)
}

#[cfg(target_os = "macos")]
fn system_select(request: &DialogRequest) -> Result<Option<PathBuf>, String> {
    // 固定 AppleScript，路径通过 argv 传入，空格/引号/美元符号不会成为脚本。
    let script = match request.purpose {
        DialogPurpose::ProjectDirectory => {
            "on run argv\nset p to choose folder with prompt (item 1 of argv)\nreturn POSIX path of p\nend run"
        }
        DialogPurpose::EegFile => {
            "on run argv\nset p to choose file with prompt (item 1 of argv) of type {\"edf\", \"csv\", \"mat\"}\nreturn POSIX path of p\nend run"
        }
        DialogPurpose::Export(_) => {
            "on run argv\nset p to choose file name with prompt (item 1 of argv) default name (item 2 of argv)\nreturn POSIX path of p\nend run"
        }
    };
    let default_name = match request.purpose {
        DialogPurpose::Export(format) => format!("eeg-report.{}", format.extension()),
        _ => String::new(),
    };
    let output = Command::new("osascript")
        .args(["-e", script, title(request.purpose), &default_name])
        .output()
        .map_err(|e| format!("无法打开系统文件选择器：{e}"))?;
    decode_output(output, None, true)
}

#[cfg(target_os = "windows")]
fn system_select(request: &DialogRequest) -> Result<Option<PathBuf>, String> {
    use std::os::windows::process::CommandExt;
    let script = r#"
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
Add-Type -AssemblyName System.Windows.Forms
if ($env:EEG_DIALOG_PURPOSE -eq 'project') {
    $dialog = New-Object System.Windows.Forms.FolderBrowserDialog
    $dialog.Description = $env:EEG_DIALOG_TITLE
    if ($env:EEG_DIALOG_INITIAL) { $dialog.SelectedPath = $env:EEG_DIALOG_INITIAL }
    if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) { [Console]::WriteLine($dialog.SelectedPath) }
} else {
    if ($env:EEG_DIALOG_PURPOSE -eq 'export') {
        $dialog = New-Object System.Windows.Forms.SaveFileDialog
        $dialog.Filter = $env:EEG_DIALOG_FILTER
        $dialog.FileName = $env:EEG_DIALOG_NAME
        $dialog.OverwritePrompt = $false
    } else {
        $dialog = New-Object System.Windows.Forms.OpenFileDialog
        $dialog.Filter = 'EEG (*.edf;*.csv;*.mat)|*.edf;*.csv;*.mat'
        $dialog.Multiselect = $false
    }
    $dialog.Title = $env:EEG_DIALOG_TITLE
    if ($env:EEG_DIALOG_INITIAL) { $dialog.InitialDirectory = $env:EEG_DIALOG_INITIAL }
    if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) { [Console]::WriteLine($dialog.FileName) }
}
$dialog.Dispose()
"#;
    let mut command = Command::new("powershell.exe");
    // 隐藏 PowerShell 控制台，原生 Forms 弹窗仍正常显示。
    command.creation_flags(0x08000000);
    command
        .args(["-NoProfile", "-NonInteractive", "-STA", "-Command", script])
        .env("EEG_DIALOG_TITLE", title(request.purpose))
        .env(
            "EEG_DIALOG_PURPOSE",
            match request.purpose {
                DialogPurpose::ProjectDirectory => "project",
                DialogPurpose::EegFile => "eeg",
                DialogPurpose::Export(_) => "export",
            },
        );
    if let DialogPurpose::Export(format) = request.purpose {
        command.env(
            "EEG_DIALOG_FILTER",
            format!(
                "{} (*.{})|*.{}",
                format.extension(),
                format.extension(),
                format.extension()
            ),
        );
        command.env(
            "EEG_DIALOG_NAME",
            format!("eeg-report.{}", format.extension()),
        );
    }
    if let Some(directory) = &request.initial_directory {
        command.env("EEG_DIALOG_INITIAL", directory);
    } else {
        command.env_remove("EEG_DIALOG_INITIAL");
    }
    decode_output(
        command
            .output()
            .map_err(|e| format!("无法打开系统文件选择器：{e}"))?,
        None,
        false,
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn system_select(_: &DialogRequest) -> Result<Option<PathBuf>, String> {
    Err("当前平台暂不支持系统文件弹窗".into())
}

fn decode_output(
    output: Output,
    cancel_code: Option<i32>,
    apple_script: bool,
) -> Result<Option<PathBuf>, String> {
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr);
        if (apple_script && message.contains("(-128)"))
            || (!apple_script && cancel_code.is_some() && output.status.code() == cancel_code)
        {
            // GTK 因无法连接显示服务器失败时也可能返回 1，不能伪装成取消。
            let diagnostic = message.to_ascii_lowercase();
            if !apple_script
                && [
                    "cannot open display",
                    "failed to open display",
                    "unable to init server",
                    "failed to initialize gtk",
                ]
                .iter()
                .any(|marker| diagnostic.contains(marker))
            {
                return Err(format!("系统文件选择器无法打开：{}", message.trim()));
            }
            return Ok(None);
        }
        return Err(format!(
            "系统文件选择器失败（退出码 {:?}）：{}",
            output.status.code(),
            message.trim()
        ));
    }
    let bytes = output.stdout;
    // 只移除协议添加的一个行结束符，不能 trim 路径里的空格/换行。
    #[cfg(target_os = "windows")]
    let bytes = bytes.strip_suffix(b"\r\n").unwrap_or(&bytes);
    #[cfg(not(target_os = "windows"))]
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
    if bytes.is_empty() {
        return Ok(None);
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(Some(std::ffi::OsString::from_vec(bytes.to_vec()).into()))
    }
    #[cfg(not(unix))]
    {
        String::from_utf8(bytes.to_vec())
            .map(|s| Some(PathBuf::from(s)))
            .map_err(|_| "文件选择器返回了无效路径编码".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn format_detection_is_case_insensitive_and_rejects_other_files() {
        for (name, index) in [("EEG.EDF", 0), ("数据.csv", 1), ("test.Mat", 2)] {
            assert_eq!(eeg_format_index(std::path::Path::new(name)).unwrap(), index);
        }
        assert!(eeg_format_index(std::path::Path::new("report.pdf")).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn protocol_preserves_path_spaces_and_distinguishes_cancel_from_failure() {
        use std::os::unix::process::ExitStatusExt;
        let output = |code, stdout: &[u8], stderr: &[u8]| Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.to_vec(),
            stderr: stderr.to_vec(),
        };
        assert_eq!(
            decode_output(output(0, b"/tmp/data file.csv \n", b""), Some(1), false).unwrap(),
            Some(PathBuf::from("/tmp/data file.csv "))
        );
        assert_eq!(
            decode_output(output(1, b"", b""), Some(1), false).unwrap(),
            None
        );
        assert!(
            decode_output(
                output(1, b"", b"Gtk-WARNING: cannot open display"),
                Some(1),
                false
            )
            .is_err()
        );
        assert_eq!(
            decode_output(output(1, b"", b"User canceled. (-128)"), None, true).unwrap(),
            None
        );
        assert!(decode_output(output(2, b"", b"failed"), Some(1), false).is_err());
        // PowerShell 退出码 1 是失败，不能沿用 Zenity 的取消协议。
        assert!(decode_output(output(1, b"", b"script failed"), None, false).is_err());
        assert_eq!(
            decode_output(
                output(1, b"", b"Gtk-Message: optional warning"),
                Some(1),
                false
            )
            .unwrap(),
            None
        );
        use std::os::unix::ffi::OsStrExt;
        let raw = decode_output(output(0, b"/tmp/eeg-\xff.csv\n", b""), Some(1), false)
            .unwrap()
            .unwrap();
        assert_eq!(raw.as_os_str().as_bytes(), b"/tmp/eeg-\xff.csv");
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn chooser_uses_distinct_arguments_and_directory_file_filters() {
        let request = DialogRequest {
            purpose: DialogPurpose::EegFile,
            initial_directory: Some(PathBuf::from("/tmp/a folder;$(echo no)")),
        };
        let command = zenity_command(&request);
        let args: Vec<_> = command.get_args().collect();
        assert!(args.contains(&std::ffi::OsStr::new("/tmp/a folder;$(echo no)/")));
        assert!(args.iter().any(|a| a.to_string_lossy().contains("*.edf")));
        let command = zenity_command(&DialogRequest {
            purpose: DialogPurpose::ProjectDirectory,
            initial_directory: None,
        });
        assert!(command.get_args().any(|a| a == "--directory"));
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn save_dialog_uses_format_filter_and_filename_without_shell_interpolation() {
        for format in [
            eeg_application::ExportFormat::Pdf,
            eeg_application::ExportFormat::Docx,
            eeg_application::ExportFormat::Json,
            eeg_application::ExportFormat::Csv,
        ] {
            let request = DialogRequest {
                purpose: DialogPurpose::Export(format),
                initial_directory: Some(PathBuf::from("/tmp/a folder;$(echo no)")),
            };
            let command = zenity_command(&request);
            let args: Vec<_> = command.get_args().map(|s| s.to_string_lossy()).collect();
            assert!(args.iter().any(|a| a == "--save"));
            assert!(
                args.iter()
                    .any(|a| a.ends_with(&format!("eeg-report.{}", format.extension())))
            );
            assert!(
                args.iter()
                    .any(|a| a.contains(&format!("*.{}", format.extension())))
            );
            assert!(!args.iter().any(|a| a == "--directory"));
        }
    }
}
