//! 证明弹窗等待不占用 UI 线程，并测试取消/错误恢复及重复请求保护。
use depression_desktop::file_dialog::*;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

fn event(controller: &DialogController) -> DialogEvent {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(event) = controller.try_next_event() {
            return event;
        }
        assert!(Instant::now() < deadline, "弹窗没有终态");
        std::thread::sleep(Duration::from_millis(1));
    }
}
struct BlockingPicker {
    entered: mpsc::SyncSender<()>,
    release: Mutex<mpsc::Receiver<()>>,
}
impl NativeFileDialog for BlockingPicker {
    fn select(&self, _: &DialogRequest) -> Result<Option<PathBuf>, String> {
        self.entered.send(()).unwrap();
        self.release
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        Ok(Some(PathBuf::from("/tmp/EEG data.csv")))
    }
}
#[test]
fn selection_wait_is_nonblocking_and_duplicate_requests_are_rejected() {
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let controller = DialogController::new(Arc::new(BlockingPicker {
        entered: entered_tx,
        release: Mutex::new(release_rx),
    }));
    let request = DialogRequest {
        purpose: DialogPurpose::EegFile,
        initial_directory: None,
    };
    controller.request(request.clone()).unwrap();
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    // select 已在后台阻塞，但 request 返回；第二次请求必须被拒绝。
    assert!(controller.request(request).is_err());
    release_tx.send(()).unwrap();
    let result = event(&controller);
    assert_eq!(result.purpose, DialogPurpose::EegFile);
    assert_eq!(
        result.result.unwrap(),
        Some(PathBuf::from("/tmp/EEG data.csv"))
    );
}
struct CancelPicker;
impl NativeFileDialog for CancelPicker {
    fn select(&self, _: &DialogRequest) -> Result<Option<PathBuf>, String> {
        Ok(None)
    }
}
#[test]
fn cancel_is_a_successful_terminal_event_and_allows_another_selection() {
    let controller = DialogController::new(Arc::new(CancelPicker));
    for _ in 0..2 {
        controller
            .request(DialogRequest {
                purpose: DialogPurpose::ProjectDirectory,
                initial_directory: None,
            })
            .unwrap();
        assert_eq!(event(&controller).result.unwrap(), None);
    }
}
struct FailingPicker;
impl NativeFileDialog for FailingPicker {
    fn select(&self, _: &DialogRequest) -> Result<Option<PathBuf>, String> {
        Err("missing chooser".into())
    }
}
#[test]
fn failure_releases_pending_gate_and_preserves_error_for_ui() {
    let controller = DialogController::new(Arc::new(FailingPicker));
    for _ in 0..2 {
        controller
            .request(DialogRequest {
                purpose: DialogPurpose::EegFile,
                initial_directory: None,
            })
            .unwrap();
        assert_eq!(event(&controller).result.unwrap_err(), "missing chooser");
    }
}
