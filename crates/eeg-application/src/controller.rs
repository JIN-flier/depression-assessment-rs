//! 有界后台调度：一个 worker 串行拥有应用服务，UI 只提交命令/消费事件。
//! Busy 保持到终态事件被取走，避免旧快照覆盖随后提交的命令。
use crate::*;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError},
    },
    thread,
};

#[derive(Debug)]
pub enum AppEvent {
    Progress(&'static str),
    Completed(Box<AppSnapshot>),
    Failed {
        error: AppError,
        snapshot: Option<Box<AppSnapshot>>,
    },
}
#[derive(Clone)]
pub struct CommandDispatcher {
    sender: SyncSender<Option<AppCommand>>,
    busy: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
}
impl CommandDispatcher {
    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }
    /// 不等待锁、不等待队列空间。重复点击被明确拒绝，而非堆积昂贵分析。
    pub fn submit(&self, command: AppCommand) -> AppResult<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(AppError::WorkerStopped);
        }
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(AppError::Busy);
        }
        if self.sender.try_send(Some(command)).is_err() {
            self.busy.store(false, Ordering::Release);
            return Err(AppError::WorkerStopped);
        }
        Ok(())
    }
}

pub struct AppController {
    dispatcher: CommandDispatcher,
    events: Receiver<AppEvent>,
}
impl AppController {
    pub fn start(mut service: ApplicationService) -> AppResult<Self> {
        let (commands_tx, commands_rx) = mpsc::sync_channel(1);
        // 每个任务只有有限个阶段事件；busy 门控使终态消费前无法开始新任务。
        let (events_tx, events_rx) = mpsc::channel();
        let busy = Arc::new(AtomicBool::new(false));
        let closed = Arc::new(AtomicBool::new(false));
        let worker_closed = closed.clone();
        thread::Builder::new()
            .name("eeg-workflow".into())
            .spawn(move || {
                while let Ok(Some(command)) = commands_rx.recv() {
                    if worker_closed.load(Ordering::Acquire) {
                        break;
                    }
                    let result = service.execute(command, &mut |stage| {
                        let _ = events_tx.send(AppEvent::Progress(stage));
                    });
                    let event = match result {
                        Ok(snapshot) => AppEvent::Completed(Box::new(snapshot)),
                        // 写入成功、刷新列表失败时，磁盘操作不能被假装回滚。
                        // 错误事件同时传当前快照，使 UI 与已发生的副作用一致。
                        Err(error) => AppEvent::Failed {
                            error,
                            snapshot: Some(Box::new(service.snapshot())),
                        },
                    };
                    if events_tx.send(event).is_err() {
                        break;
                    }
                }
                worker_closed.store(true, Ordering::Release);
            })
            .map_err(|e| AppError::service("启动后台任务", e))?;
        Ok(Self {
            dispatcher: CommandDispatcher {
                sender: commands_tx,
                busy,
                closed,
            },
            events: events_rx,
        })
    }
    pub fn dispatcher(&self) -> CommandDispatcher {
        self.dispatcher.clone()
    }
    /// 由 UI timer 调用；返回 None 表示暂无事件。worker 意外退出也能解除 busy。
    pub fn try_next_event(&self) -> Option<AppEvent> {
        match self.events.try_recv() {
            Ok(event) => {
                if matches!(event, AppEvent::Completed(_) | AppEvent::Failed { .. }) {
                    self.dispatcher.busy.store(false, Ordering::Release);
                }
                Some(event)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                if self.dispatcher.busy.swap(false, Ordering::AcqRel) {
                    Some(AppEvent::Failed {
                        error: AppError::WorkerStopped,
                        snapshot: None,
                    })
                } else {
                    None
                }
            }
        }
    }
}

impl Drop for AppController {
    fn drop(&mut self) {
        // 不在 UI 主线程 join 耗时算法。关闭标志阻止残留 dispatcher 继续提交，
        // None 唤醒空闲 worker；进行中的任务结束后会因事件通道关闭而退出。
        self.dispatcher.closed.store(true, Ordering::Release);
        self.dispatcher.busy.store(false, Ordering::Release);
        let _ = self.dispatcher.sender.try_send(None);
    }
}
