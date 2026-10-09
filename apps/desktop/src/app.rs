//! UI 生命周期和回调绑定。后台 worker 通过事件回传，Slint Timer 非阻塞轮询。
use crate::file_dialog::{
    DialogController, DialogPurpose, DialogRequest, NativeFileDialog, SystemFileDialog,
    eeg_format_index,
};
use crate::{AppWindow, plot_renderer::PlotRenderer, presentation::apply_snapshot_with_renderer};
use eeg_application::*;
use slint::{ComponentHandle, Model};
use std::{cell::RefCell, path::PathBuf, rc::Rc, sync::Arc, time::Duration};

/// 保留 OS 原始路径；界面使用的 lossy 字符串只用于展示，不能作为 IO 真值。
#[derive(Default)]
struct SelectedPaths {
    eeg: Option<PathBuf>,
}

pub struct DesktopApp {
    window: AppWindow,
    // 必须与窗口同寿命；timer 回调持有 controller，销毁时关闭后台命令通道。
    _timer: slint::Timer,
}
impl DesktopApp {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_file_dialog(Arc::new(SystemFileDialog))
    }
    pub fn with_file_dialog(
        provider: Arc<dyn NativeFileDialog>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_service_and_file_dialog(ApplicationService::default(), provider)
    }
    /// 宿主可替换应用服务/报告 Provider；无显示服务器的集成测试也使用真实回调链。
    pub fn with_service_and_file_dialog(
        service: ApplicationService,
        provider: Arc<dyn NativeFileDialog>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let window = AppWindow::new()?;
        let controller = AppController::start(service)?;
        let state = Rc::new(RefCell::new(AppSnapshot::default()));
        let renderer = Rc::new(RefCell::new(PlotRenderer::default()));
        let reporter = renderer.clone();
        window.on_plot_geometry(move |kind, width, height| {
            reporter.borrow_mut().report_geometry(kind, width, height)
        });
        let dialogs = Rc::new(DialogController::new(provider));
        let paths = Rc::new(RefCell::new(SelectedPaths::default()));
        bind(
            &window,
            controller.dispatcher(),
            state.clone(),
            paths.clone(),
            renderer.clone(),
        );
        bind_dialogs(&window, dialogs.clone(), state.clone());
        let dispatcher = controller.dispatcher();
        let weak = window.as_weak();
        let timer = slint::Timer::default();
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(30),
            move || {
                let Some(window) = weak.upgrade() else {
                    return;
                };
                while let Some(event) = dialogs.try_next_event() {
                    window.set_dialog_open(false);
                    match event.result {
                        Ok(Some(path)) => match event.purpose {
                            DialogPurpose::ProjectDirectory => {
                                submit(&window, &dispatcher, Ok(AppCommand::OpenProject(path)));
                            }
                            DialogPurpose::Export(format) => {
                                submit(
                                    &window,
                                    &dispatcher,
                                    Ok(AppCommand::ExportReport(ExportRequest {
                                        destination: path,
                                        format,
                                    })),
                                );
                            }
                            DialogPurpose::EegFile => match eeg_format_index(&path) {
                                Ok(format) => {
                                    window.set_file_path(path.to_string_lossy().as_ref().into());
                                    window.set_file_label(
                                        path.file_name()
                                            .unwrap_or(path.as_os_str())
                                            .to_string_lossy()
                                            .as_ref()
                                            .into(),
                                    );
                                    window.set_format_index(format);
                                    window.set_file_selected(true);
                                    paths.borrow_mut().eeg = Some(path);
                                    window.set_status("文件已选择，确认导入参数后即可导入".into());
                                }
                                Err(error) => {
                                    window.set_status("文件格式不支持，请重新选择".into());
                                    window.set_error_message(error.into());
                                }
                            },
                        },
                        Ok(None) => window.set_status("已取消选择".into()),
                        Err(error) => {
                            window.set_status("文件选择失败，可重试".into());
                            window.set_error_message(error.into());
                        }
                    }
                }
                while let Some(event) = controller.try_next_event() {
                    match event {
                        AppEvent::Progress(stage) => window.set_status(stage.into()),
                        AppEvent::Completed(snapshot) => {
                            // 换项目清除上个项目待导入的文件，避免跨项目误导入。
                            if state.borrow().project_path != snapshot.project_path {
                                paths.borrow_mut().eeg = None;
                                window.set_file_selected(false);
                                window.set_file_path("".into());
                                window.set_file_label("尚未选择文件".into());
                            }
                            // 先清空旧图像，再发布新快照；失败不留下上一录制的图形。
                            if let Err(error) = apply_snapshot_with_renderer(
                                &window,
                                &snapshot,
                                &mut renderer.borrow_mut(),
                            ) {
                                window.set_error_message(error.into());
                            }
                            let export_changed = snapshot.last_export != state.borrow().last_export;
                            if export_changed && let Some(receipt) = &snapshot.last_export {
                                window.set_status(
                                    format!(
                                        "已导出：{}（{} 字节）",
                                        receipt.destination.display(),
                                        receipt.bytes_written
                                    )
                                    .into(),
                                );
                            } else {
                                window.set_status("操作完成".into());
                            }
                            *state.borrow_mut() = *snapshot;
                            window.set_busy(false);
                        }
                        AppEvent::Failed { error, snapshot } => {
                            let mut message = error.to_string();
                            if let Some(snapshot) = snapshot {
                                // 算法失败时这是上次成功结果；持久化后刷新失败时
                                // 则包含已提交的录制，不能继续显示错误的旧会话。
                                if let Err(image_error) = apply_snapshot_with_renderer(
                                    &window,
                                    &snapshot,
                                    &mut renderer.borrow_mut(),
                                ) {
                                    message.push_str(&format!("\n{image_error}"));
                                }
                                *state.borrow_mut() = *snapshot;
                            }
                            window.set_busy(false);
                            window.set_error_message(message.into());
                            window.set_status("操作失败，可修改参数后重试".into());
                            // worker 意外退出时没有快照，恢复最后已知选择。
                            restore_indices(&window, &state.borrow());
                        }
                    }
                }
                // 合并布局回报；也检测跨显示器/系统缩放变化。
                renderer.borrow_mut().refresh(&window, &state.borrow());
            },
        );
        Ok(Self {
            window,
            _timer: timer,
        })
    }
    pub fn run(&self) -> Result<(), slint::PlatformError> {
        self.window.run()
    }
    /// 供宿主集成和无显示服务器的 UI smoke 测试访问生成组件。
    pub fn window(&self) -> &AppWindow {
        &self.window
    }
}

fn restore_indices(window: &AppWindow, state: &AppSnapshot) {
    window.set_subject_index(
        state
            .subjects
            .iter()
            .position(|s| Some(s.id) == state.selected_subject)
            .map_or(-1, |i| i as i32),
    );
    window.set_recording_index(
        state
            .recordings
            .iter()
            .position(|r| state.raw.as_ref().is_some_and(|raw| raw.id == r.id))
            .map_or(-1, |i| i as i32),
    );
}
fn submit(window: &AppWindow, dispatcher: &CommandDispatcher, command: AppResult<AppCommand>) {
    match command.and_then(|command| dispatcher.submit(command)) {
        Ok(()) => {
            window.set_busy(true);
            window.set_error_message("".into());
            window.set_status("后台任务已提交…".into());
        }
        Err(error) => window.set_error_message(error.to_string().into()),
    }
}
fn bind(
    window: &AppWindow,
    dispatcher: CommandDispatcher,
    state: Rc<RefCell<AppSnapshot>>,
    paths: Rc<RefCell<SelectedPaths>>,
    renderer: Rc<RefCell<PlotRenderer>>,
) {
    // 复选框变更立即提交视图命令，不执行分析。busy 时拒绝二次修改，
    // 失败由快照恢复已接受的选择；程序设置 checked 不触发 toggled。
    let weak = window.as_weak();
    let sender = dispatcher.clone();
    let snapshot = state.clone();
    let painter = renderer.clone();
    window.on_toggle_channel(move |index, selected| {
        if let Some(window) = weak.upgrade() {
            if window.get_busy() || window.get_dialog_open() {
                return;
            }
            let model = window.get_channel_options();
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            let Some(mut row) = model.row_data(index) else {
                return;
            };
            row.selected = selected;
            model.set_row_data(index, row);
            submit(
                &window,
                &sender,
                command_from_form(&window, &snapshot.borrow(), 7),
            );
            if !window.get_busy() {
                // 表单校验失败没有后台事件，仍须恢复复选框，而不是显示未提交的状态。
                let _ = apply_snapshot_with_renderer(
                    &window,
                    &snapshot.borrow(),
                    &mut painter.borrow_mut(),
                );
            }
        }
    });
    let weak = window.as_weak();
    let sender = dispatcher.clone();
    let snapshot = state.clone();
    let painter = renderer.clone();
    window.on_select_all_channels(move |selected| {
        if let Some(window) = weak.upgrade() {
            if window.get_busy() || window.get_dialog_open() {
                return;
            }
            let model = window.get_channel_options();
            for index in 0..model.row_count() {
                if let Some(mut row) = model.row_data(index) {
                    row.selected = selected;
                    model.set_row_data(index, row);
                }
            }
            submit(
                &window,
                &sender,
                command_from_form(&window, &snapshot.borrow(), 7),
            );
            if !window.get_busy() {
                let _ = apply_snapshot_with_renderer(
                    &window,
                    &snapshot.borrow(),
                    &mut painter.borrow_mut(),
                );
            }
        }
    });
    let weak = window.as_weak();
    let sender = dispatcher.clone();
    let snapshot = state.clone();
    window.on_action(move |action| {
        if let Some(window) = weak.upgrade() {
            if window.get_dialog_open() || window.get_busy() {
                return;
            }
            let mut command = command_from_form(&window, &snapshot.borrow(), action);
            if action == 4 {
                command = command.and_then(|mut command| {
                    let chosen = paths
                        .borrow()
                        .eeg
                        .clone()
                        .ok_or(AppError::InvalidState("请先选择 EEG 文件"))?;
                    if let AppCommand::ImportEeg { path, .. } = &mut command {
                        *path = chosen;
                    }
                    Ok(command)
                });
            }
            submit(&window, &sender, command);
        }
    });
    let weak = window.as_weak();
    let sender = dispatcher.clone();
    let snapshot = state.clone();
    window.on_select_subject(move |index| {
        if let Some(window) = weak.upgrade() {
            if window.get_dialog_open() || window.get_busy() {
                return;
            }
            let command = usize::try_from(index)
                .ok()
                .and_then(|i| snapshot.borrow().subjects.get(i).map(|s| s.id))
                .map(AppCommand::SelectSubject)
                .ok_or(AppError::InvalidInput("请选择有效受试者".into()));
            submit(&window, &sender, command);
        }
    });
    let weak = window.as_weak();
    window.on_select_recording(move |index| {
        if let Some(window) = weak.upgrade() {
            if window.get_dialog_open() || window.get_busy() {
                return;
            }
            let command = usize::try_from(index)
                .ok()
                .and_then(|i| state.borrow().recordings.get(i).map(|r| r.id))
                .map(AppCommand::LoadRecording)
                .ok_or(AppError::InvalidInput("请选择有效录制".into()));
            submit(&window, &dispatcher, command);
        }
    });
}

fn bind_dialogs(
    window: &AppWindow,
    dialogs: Rc<DialogController>,
    state: Rc<RefCell<AppSnapshot>>,
) {
    let weak = window.as_weak();
    let picker = dialogs.clone();
    let snapshot = state.clone();
    window.on_choose_project(move || {
        if let Some(window) = weak.upgrade() {
            request_dialog(
                &window,
                &picker,
                DialogRequest {
                    purpose: DialogPurpose::ProjectDirectory,
                    initial_directory: snapshot.borrow().project_path.clone(),
                },
            );
        }
    });
    let weak = window.as_weak();
    let export_picker = dialogs.clone();
    let export_state = state.clone();
    window.on_choose_export(move || {
        if let Some(window) = weak.upgrade() {
            let format = match window.get_export_format_index() {
                0 => ExportFormat::Pdf,
                1 => ExportFormat::Docx,
                2 => ExportFormat::Json,
                3 => ExportFormat::Csv,
                _ => {
                    window.set_error_message("请选择导出格式".into());
                    return;
                }
            };
            let snapshot = export_state.borrow();
            if snapshot.analysis.is_none()
                || (format.needs_narrative() && snapshot.report.is_none())
            {
                window.set_error_message("请先完成分析；PDF / DOCX 还需生成报告".into());
                return;
            }
            request_dialog(
                &window,
                &export_picker,
                DialogRequest {
                    purpose: DialogPurpose::Export(format),
                    initial_directory: snapshot.project_path.clone(),
                },
            );
        }
    });
    let weak = window.as_weak();
    window.on_choose_eeg(move || {
        if let Some(window) = weak.upgrade() {
            if !window.get_subject_selected() {
                window.set_error_message("请先选择受试者".into());
                return;
            }
            request_dialog(
                &window,
                &dialogs,
                DialogRequest {
                    purpose: DialogPurpose::EegFile,
                    initial_directory: state.borrow().project_path.clone(),
                },
            );
        }
    });
}
fn request_dialog(window: &AppWindow, dialogs: &DialogController, request: DialogRequest) {
    if window.get_busy() || window.get_dialog_open() {
        return;
    }
    match dialogs.request(request) {
        Ok(()) => {
            window.set_dialog_open(true);
            window.set_error_message("".into());
            window.set_status("请在系统弹窗中完成选择…".into());
        }
        Err(error) => window.set_error_message(error.into()),
    }
}

fn number(name: &str, text: slint::SharedString) -> AppResult<f64> {
    text.trim()
        .parse()
        .map_err(|_| AppError::InvalidInput(format!("{name}必须是数字")))
}
fn nonempty(value: slint::SharedString) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value.trim().into())
    }
}

/// 纯表单→命令映射；没有文件 IO、数据库查询或算法调用，可直接做 UI smoke 测试。
/// action 编号与 app.slint 的按钮对应；导航页切换由 Slint 自身完成。
pub fn command_from_form(
    window: &AppWindow,
    state: &AppSnapshot,
    action: i32,
) -> AppResult<AppCommand> {
    let subject = || {
        state
            .selected_subject
            .ok_or(AppError::InvalidState("请先选择受试者"))
    };
    let raw_id = || {
        state
            .raw
            .as_ref()
            .map(|r| r.id)
            .ok_or(AppError::InvalidState("请先选择录制"))
    };
    let draft = || {
        SubjectForm {
            age: window.get_subject_age().to_string(),
            sex_index: window.get_sex_index(),
            notes: window.get_subject_notes().to_string(),
        }
        .parse()
    };
    match action {
        1 => Ok(AppCommand::CreateSubject(draft()?)),
        2 => Ok(AppCommand::UpdateSubject {
            id: subject()?,
            draft: draft()?,
        }),
        3 | 9 => {
            if !window.get_confirm_delete() {
                return Err(AppError::InvalidState("请勾选确认删除"));
            }
            if action == 3 {
                Ok(AppCommand::DeleteSubject(subject()?))
            } else {
                Ok(AppCommand::DeleteRecording(raw_id()?))
            }
        }
        4 => {
            let text = window.get_file_path();
            if text.trim().is_empty() {
                return Err(AppError::InvalidInput("文件路径不能为空".into()));
            }
            let format = ImportForm {
                format_index: window.get_format_index(),
                sampling_rate: window.get_sampling_rate().to_string(),
                unit: window.get_source_unit().to_string(),
                csv_headers: window.get_csv_headers(),
                labels: window.get_channel_labels().to_string(),
                mat_variable: window.get_mat_variable().to_string(),
                mat_rate_variable: window.get_mat_rate_variable().to_string(),
                mat_samples_by_channels: window.get_mat_transposed(),
            }
            .parse()?;
            let mut metadata = RecordingMetadata {
                device: nonempty(window.get_device()),
                electrode_system: nonempty(window.get_electrode_system()),
                notes: nonempty(window.get_recording_notes()),
                ..Default::default()
            };
            if let Some(date) = nonempty(window.get_recorded_at()) {
                metadata.recorded_at =
                    Some(date.parse().map_err(|_| {
                        AppError::InvalidInput("采集时间须为带时区的 RFC3339".into())
                    })?);
            }
            if let Some(condition) = nonempty(window.get_recording_condition()) {
                metadata
                    .extra
                    .insert("recording_condition".into(), condition);
            }
            Ok(AppCommand::ImportEeg {
                path: PathBuf::from(text.trim()),
                format,
                metadata,
            })
        }
        5 => Ok(AppCommand::AssessQuality(quality_config(window)?)),
        6 => {
            let mut request = ProcessingForm {
                dc: window.get_remove_dc(),
                bandpass: window.get_bandpass(),
                low_hz: window.get_low_hz().to_string(),
                high_hz: window.get_high_hz().to_string(),
                notch: window.get_notch(),
                notch_hz: window.get_notch_hz().to_string(),
                resample: window.get_resample(),
                target_hz: window.get_target_hz().to_string(),
                exclude_bad_channels: window.get_exclude_bad(),
            }
            .parse()?;
            request.quality = quality_config(window)?;
            Ok(AppCommand::Analyze(request))
        }
        7 | 10..=13 => {
            let mut view = ViewRequest {
                // 精确读取复选框模型。Some(empty) 是隐藏全部，None 才是默认全部。
                channels: Some(
                    window
                        .get_channel_options()
                        .iter()
                        .filter(|c| c.selected)
                        .map(|c| c.label.to_string())
                        .collect(),
                ),
                start_seconds: number("起点", window.get_start_seconds())?,
                window_seconds: number("窗长", window.get_window_seconds())?,
                amplitude_uv: number("幅度", window.get_amplitude_uv())?,
                processed: window.get_show_processed(),
                band: match window.get_band_index() {
                    0 => FrequencyBand::Delta,
                    1 => FrequencyBand::Theta,
                    2 => FrequencyBand::Alpha,
                    3 => FrequencyBand::Beta,
                    _ => return Err(AppError::InvalidInput("请选择有效频段".into())),
                },
                relative_power: window.get_relative_power(),
            };
            view.validate()?;
            // 导航边界必须使用正在显示的采样网格。重采样后若沿用原始
            // 采样率，最后一窗可能越过处理后最后一个样本，产生假空窗口。
            let recording = if view.processed {
                state
                    .analysis
                    .as_ref()
                    .ok_or(AppError::InvalidState("请先完成分析"))?
                    .processed
                    .as_ref()
            } else {
                state
                    .raw
                    .as_deref()
                    .ok_or(AppError::InvalidState("请先选择录制"))?
            };
            let last =
                recording.sample_count().saturating_sub(1) as f64 / recording.sampling_rate_hz;
            match action {
                10 => view.start_seconds = (view.start_seconds - view.window_seconds).max(0.0),
                11 => view.start_seconds = (view.start_seconds + view.window_seconds).min(last),
                12 => {
                    view.window_seconds =
                        (view.window_seconds / 2.0).max(1.0 / recording.sampling_rate_hz)
                }
                13 => {
                    view.window_seconds =
                        (view.window_seconds * 2.0).min(recording.duration_seconds)
                }
                _ => {}
            }
            view.validate()?;
            Ok(AppCommand::SetView(view))
        }
        8 => Ok(AppCommand::SaveProcessed),
        14 => {
            if state.analysis.is_none() {
                return Err(AppError::InvalidState("请先完成 EEG 分析"));
            }
            Ok(AppCommand::GenerateReport)
        }
        _ => Err(AppError::InvalidInput("未知操作".into())),
    }
}

// 采集工频和预处理陷波分别配置，关掉陷波不会隐式关闭质量评价。
fn quality_config(window: &AppWindow) -> AppResult<QualityConfig> {
    let mut config = QualityConfig::default();
    config.spectral.line_frequency_hz = if window.get_quality_line_enabled() {
        let value = number("质量评价工频", window.get_quality_line_hz())?;
        if !value.is_finite() || value <= 0.0 {
            return Err(AppError::InvalidInput("质量评价工频须为有限正数".into()));
        }
        Some(value)
    } else {
        None
    };
    Ok(config)
}
