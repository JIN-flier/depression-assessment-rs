mod support;
use eeg_application::*;
use std::{
    path::Path,
    sync::mpsc,
    time::{Duration, Instant},
};
use support::*;

fn terminal(controller: &AppController) -> AppEvent {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(event) = controller.try_next_event()
            && !matches!(event, AppEvent::Progress(_))
        {
            return event;
        }
        assert!(Instant::now() < deadline, "后台任务没有终态事件");
        std::thread::sleep(Duration::from_millis(1));
    }
}

struct BlockingFactory {
    entered: mpsc::SyncSender<()>,
    release: mpsc::Receiver<()>,
}
impl ProjectFactory for BlockingFactory {
    fn open(&self, path: &Path) -> AppResult<Box<dyn ProjectRepository>> {
        self.entered.send(()).unwrap();
        self.release.recv_timeout(Duration::from_secs(5)).unwrap();
        RedbProjectFactory.open(path)
    }
}

#[test]
fn commands_are_nonblocking_busy_is_guarded_until_terminal_event_is_consumed() {
    let project = TestProject::new();
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let service = ApplicationService::new(
        Box::new(BlockingFactory {
            entered: entered_tx,
            release: release_rx,
        }),
        Box::new(CoreEngine),
    );
    let controller = AppController::start(service).unwrap();
    let sender = controller.dispatcher();
    // worker 在 gate 中等待；submit 已返回，证明调用者没有等待文件任务完成。
    sender
        .submit(AppCommand::OpenProject(project.0.clone()))
        .unwrap();
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(sender.is_busy());
    assert!(matches!(
        sender.submit(AppCommand::SaveProcessed),
        Err(AppError::Busy)
    ));
    release_tx.send(()).unwrap();
    // 即使后台已经运行完，busy 也由接收终态这一动作解除，保证快照发布顺序。
    assert!(sender.is_busy());
    assert!(matches!(terminal(&controller), AppEvent::Completed(_)));
    assert!(!sender.is_busy());
    sender.submit(AppCommand::SaveProcessed).unwrap();
    assert!(matches!(
        terminal(&controller),
        AppEvent::Failed {
            error: AppError::InvalidState(_),
            ..
        }
    ));
    assert!(!sender.is_busy());
    sender
        .submit(AppCommand::CreateSubject(SubjectDraft {
            age: None,
            sex: Sex::Unknown,
            notes: String::new(),
        }))
        .unwrap();
    assert!(matches!(terminal(&controller), AppEvent::Completed(_)));
}

#[test]
fn dispatcher_reports_shutdown_and_failures_allow_retry() {
    let controller = AppController::start(ApplicationService::default()).unwrap();
    let sender = controller.dispatcher();
    sender.submit(AppCommand::SaveProcessed).unwrap();
    assert!(matches!(terminal(&controller), AppEvent::Failed { .. }));
    assert!(!sender.is_busy());
    drop(controller);
    assert!(matches!(
        sender.submit(AppCommand::SaveProcessed),
        Err(AppError::WorkerStopped)
    ));
    assert!(!sender.is_busy());
}

// 故障注入只包装列表读取；写入仍交给真实 redb。用于验证“持久化成功、
// 随后刷新失败”时，UI 的错误事件不会谎称磁盘操作已回滚。
struct FailingListRepository {
    store: storage::ProjectStorage,
    fail: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl storage::SubjectRepository for FailingListRepository {
    fn create_subject(&self, value: &domain::Subject) -> storage::StorageResult<()> {
        self.store.create_subject(value)
    }
    fn get_subject(&self, id: SubjectId) -> storage::StorageResult<Option<domain::Subject>> {
        self.store.get_subject(id)
    }
    fn list_subjects(&self) -> storage::StorageResult<Vec<domain::Subject>> {
        self.store.list_subjects()
    }
    fn update_subject(&self, value: &domain::Subject) -> storage::StorageResult<()> {
        self.store.update_subject(value)
    }
    fn delete_subject(&self, id: SubjectId) -> storage::StorageResult<()> {
        self.store.delete_subject(id)
    }
}
impl storage::RecordingRepository for FailingListRepository {
    fn create_recording(&self, value: &domain::EegRecording) -> storage::StorageResult<()> {
        self.store.create_recording(value)
    }
    fn get_recording(
        &self,
        id: RecordingId,
    ) -> storage::StorageResult<Option<domain::EegRecording>> {
        self.store.get_recording(id)
    }
    fn get_recording_metadata(
        &self,
        id: RecordingId,
    ) -> storage::StorageResult<Option<storage::StoredRecordingMetadata>> {
        self.store.get_recording_metadata(id)
    }
    fn list_recording_metadata(
        &self,
        id: SubjectId,
    ) -> storage::StorageResult<Vec<storage::StoredRecordingMetadata>> {
        if self.fail.swap(false, std::sync::atomic::Ordering::AcqRel) {
            return Err(storage::StorageError::Database(
                "injected list failure".into(),
            ));
        }
        self.store.list_recording_metadata(id)
    }
    fn update_recording(&self, value: &domain::EegRecording) -> storage::StorageResult<()> {
        self.store.update_recording(value)
    }
    fn delete_recording(&self, id: RecordingId) -> storage::StorageResult<()> {
        self.store.delete_recording(id)
    }
}
struct FailingListFactory {
    store: storage::ProjectStorage,
    fail: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl ProjectFactory for FailingListFactory {
    fn open(&self, _: &Path) -> AppResult<Box<dyn ProjectRepository>> {
        Ok(Box::new(FailingListRepository {
            store: self.store.clone(),
            fail: self.fail.clone(),
        }))
    }
}

#[test]
fn failed_refresh_reports_committed_recording_and_reopen_recovers_listing() {
    use storage::RecordingRepository;
    let project = TestProject::new();
    let store = storage::ProjectStorage::open(&project.0).unwrap();
    let fail = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let service = ApplicationService::new(
        Box::new(FailingListFactory {
            store: store.clone(),
            fail: fail.clone(),
        }),
        Box::new(CoreEngine),
    );
    let controller = AppController::start(service).unwrap();
    let sender = controller.dispatcher();
    sender
        .submit(AppCommand::OpenProject(project.0.clone()))
        .unwrap();
    assert!(matches!(terminal(&controller), AppEvent::Completed(_)));
    sender
        .submit(AppCommand::CreateSubject(SubjectDraft {
            age: None,
            sex: Sex::Unknown,
            notes: String::new(),
        }))
        .unwrap();
    assert!(matches!(terminal(&controller), AppEvent::Completed(_)));
    fail.store(true, std::sync::atomic::Ordering::Release);
    sender
        .submit(AppCommand::ImportEeg {
            path: project.csv(&["Fp1"], 2048, 256.0),
            format: ImportFormat::Csv(eeg_io::CsvOptions::new(256.0)),
            metadata: RecordingMetadata::default(),
        })
        .unwrap();
    let AppEvent::Failed {
        error,
        snapshot: Some(snapshot),
    } = terminal(&controller)
    else {
        panic!("应发布失败及最新快照")
    };
    assert!(error.to_string().contains("刷新录制列表"));
    let raw = snapshot.raw.as_ref().unwrap();
    assert_eq!(store.get_recording(raw.id).unwrap().unwrap(), **raw);
    sender
        .submit(AppCommand::OpenProject(project.0.clone()))
        .unwrap();
    let AppEvent::Completed(snapshot) = terminal(&controller) else {
        panic!("重新打开应恢复")
    };
    assert_eq!(snapshot.recordings.len(), 1);
    assert_eq!(snapshot.recordings[0].id, raw.id);
}
