//! 真实 EEG 工作流到报告的集成测试；仅 Provider 被替换，算法、存储和校验均真实运行。
mod support;
use eeg_application::*;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};
use support::*;

fn candidate(input: &NarrationInput) -> String {
    serde_json::json!({
        "summary": {"text": "已有分析结果如下：{{raw_quality_score}}；{{processed_quality_score}}。", "fact_ids": ["raw_quality_score", "processed_quality_score"]},
        "description": {"text": "测量结果如下：{{channel_1.alpha.relative}}。", "fact_ids": ["channel_1.alpha.relative"]},
        "interpretation": input.interpretation, "limitations": input.limitations,
    }).to_string()
}
#[derive(Clone, Copy)]
enum Outcome {
    Ok,
    Timeout,
    Unsafe,
}
struct ScriptedNarrator {
    outcomes: Mutex<VecDeque<Outcome>>,
    inputs: Arc<Mutex<Vec<NarrationInput>>>,
}
impl ReportNarrator for ScriptedNarrator {
    fn narrate(&self, input: &NarrationInput) -> Result<String, LlmError> {
        self.inputs.lock().unwrap().push(input.clone());
        match self
            .outcomes
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Outcome::Ok)
        {
            Outcome::Ok => Ok(candidate(input)),
            Outcome::Timeout => Err(LlmError::Timeout),
            Outcome::Unsafe => Ok(candidate(input).replace("已有分析结果如下", "重度抑郁")),
        }
    }
    fn provider_name(&self) -> &'static str {
        "test"
    }
}
fn service(
    project: &TestProject,
    outcomes: &[Outcome],
) -> (ApplicationService, Arc<Mutex<Vec<NarrationInput>>>) {
    let inputs = Arc::new(Mutex::new(vec![]));
    let reports = ReportService::new(Box::new(ScriptedNarrator {
        outcomes: Mutex::new(outcomes.iter().copied().collect()),
        inputs: inputs.clone(),
    }));
    let mut service = imported(project, &["Fp1", "Fp2", "Cz"], 2048).with_report_service(reports);
    run(&mut service, AppCommand::Analyze(analysis()));
    (service, inputs)
}

#[test]
fn structured_report_generation_never_recomputes_or_mutates_eeg_and_is_private() {
    let project = TestProject::new();
    let (mut service, inputs) = service(&project, &[Outcome::Ok]);
    let before = service.snapshot();
    let after = run(&mut service, AppCommand::GenerateReport);
    assert!(Arc::ptr_eq(
        before.analysis.as_ref().unwrap(),
        after.analysis.as_ref().unwrap()
    ));
    assert!(Arc::ptr_eq(
        before.raw.as_ref().unwrap(),
        after.raw.as_ref().unwrap()
    ));
    assert!(Arc::ptr_eq(&before.plots, &after.plots));
    let document = after.report.unwrap();
    assert_eq!(
        document.context().subject.subject_id,
        after.selected_subject.unwrap()
    );
    assert!(!document.context().integrated_assessment.conclusion_allowed);
    assert_eq!(document.context().metadata["channel_1"], "Fp1");
    let input = inputs.lock().unwrap();
    let prompt = serde_json::to_string(&input[0]).unwrap();
    for secret in [
        "Fp1".to_string(),
        "合成测试".into(),
        before.raw.unwrap().id.to_string(),
        after.selected_subject.unwrap().to_string(),
    ] {
        assert!(!prompt.contains(&secret));
    }
    assert_eq!(
        input[0].facts["raw_quality_score"],
        format!(
            "原始信号质量评分：{:.3}%",
            before.analysis.unwrap().raw_quality.overall_score.get() * 100.0
        )
    );
}

#[test]
fn failures_keep_last_valid_document_and_retry_can_publish_a_new_one() {
    let project = TestProject::new();
    let (mut service, _) = service(
        &project,
        &[Outcome::Ok, Outcome::Timeout, Outcome::Unsafe, Outcome::Ok],
    );
    let before = run(&mut service, AppCommand::GenerateReport);
    for _ in 0..2 {
        assert!(
            service
                .execute(AppCommand::GenerateReport, &mut |_| {})
                .is_err()
        );
        let after = service.snapshot();
        assert!(Arc::ptr_eq(
            before.report.as_ref().unwrap(),
            after.report.as_ref().unwrap()
        ));
        assert!(Arc::ptr_eq(
            before.analysis.as_ref().unwrap(),
            after.analysis.as_ref().unwrap()
        ));
    }
    let retried = run(&mut service, AppCommand::GenerateReport);
    assert!(!Arc::ptr_eq(
        before.report.as_ref().unwrap(),
        retried.report.as_ref().unwrap()
    ));
}

#[test]
fn view_and_save_keep_report_but_successful_data_changes_invalidate_it() {
    let project = TestProject::new();
    let (mut service, _) = service(&project, &[]);
    let before = run(&mut service, AppCommand::GenerateReport);
    let mut view = before.view.clone();
    view.channels = Some(vec!["Fp1".into()]);
    let after = run(&mut service, AppCommand::SetView(view));
    assert!(Arc::ptr_eq(
        before.report.as_ref().unwrap(),
        after.report.as_ref().unwrap()
    ));
    assert!(
        run(&mut service, AppCommand::SaveProcessed)
            .report
            .is_some()
    );
    // 失败的分析不会提前删除报告。
    let mut bad = analysis();
    bad.pipeline.steps = vec![eeg_signal::SignalStep::notch(1000.0)];
    assert!(
        service
            .execute(AppCommand::Analyze(bad), &mut |_| {})
            .is_err()
    );
    assert!(service.snapshot().report.is_some());
    assert!(
        run(&mut service, AppCommand::Analyze(analysis()))
            .report
            .is_none()
    );
    run(&mut service, AppCommand::GenerateReport);
    assert!(
        run(
            &mut service,
            AppCommand::UpdateSubject {
                id: before.selected_subject.unwrap(),
                draft: SubjectDraft {
                    age: Some(33),
                    sex: Sex::Female,
                    notes: "修改".into()
                }
            }
        )
        .report
        .is_none()
    );
    run(&mut service, AppCommand::GenerateReport);
    assert!(
        run(
            &mut service,
            AppCommand::AssessQuality(QualityConfig::default())
        )
        .report
        .is_none()
    );
    run(&mut service, AppCommand::GenerateReport);
    assert!(
        run(
            &mut service,
            AppCommand::LoadRecording(before.raw.unwrap().id)
        )
        .report
        .is_none()
    );
    assert!(
        service
            .execute(AppCommand::GenerateReport, &mut |_| {})
            .is_err()
    );
}

#[test]
fn context_builder_rejects_cross_subject_and_cross_recording_results() {
    let project = TestProject::new();
    let (service, _) = service(&project, &[]);
    let state = service.snapshot();
    let subject = state.subjects[0].clone();
    let analysis = state.analysis.unwrap();
    let id = state.raw.unwrap().id;
    assert!(build_v1_report_context(&subject, RecordingId::new(), &analysis).is_err());
    let mut other = subject.clone();
    other.id = SubjectId::new();
    assert!(build_v1_report_context(&other, id, &analysis).is_err());
    let mut other = (*analysis).clone();
    other.features.recording_id = RecordingId::new();
    assert!(build_v1_report_context(&subject, id, &other).is_err());
}

#[test]
fn default_provider_fails_clearly_without_credentials_and_preserves_analysis() {
    let project = TestProject::new();
    let mut service = imported(&project, &["Fp1", "Fp2", "Cz"], 2048)
        .with_report_service(ReportService::new(Box::new(llm::UnconfiguredNarrator)));
    let before = run(&mut service, AppCommand::Analyze(analysis()));
    assert!(
        service
            .execute(AppCommand::GenerateReport, &mut |_| {})
            .unwrap_err()
            .to_string()
            .contains("未配置 LLM")
    );
    assert!(Arc::ptr_eq(
        before.analysis.as_ref().unwrap(),
        service.snapshot().analysis.as_ref().unwrap()
    ));
}

#[test]
fn slow_report_provider_keeps_dispatch_nonblocking_and_busy_until_terminal_event() {
    struct Gate {
        entered: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
    }
    impl ReportNarrator for Gate {
        fn narrate(&self, input: &NarrationInput) -> Result<String, LlmError> {
            self.entered.send(()).unwrap();
            self.release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            Ok(candidate(input))
        }
        fn provider_name(&self) -> &'static str {
            "test"
        }
    }
    let project = TestProject::new();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (service, _) = service(&project, &[]);
    let controller = AppController::start(service.with_report_service(ReportService::new(
        Box::new(Gate {
            entered: entered_tx,
            release: Mutex::new(release_rx),
        }),
    )))
    .unwrap();
    let dispatcher = controller.dispatcher();
    dispatcher.submit(AppCommand::GenerateReport).unwrap();
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(matches!(
        dispatcher.submit(AppCommand::GenerateReport),
        Err(AppError::Busy)
    ));
    release_tx.send(()).unwrap();
    assert!(dispatcher.is_busy());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(event) = controller.try_next_event() {
            match event {
                AppEvent::Completed(state) => {
                    assert!(state.report.is_some());
                    break;
                }
                AppEvent::Failed { error, .. } => panic!("{error}"),
                AppEvent::Progress(_) => {}
            }
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!dispatcher.is_busy());
}
