//! 真实 Slint 编译组件 + 软件窗口 + 后台 worker 的完整回调工作流。
//! 无需 X11/Wayland、额外测试 crate、真实 EEG 或网络连接。
use depression_desktop::{
    AppWindow, DesktopApp, command_from_form,
    file_dialog::{DialogPurpose, DialogRequest, NativeFileDialog},
};
use eeg_application::*;
use slint::{
    ComponentHandle, Model, Rgb8Pixel,
    platform::{
        Platform, WindowAdapter,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    },
};
use std::{
    collections::VecDeque,
    f64::consts::TAU,
    fs,
    io::Write,
    path::Path,
    rc::Rc,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

type Choice = (DialogPurpose, Result<Option<std::path::PathBuf>, String>);
struct ScriptedChooser(Mutex<VecDeque<Choice>>);
impl NativeFileDialog for ScriptedChooser {
    fn select(&self, request: &DialogRequest) -> Result<Option<std::path::PathBuf>, String> {
        let (purpose, result) = self
            .0
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected dialog request");
        assert_eq!(purpose, request.purpose);
        result
    }
}

struct Headless {
    window: Rc<MinimalSoftwareWindow>,
}
impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
}
fn settle(window: &AppWindow) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while window.get_busy() || window.get_dialog_open() {
        slint::platform::update_timers_and_animations();
        assert!(Instant::now() < deadline, "UI 任务超时");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn wait(window: &AppWindow) {
    settle(window);
    assert!(
        window.get_error_message().is_empty(),
        "{}",
        window.get_error_message()
    );
}
fn action(window: &AppWindow, value: i32) {
    window.invoke_action(value);
    wait(window);
}
fn render(window: &Rc<MinimalSoftwareWindow>, path: Option<&Path>) -> Vec<Rgb8Pixel> {
    window.request_redraw();
    let mut buffer = vec![Rgb8Pixel::default(); 1280 * 900];
    assert!(window.draw_if_needed(|renderer| {
        renderer.render(&mut buffer, 1280);
    }));
    assert!(
        buffer.iter().any(|p| p.r != p.g),
        "窗口应包含有颜色的 UI 内容"
    );
    let header = buffer[1280 * 96..1280 * 158].to_vec();
    if let Some(path) = path {
        let mut file = fs::File::create(path).unwrap();
        write!(file, "P6\n1280 900\n255\n").unwrap();
        for pixel in buffer {
            file.write_all(&[pixel.r, pixel.g, pixel.b]).unwrap();
        }
    }
    header
}

fn render_full(window: &Rc<MinimalSoftwareWindow>) -> Vec<Rgb8Pixel> {
    window.request_redraw();
    let mut pixels = vec![Rgb8Pixel::default(); 1280 * 900];
    assert!(window.draw_if_needed(|renderer| {
        renderer.render(&mut pixels, 1280);
    }));
    pixels
}

// changed 回调在布局求值后交付，timer 随后合并几何变化并替换像素图。
// 使用实际物理尺寸渲染，支持 1×/2× DPI 与窗口放大，避免固定测试画布掩盖问题。
fn flush_raster(window: &AppWindow, software: &Rc<MinimalSoftwareWindow>) {
    for _ in 0..5 {
        slint::platform::update_timers_and_animations();
        let size = window.window().size();
        let mut pixels = vec![Rgb8Pixel::default(); size.width as usize * size.height as usize];
        software.request_redraw();
        software.draw_if_needed(|renderer| {
            renderer.render(&mut pixels, size.width as usize);
        });
        std::thread::sleep(Duration::from_millis(35));
    }
}

#[test]
fn slint_callbacks_complete_eeg_workflow_and_render_all_pages_without_display_server() {
    let software = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Headless {
        window: software.clone(),
    }))
    .unwrap();
    let project = std::env::temp_dir().join(format!("p8-ui-{}", RecordingId::new()));
    fs::create_dir_all(&project).unwrap();
    let input = project.join("synthetic.csv");
    let labels = [
        "Fp1", "Fp2", "Cz", "O1", "O2", "F3", "F4", "F7", "F8", "P3", "P4", "Pz",
    ];
    let mut csv = format!("{}\n", labels.join(","));
    for i in 0..2048 {
        for j in 0..labels.len() {
            if j > 0 {
                csv.push(',');
            }
            csv.push_str(&format!(
                "{:.6}",
                (20.0 + j as f64) * (TAU * 10.0 * i as f64 / 256.0).sin()
            ));
        }
        csv.push('\n');
    }
    fs::write(&input, csv).unwrap();
    let chooser = Arc::new(ScriptedChooser(Mutex::new(VecDeque::from([
        (DialogPurpose::ProjectDirectory, Ok(Some(project.clone()))),
        (DialogPurpose::ProjectDirectory, Ok(None)),
        (DialogPurpose::EegFile, Ok(Some(input.clone()))),
        (DialogPurpose::EegFile, Ok(None)),
        (
            DialogPurpose::EegFile,
            Ok(Some(project.join("invalid.pdf"))),
        ),
        (DialogPurpose::EegFile, Ok(Some(input.clone()))),
        (
            DialogPurpose::EegFile,
            Err("injected native failure".into()),
        ),
        (DialogPurpose::EegFile, Ok(Some(input.clone()))),
    ]))));
    let desktop = DesktopApp::with_file_dialog(chooser.clone()).unwrap();
    let window = desktop.window();
    window
        .window()
        .set_size(slint::PhysicalSize::new(1280, 900));
    window.show().unwrap();
    window.invoke_choose_project();
    assert!(window.get_dialog_open());
    window.invoke_choose_project(); // 重复点击不会打开第二个弹窗
    wait(window);
    assert!(window.get_project_open());
    let saved_project = window.get_project_path();
    window.invoke_choose_project();
    wait(window);
    assert_eq!(window.get_project_path(), saved_project); // 取消保留项目
    window.set_subject_age("32".into());
    window.set_subject_notes("Synthetic test".into());
    action(window, 1);
    assert!(window.get_subject_selected());
    window.set_sampling_rate("256".into());
    window.invoke_choose_eeg();
    wait(window);
    assert_eq!(window.get_format_index(), 1);
    assert!(window.get_file_selected());
    let selected_path = window.get_file_path();
    window.invoke_choose_eeg();
    wait(window);
    assert_eq!(window.get_file_path(), selected_path);
    window.invoke_choose_eeg();
    settle(window);
    assert!(!window.get_error_message().is_empty());
    assert!(window.get_status().contains("文件格式不支持"));
    assert_eq!(window.get_file_path(), selected_path); // 无效扩展名保留上次选择
    window.invoke_choose_eeg();
    wait(window);
    window.invoke_choose_eeg();
    settle(window);
    assert!(
        window
            .get_error_message()
            .contains("injected native failure")
    );
    assert!(!window.get_dialog_open());
    assert_eq!(window.get_file_path(), selected_path);
    window.invoke_choose_eeg();
    wait(window);
    window.set_recorded_at("2026-10-07T17:00:00+08:00".into());
    action(window, 4);
    assert!(window.get_recording_loaded());
    assert_eq!(window.get_waveform_rows().row_count(), labels.len());
    assert!(window.get_channel_options().iter().all(|c| c.selected));
    assert!(
        window
            .get_waveform_rows()
            .iter()
            .all(|r| r.pixels.size().width > 0)
    );
    assert_eq!(window.get_selected_channel_count(), labels.len() as i32);
    window.invoke_select_subject(window.get_subject_index());
    wait(window);
    assert!(!window.get_recording_loaded());
    window.invoke_select_recording(0);
    wait(window);
    assert!(window.get_recording_loaded());
    action(window, 5);
    assert!(window.get_quality_summary().contains("原始信号"));
    window.set_resample(true);
    window.set_target_hz("128".into());
    action(window, 6);
    assert!(window.get_analysis_ready());
    assert!(window.get_has_psd());
    assert!(window.get_has_topomap());
    assert!(window.get_band_summary().contains("Alpha"));
    assert!(window.get_result_summary().contains("特征溯源"));
    action(window, 12);
    assert_eq!(window.get_window_seconds(), "2.5");
    action(window, 11);
    assert_eq!(window.get_start_seconds(), "2.5");
    window.set_start_seconds("7.5".into());
    action(window, 7);
    action(window, 11);
    assert_eq!(
        window.get_start_seconds().parse::<f64>().unwrap(),
        1023.0 / 128.0
    );
    action(window, 11); // 在最后一个处理后采样点重复下一窗仍可显示
    window.set_start_seconds("2.5".into());
    action(window, 7);
    window.set_amplitude_uv("25".into());
    window.invoke_select_all_channels(false);
    wait(window);
    assert_eq!(window.get_waveform_rows().row_count(), 0);
    assert_eq!(window.get_psd_rows().row_count(), 0);
    assert!(window.get_channel_options().iter().all(|c| !c.selected));
    assert!(window.get_analysis_ready()); // 隐藏曲线不会删除特征或关闭分析结果。
    window.invoke_toggle_channel(2, true);
    wait(window);
    assert_eq!(window.get_waveform_rows().row_count(), 1);
    assert_eq!(window.get_waveform_rows().row_data(0).unwrap().label, "Cz");
    assert_eq!(window.get_psd_rows().row_count(), 1);
    assert_eq!(window.get_selected_channel_count(), 1);
    action(window, 8);
    assert!(window.get_processed_saved());
    // 通过实际回调触发错误，然后恢复：终态不能把 UI 永久卡在 busy。
    window.set_low_hz("NaN".into());
    window.invoke_action(6);
    assert!(!window.get_busy());
    assert!(!window.get_error_message().is_empty());
    window.set_low_hz("0.5".into());
    action(window, 6);
    window.invoke_select_all_channels(true);
    wait(window);
    assert_eq!(window.get_waveform_rows().row_count(), labels.len());
    assert_eq!(window.get_psd_rows().row_count(), labels.len());
    window.set_page(1);
    window.set_plot_index(0);
    slint::platform::update_timers_and_animations();
    flush_raster(window, &software);
    let normal_wave_size = window
        .get_waveform_rows()
        .row_data(0)
        .unwrap()
        .pixels
        .size();
    assert!(
        normal_wave_size.width > 1000 && normal_wave_size.width < 1920,
        "actual view width: {normal_wave_size:?}"
    );
    assert!(
        normal_wave_size.height < 80,
        "actual row height: {normal_wave_size:?}"
    );
    assert_eq!(window.get_waveform_scroll_y(), 0.0);
    // 向图表真实派发滚轮事件，既检查位置变化也检查图像内容变化。
    let before_scroll = render_full(&software);
    // 固定 1280×900 的真实软件渲染中，图表区应出现 8 条通道行分割线。
    // 同时检查左右端的颜色，避免把文字/曲线像素计成行边界。
    let separators = (515..810)
        .filter(|y| {
            [380usize, 1200].iter().all(|x| {
                let p = before_scroll[y * 1280 + x];
                (p.r, p.g, p.b) == (227, 231, 237)
            })
        })
        .count();
    assert_eq!(separators, 8, "初始可视区应容纳 8 个通道行");
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
            position: slint::LogicalPosition::new(900.0, 600.0),
            delta_x: 0.0,
            delta_y: -180.0,
        });
    slint::platform::update_timers_and_animations();
    let after_scroll = render_full(&software);
    assert!(window.get_waveform_scroll_y() < 0.0, "滚轮应移动通道列表");
    assert_ne!(before_scroll, after_scroll);
    assert_eq!(window.get_waveform_rows().row_count(), labels.len());
    let waveform_offset = window.get_waveform_scroll_y();
    let wave_pixels = window.get_waveform_rows().row_data(0).unwrap().pixels;
    flush_raster(window, &software);
    assert_eq!(
        wave_pixels,
        window.get_waveform_rows().row_data(0).unwrap().pixels,
        "滚动应复用缓存图像"
    );
    window.set_plot_index(1);
    flush_raster(window, &software);
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
            position: slint::LogicalPosition::new(900.0, 600.0),
            delta_x: 0.0,
            delta_y: -180.0,
        });
    slint::platform::update_timers_and_animations();
    render(&software, None);
    assert!(window.get_psd_scroll_y() < 0.0);
    assert_eq!(window.get_waveform_scroll_y(), waveform_offset); // 两个视区独立滚动。
    window.set_plot_index(0);
    action(window, 7); // 同一通道集合重新绘制，保留滚动位置。
    assert!(window.get_waveform_scroll_y() < 0.0);
    window.invoke_toggle_channel(0, false);
    wait(window);
    assert_eq!(window.get_waveform_rows().row_count(), labels.len() - 1);
    assert_eq!(window.get_waveform_scroll_y(), 0.0); // 集合变化恢复到首行。
    window.invoke_select_all_channels(true);
    wait(window);
    // 窗口放大自动提高像素宽度，不改变窗长/量程/结构化分析结果。
    window.set_plot_index(0);
    let summary = window.get_result_summary();
    let amplitude = window.get_amplitude_uv();
    let period = window.get_window_seconds();
    window
        .window()
        .set_size(slint::PhysicalSize::new(1600, 1000));
    flush_raster(window, &software);
    let wide_wave_size = window
        .get_waveform_rows()
        .row_data(0)
        .unwrap()
        .pixels
        .size();
    assert!(wide_wave_size.width > normal_wave_size.width);
    window
        .window()
        .set_size(slint::PhysicalSize::new(1280, 900));
    flush_raster(window, &software);
    window.set_plot_index(1);
    flush_raster(window, &software);
    let normal_psd_size = window.get_psd_rows().row_data(0).unwrap().pixels.size();
    window.set_plot_index(2);
    flush_raster(window, &software);
    let normal_map_size = window.get_topomap_image().size();
    assert!(window.get_topomap_labels().row_count() > 0);
    assert_ne!(
        window.get_topomap_upper_label(),
        window.get_topomap_lower_label()
    );
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: 2.0 });
    window
        .window()
        .set_size(slint::PhysicalSize::new(2560, 1800));
    flush_raster(window, &software);
    let hires_map_size = window.get_topomap_image().size();
    assert!(hires_map_size.width >= normal_map_size.width * 2 - 2);
    window.set_plot_index(0);
    flush_raster(window, &software);
    let hires_wave_size = window
        .get_waveform_rows()
        .row_data(0)
        .unwrap()
        .pixels
        .size();
    assert!(hires_wave_size.width >= normal_wave_size.width * 2 - 2);
    assert!(hires_wave_size.height >= normal_wave_size.height * 2 - 2);
    window.set_plot_index(1);
    flush_raster(window, &software);
    let hires_psd_size = window.get_psd_rows().row_data(0).unwrap().pixels.size();
    assert!(hires_psd_size.width >= normal_psd_size.width * 2 - 2);
    assert_eq!(window.get_result_summary(), summary);
    assert_eq!(window.get_amplitude_uv(), amplitude);
    assert_eq!(window.get_window_seconds(), period);
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
    window
        .window()
        .set_size(slint::PhysicalSize::new(1280, 900));
    window.set_plot_index(0);
    flush_raster(window, &software);
    let output = std::env::var_os("P8_UI_PREVIEW_DIR").map(std::path::PathBuf::from);
    if let Some(path) = &output {
        fs::create_dir_all(path).unwrap();
    }
    let mut previous_header = None;
    for page in 0..4 {
        window.set_page(page);
        slint::platform::update_timers_and_animations();
        let header = render(
            &software,
            output
                .as_ref()
                .map(|p| p.join(format!("page-{page}.ppm")))
                .as_deref(),
        );
        if let Some(previous) = &previous_header {
            assert_eq!(previous, &header, "切换页面不能遮挡顶部导航与状态栏");
        }
        previous_header = Some(header);
    }
    window.set_page(1);
    for plot in 0..5 {
        window.set_plot_index(plot);
        flush_raster(window, &software);
        render(
            &software,
            output
                .as_ref()
                .map(|p| p.join(format!("plot-{plot}.ppm")))
                .as_deref(),
        );
    }
    window.set_quality_line_hz("60".into());
    let AppCommand::AssessQuality(config) =
        command_from_form(window, &AppSnapshot::default(), 5).unwrap()
    else {
        panic!()
    };
    assert_eq!(config.spectral.line_frequency_hz, Some(60.0));
    window.set_quality_line_enabled(false);
    window.set_quality_line_hz("ignored".into());
    let AppCommand::AssessQuality(config) =
        command_from_form(window, &AppSnapshot::default(), 5).unwrap()
    else {
        panic!()
    };
    assert_eq!(config.spectral.line_frequency_hz, None);
    // 删除需要 UI 中的显式勾选；空路径/非法时间也不能成为命令。
    assert!(command_from_form(window, &AppSnapshot::default(), 9).is_err());
    window.set_recorded_at("yesterday".into());
    assert!(command_from_form(window, &AppSnapshot::default(), 4).is_err());
    // 空快照必须清掉旧结果和图像。
    depression_desktop::apply_snapshot(window, &AppSnapshot::default()).unwrap();
    assert!(!window.get_analysis_ready());
    assert!(!window.get_has_psd());
    assert_eq!(window.get_waveform_rows().row_count(), 0);
    assert_eq!(window.get_psd_rows().row_count(), 0);
    assert_eq!(window.get_channel_options().row_count(), 0);
    assert!(chooser.0.lock().unwrap().is_empty());
    window.hide().unwrap();
    drop(desktop);
    // worker 被通道关闭唤醒；清理测试产生的合成数据，允许已关闭句柄的异步退出。
    let _ = fs::remove_dir_all(project);
}
