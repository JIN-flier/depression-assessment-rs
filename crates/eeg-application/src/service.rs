//! 可回放的同步应用服务。每个命令先计算候选结果，成功后发布完整快照。
//! 此对象仅由一个后台 worker 拥有，因此不存在 UI 与数据库的共享可变状态。
use crate::*;
use domain::{EegRecording, Subject};
use eeg_io::ImportContext;
use std::{path::Path, sync::Arc, time::SystemTime};
use storage::{ProjectStorage, RecordingRepository, SubjectRepository};

pub trait ProjectRepository: SubjectRepository + RecordingRepository {}
impl<T: SubjectRepository + RecordingRepository> ProjectRepository for T {}
pub trait ProjectFactory: Send {
    fn open(&self, path: &Path) -> AppResult<Box<dyn ProjectRepository>>;
}
#[derive(Default)]
pub struct RedbProjectFactory;
impl ProjectFactory for RedbProjectFactory {
    fn open(&self, path: &Path) -> AppResult<Box<dyn ProjectRepository>> {
        Ok(Box::new(
            ProjectStorage::open(path).map_err(|e| AppError::service("打开项目", e))?,
        ))
    }
}

pub struct ApplicationService {
    factory: Box<dyn ProjectFactory>,
    engine: Box<dyn EegEngine>,
    project: Option<Box<dyn ProjectRepository>>,
    state: AppSnapshot,
}
impl Default for ApplicationService {
    fn default() -> Self {
        Self::new(Box::new(RedbProjectFactory), Box::new(CoreEngine))
    }
}
impl ApplicationService {
    pub fn new(factory: Box<dyn ProjectFactory>, engine: Box<dyn EegEngine>) -> Self {
        Self {
            factory,
            engine,
            project: None,
            state: AppSnapshot::default(),
        }
    }
    pub fn snapshot(&self) -> AppSnapshot {
        self.state.clone()
    }
    fn project(&self) -> AppResult<&dyn ProjectRepository> {
        self.project
            .as_deref()
            .ok_or(AppError::InvalidState("请先打开或创建项目"))
    }
    fn raw(&self) -> AppResult<&EegRecording> {
        self.state
            .raw
            .as_deref()
            .ok_or(AppError::InvalidState("请先导入或选择 EEG 录制"))
    }
    fn clear_recording(&mut self) {
        self.state.raw = None;
        self.state.quality = None;
        self.state.analysis = None;
        self.state.saved_processed = None;
        self.state.view = ViewRequest::default();
        self.state.plots = Arc::new(PlotData::default());
    }
    fn publish_recording(&mut self, recording: EegRecording, plots: PlotData) {
        self.clear_recording();
        self.state.raw = Some(Arc::new(recording));
        self.state.plots = Arc::new(plots);
    }
    /// progress 只携带阶段名，避免日志/状态提示泄漏受试者身份或样本。
    pub fn execute(
        &mut self,
        command: AppCommand,
        progress: &mut dyn FnMut(&'static str),
    ) -> AppResult<AppSnapshot> {
        match command {
            AppCommand::OpenProject(path) => {
                if path.as_os_str().is_empty() {
                    return Err(AppError::InvalidInput("项目目录不能为空".into()));
                }
                // 同一路径重开复用数据库句柄，避免 redb 同进程重复锁定。
                if self.state.project_path.as_ref().is_some_and(|open| {
                    *open == path
                        || std::fs::canonicalize(open)
                            .ok()
                            .zip(std::fs::canonicalize(&path).ok())
                            .is_some_and(|(a, b)| a == b)
                }) {
                    self.state.subjects = self
                        .project()?
                        .list_subjects()
                        .map_err(|e| AppError::service("受试者列表", e))?;
                    self.refresh_recordings()?;
                    return Ok(self.snapshot());
                }
                progress("打开项目");
                let project = self.factory.open(&path)?;
                let subjects = project
                    .list_subjects()
                    .map_err(|e| AppError::service("受试者列表", e))?;
                self.project = Some(project);
                self.state = AppSnapshot {
                    project_path: Some(path),
                    subjects,
                    ..Default::default()
                };
            }
            AppCommand::CreateSubject(draft) => {
                let subject = make_subject(SubjectId::new(), draft)?;
                self.project()?
                    .create_subject(&subject)
                    .map_err(|e| AppError::service("创建受试者", e))?;
                self.state.subjects.push(subject.clone());
                self.state.subjects.sort_by_key(|s| s.id);
                self.clear_recording();
                self.state.selected_subject = Some(subject.id);
                self.state.recordings.clear();
            }
            AppCommand::UpdateSubject { id, draft } => {
                let mut subject = self
                    .project()?
                    .get_subject(id)
                    .map_err(|e| AppError::service("查询受试者", e))?
                    .ok_or(AppError::InvalidState("受试者不存在"))?;
                // 验证年龄，同时保留创建时间和非表单 metadata。
                let validated = make_subject(id, draft)?;
                subject.age = validated.age;
                subject.sex = validated.sex;
                subject.metadata.insert(
                    "notes".into(),
                    validated.metadata.get("notes").cloned().unwrap_or_default(),
                );
                self.project()?
                    .update_subject(&subject)
                    .map_err(|e| AppError::service("更新受试者", e))?;
                if let Some(old) = self.state.subjects.iter_mut().find(|s| s.id == id) {
                    *old = subject;
                }
            }
            AppCommand::DeleteSubject(id) => {
                self.project()?
                    .delete_subject(id)
                    .map_err(|e| AppError::service("删除受试者", e))?;
                self.state.subjects.retain(|s| s.id != id);
                if self.state.selected_subject == Some(id) {
                    self.clear_recording();
                    self.state.selected_subject = None;
                    self.state.recordings.clear();
                }
            }
            AppCommand::SelectSubject(id) => {
                if !self.state.subjects.iter().any(|s| s.id == id) {
                    return Err(AppError::InvalidState("受试者不存在"));
                }
                let recordings = self
                    .project()?
                    .list_recording_metadata(id)
                    .map_err(|e| AppError::service("录制列表", e))?;
                self.clear_recording();
                self.state.selected_subject = Some(id);
                self.state.recordings = recordings;
            }
            AppCommand::ImportEeg {
                path,
                format,
                metadata,
            } => {
                let subject_id = self
                    .state
                    .selected_subject
                    .ok_or(AppError::InvalidState("请先选择受试者"))?;
                self.project()?;
                progress("读取 EEG 文件");
                let mut context = ImportContext::new(RecordingId::new(), subject_id);
                context.metadata = metadata;
                let recording = self.engine.import(&path, &format, &context)?;
                let plots = self
                    .engine
                    .plots(&recording, None, &ViewRequest::default())?;
                self.project()?
                    .create_recording(&recording)
                    .map_err(|e| AppError::service("保存原始录制", e))?;
                self.publish_recording(recording, plots);
                self.refresh_recordings()?;
            }
            AppCommand::LoadRecording(id) => {
                progress("加载录制");
                let recording = self
                    .project()?
                    .get_recording(id)
                    .map_err(|e| AppError::service("加载录制", e))?
                    .ok_or(AppError::InvalidState("录制不存在"))?;
                if Some(recording.subject_id) != self.state.selected_subject {
                    return Err(AppError::InvalidState("录制不属于当前受试者"));
                }
                let plots = self
                    .engine
                    .plots(&recording, None, &ViewRequest::default())?;
                self.publish_recording(recording, plots);
            }
            AppCommand::DeleteRecording(id) => {
                if !self.state.recordings.iter().any(|r| r.id == id) {
                    return Err(AppError::InvalidState("录制不属于当前受试者"));
                }
                self.project()?
                    .delete_recording(id)
                    .map_err(|e| AppError::service("删除录制", e))?;
                self.state.recordings.retain(|r| r.id != id);
                if self.state.raw.as_ref().is_some_and(|r| r.id == id) {
                    self.clear_recording();
                }
                if self.state.saved_processed == Some(id) {
                    self.state.saved_processed = None;
                }
            }
            AppCommand::AssessQuality(config) => {
                progress("原始质量评价");
                let quality = self.engine.quality(self.raw()?, &config)?;
                self.state.quality = Some(Arc::new(quality));
            }
            AppCommand::Analyze(request) => {
                // 始终从选中的原始/已载入录制出发，不对上次分析输出重复滤波。
                let analysis = self.engine.analyze(self.raw()?, &request, progress)?;
                progress("准备波形 / PSD / 地形图");
                let mut view = self.state.view.clone();
                view.processed = true;
                // 原始末尾时间窗在降采样后可能没有对应样本。调整到新网格
                // 的最后采样时刻，使一次成功分析不会因旧显示范围而被丢弃。
                let last = analysis.processed.sample_count().saturating_sub(1) as f64
                    / analysis.processed.sampling_rate_hz;
                view.start_seconds = view.start_seconds.min(last);
                let plots = self.engine.plots(self.raw()?, Some(&analysis), &view)?;
                self.state.quality = Some(Arc::new(analysis.raw_quality.clone()));
                self.state.analysis = Some(Arc::new(analysis));
                self.state.saved_processed = None;
                self.state.view = view;
                self.state.plots = Arc::new(plots);
            }
            AppCommand::SetView(view) => {
                let plots =
                    self.engine
                        .plots(self.raw()?, self.state.analysis.as_deref(), &view)?;
                self.state.view = view;
                self.state.plots = Arc::new(plots);
            }
            AppCommand::SaveProcessed => {
                let analysis = self
                    .state
                    .analysis
                    .as_ref()
                    .ok_or(AppError::InvalidState("请先完成分析"))?;
                if self.state.saved_processed != Some(analysis.processed.id) {
                    self.project()?
                        .create_recording(&analysis.processed)
                        .map_err(|e| AppError::service("保存处理后录制", e))?;
                    self.state.saved_processed = Some(analysis.processed.id);
                    self.refresh_recordings()?;
                }
            }
        }
        Ok(self.snapshot())
    }
    fn refresh_recordings(&mut self) -> AppResult<()> {
        if let Some(id) = self.state.selected_subject {
            self.state.recordings = self
                .project()?
                .list_recording_metadata(id)
                .map_err(|e| AppError::service("刷新录制列表", e))?;
        }
        Ok(())
    }
}
fn make_subject(id: SubjectId, draft: SubjectDraft) -> AppResult<Subject> {
    let mut subject = Subject::new(id, draft.age, draft.sex, SystemTime::now().into())
        .map_err(|e| AppError::service("受试者验证", e))?;
    subject.metadata.insert("notes".into(), draft.notes);
    Ok(subject)
}
