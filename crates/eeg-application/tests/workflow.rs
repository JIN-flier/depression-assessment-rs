mod support;
use eeg_application::*;
use std::sync::Arc;
use storage::{ProjectStorage, RecordingRepository, SubjectRepository};
use support::*;

#[test]
fn complete_workflow_preserves_raw_and_persists_separate_processed_recording() {
    let project = TestProject::new();
    let mut service = imported(&project, &["Fp1", "Fp2", "Cz", "O1", "O2"], 2048);
    let imported = service.snapshot();
    let raw = imported.raw.clone().unwrap();
    assert!(imported.plots.waveform.is_some());
    assert_eq!(
        imported.plots.waveform.as_ref().unwrap().traces[0]
            .points
            .len(),
        1280,
        "保留完整 5s×256Hz 采样，不能被旧的 1000 点上限截断"
    );
    let mut stages = Vec::new();
    let snapshot = service
        .execute(AppCommand::Analyze(analysis()), &mut |stage| {
            stages.push(stage)
        })
        .unwrap();
    let analysis = snapshot.analysis.unwrap();
    assert!(stages.len() >= 5);
    assert_eq!(*raw, *service.snapshot().raw.unwrap());
    assert!(Arc::ptr_eq(&raw, service.snapshot().raw.as_ref().unwrap()));
    assert_ne!(raw.id, analysis.processed.id);
    assert_eq!(analysis.processed.sampling_rate_hz, 128.0);
    assert_eq!(analysis.processed.sample_count(), 1024);
    assert_eq!(analysis.processed.processing_history.len(), 4);
    assert_eq!(analysis.features.recording_id, analysis.processed.id);
    assert_eq!(analysis.raw_quality.recording_id, raw.id);
    assert_eq!(
        analysis.processed_quality.recording_id,
        analysis.processed.id
    );
    assert!(snapshot.plots.psd.is_some());
    assert!(snapshot.plots.topomap.is_some());
    assert_eq!(snapshot.plots.topomap.as_ref().unwrap().resolution, 256);
    let map = snapshot.plots.topomap.as_ref().unwrap();
    assert_eq!(map.coverage, TopomapCoverage::HeadCircle);
    for (index, value) in map.values.iter().enumerate() {
        let x = -1.0 + 2.0 * ((index % map.resolution) as f64 + 0.5) / map.resolution as f64;
        let y = 1.0 - 2.0 * ((index / map.resolution) as f64 + 0.5) / map.resolution as f64;
        assert_eq!(value.is_some(), x.hypot(y) <= 1.0, "桌面默认应填满整个头圆");
    }
    let alpha = analysis
        .features
        .spectral
        .as_ref()
        .unwrap()
        .band_power_by_channel["Fp1"][&FrequencyBand::Alpha];
    assert!(alpha.relative.get() > 0.85, "10 Hz 正弦应以 Alpha 功率为主");
    let saved = run(&mut service, AppCommand::SaveProcessed);
    assert_eq!(saved.saved_processed, Some(analysis.processed.id));
    assert_eq!(saved.recordings.len(), 2);
    assert_eq!(
        run(&mut service, AppCommand::SaveProcessed)
            .recordings
            .len(),
        2
    );
    drop(service);
    let store = ProjectStorage::open(&project.0).unwrap();
    assert_eq!(store.get_recording(raw.id).unwrap().unwrap(), *raw);
    assert_eq!(
        store.get_recording(analysis.processed.id).unwrap().unwrap(),
        *analysis.processed
    );
    assert_eq!(store.list_subjects().unwrap().len(), 1);
}

#[test]
fn invalid_commands_preserve_last_successful_analysis_and_plots() {
    let project = TestProject::new();
    let mut service = imported(&project, &["Fp1", "Fp2", "Cz"], 2048);
    let before = run(&mut service, AppCommand::Analyze(analysis()));
    let mut request = analysis();
    request.pipeline.steps = vec![eeg_signal::SignalStep::notch(1000.0)];
    assert!(
        service
            .execute(AppCommand::Analyze(request), &mut |_| {})
            .is_err()
    );
    assert!(
        service
            .execute(
                AppCommand::SetView(ViewRequest {
                    channels: Some(vec!["unknown".into()]),
                    ..Default::default()
                }),
                &mut |_| {}
            )
            .is_err()
    );
    assert!(
        service
            .execute(
                AppCommand::SetView(ViewRequest {
                    start_seconds: 1000.0,
                    ..Default::default()
                }),
                &mut |_| {}
            )
            .is_err()
    );
    assert!(
        service
            .execute(
                AppCommand::OpenProject(std::path::PathBuf::new()),
                &mut |_| {}
            )
            .is_err()
    );
    let after = service.snapshot();
    assert!(Arc::ptr_eq(
        before.analysis.as_ref().unwrap(),
        after.analysis.as_ref().unwrap()
    ));
    assert!(Arc::ptr_eq(&before.plots, &after.plots));
    assert_eq!(before.view, after.view);
}

#[test]
fn display_updates_reuse_features_and_do_not_mutate_samples() {
    let project = TestProject::new();
    let mut service = imported(&project, &["Fp1", "Fp2", "Cz"], 2048);
    let before = run(&mut service, AppCommand::Analyze(analysis()));
    let updated = run(
        &mut service,
        AppCommand::SetView(ViewRequest {
            channels: Some(vec!["Cz".into()]),
            start_seconds: 1.0,
            window_seconds: 2.0,
            amplitude_uv: 20.0,
            band: FrequencyBand::Beta,
            relative_power: false,
            processed: false,
        }),
    );
    assert!(Arc::ptr_eq(
        before.analysis.as_ref().unwrap(),
        updated.analysis.as_ref().unwrap()
    ));
    assert!(Arc::ptr_eq(
        before.raw.as_ref().unwrap(),
        updated.raw.as_ref().unwrap()
    ));
    assert_eq!(updated.view.amplitude_uv, 20.0);
    let waveform = updated.plots.waveform.as_ref().unwrap();
    assert_eq!(waveform.traces.len(), 1);
    assert_eq!(waveform.traces[0].channel, "Cz");
    assert!(updated.plots.topomap.is_some());
    assert_ne!(before.plots.waveform, updated.plots.waveform);
}

#[test]
fn unknown_or_insufficient_topomap_positions_do_not_block_analysis() {
    for labels in [&["unknown1", "unknown2"][..], &["Fp1"][..]] {
        let project = TestProject::new();
        let mut service = imported(&project, labels, 2048);
        let snapshot = run(
            &mut service,
            AppCommand::Analyze(AnalysisRequest::default()),
        );
        assert!(snapshot.analysis.is_some());
        assert!(snapshot.plots.psd.is_some());
        assert!(snapshot.plots.topomap.is_none());
        assert!(
            snapshot
                .plots
                .warnings
                .iter()
                .any(|w| w.contains("地形图不可用"))
        );
    }
}

#[test]
fn selecting_subject_or_recording_clears_incompatible_results_and_deletion_is_guarded() {
    let project = TestProject::new();
    let mut service = imported(&project, &["Fp1", "Fp2", "Cz"], 2048);
    let first = run(&mut service, AppCommand::Analyze(analysis()));
    let subject_id = first.selected_subject.unwrap();
    let raw_id = first.raw.unwrap().id;
    assert!(
        service
            .execute(AppCommand::DeleteSubject(subject_id), &mut |_| {})
            .is_err()
    );
    let second = run(
        &mut service,
        AppCommand::CreateSubject(SubjectDraft {
            age: None,
            sex: Sex::Unknown,
            notes: "第二受试者".into(),
        }),
    );
    assert!(second.raw.is_none());
    assert!(second.analysis.is_none());
    assert!(second.plots.waveform.is_none());
    assert!(
        service
            .execute(AppCommand::LoadRecording(raw_id), &mut |_| {})
            .is_err()
    );
    assert!(
        service
            .execute(AppCommand::DeleteRecording(raw_id), &mut |_| {})
            .is_err()
    );
    run(&mut service, AppCommand::SelectSubject(subject_id));
    let loaded = run(&mut service, AppCommand::LoadRecording(raw_id));
    assert!(loaded.analysis.is_none());
    let deleted = run(&mut service, AppCommand::DeleteRecording(raw_id));
    assert!(deleted.raw.is_none());
    let deleted = run(&mut service, AppCommand::DeleteSubject(subject_id));
    assert!(deleted.selected_subject.is_none());
}

#[test]
fn repeated_analysis_starts_from_raw_and_project_reopen_restores_subjects_and_recordings() {
    let project = TestProject::new();
    let mut service = imported(&project, &["Fp1", "Fp2", "Cz"], 2048);
    let first = run(&mut service, AppCommand::Analyze(analysis()))
        .analysis
        .unwrap();
    let second = run(&mut service, AppCommand::Analyze(analysis()))
        .analysis
        .unwrap();
    assert_eq!(first.processed.samples, second.processed.samples);
    assert_eq!(first.features.spectral, second.features.spectral);
    assert_eq!(
        first.processed.processing_history.len(),
        second.processed.processing_history.len()
    );
    assert_ne!(first.processed.id, second.processed.id);
    let id = service.snapshot().selected_subject.unwrap();
    run(&mut service, AppCommand::OpenProject(project.0.clone())); // 同目录复用句柄
    drop(service);
    let mut reopened = ApplicationService::default();
    let opened = run(&mut reopened, AppCommand::OpenProject(project.0.clone()));
    assert_eq!(opened.subjects.len(), 1);
    assert!(opened.raw.is_none());
    assert_eq!(
        run(&mut reopened, AppCommand::SelectSubject(id))
            .recordings
            .len(),
        1
    );
}

#[test]
fn standalone_quality_and_subject_edit_keep_existing_recording_intact() {
    let project = TestProject::new();
    let mut service = imported(&project, &["Fp1"], 2048);
    let before = service.snapshot();
    let quality = run(
        &mut service,
        AppCommand::AssessQuality(QualityConfig::default()),
    );
    assert!(quality.quality.is_some());
    assert!(quality.analysis.is_none());
    let edited = run(
        &mut service,
        AppCommand::UpdateSubject {
            id: before.selected_subject.unwrap(),
            draft: SubjectDraft {
                age: Some(41),
                sex: Sex::Male,
                notes: "已编辑".into(),
            },
        },
    );
    assert_eq!(edited.subjects[0].age, Some(41));
    assert_eq!(edited.subjects[0].created_at, before.subjects[0].created_at);
    assert!(Arc::ptr_eq(
        edited.raw.as_ref().unwrap(),
        before.raw.as_ref().unwrap()
    ));
}

#[test]
fn too_short_recording_imports_and_views_but_analysis_fails_without_false_success() {
    let project = TestProject::new();
    let mut service = imported(&project, &["Fp1"], 10);
    assert!(
        service
            .execute(AppCommand::Analyze(AnalysisRequest::default()), &mut |_| {})
            .is_err()
    );
    assert!(service.snapshot().analysis.is_none());
    assert!(service.snapshot().plots.waveform.is_some());
}

#[test]
fn bad_channel_exclusion_is_explicit_and_keeps_all_recorded_samples() {
    let project = TestProject::new();
    let mut service = setup(&project);
    let path = project.csv(&["Fp1", "Fp2", "Cz", "O1"], 2048, 256.0);
    let original = std::fs::read_to_string(&path).unwrap();
    let text = original
        .lines()
        .enumerate()
        .map(|(i, line)| {
            if i == 0 {
                line.to_string()
            } else {
                format!("0,{}", line.split_once(',').unwrap().1)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&path, text).unwrap();
    import_csv(&mut service, path, 256.0);
    let request = AnalysisRequest {
        exclude_bad_channels: true,
        ..Default::default()
    };
    let snapshot = run(&mut service, AppCommand::Analyze(request));
    let analysis = snapshot.analysis.unwrap();
    assert!(analysis.raw_quality.bad_channels.contains(&"Fp1".into()));
    assert!(
        !analysis
            .features
            .spectral
            .as_ref()
            .unwrap()
            .psd_by_channel
            .contains_key("Fp1")
    );
    assert_eq!(analysis.processed.channels.len(), 4);
    assert_eq!(snapshot.raw.unwrap().channels.len(), 4);
    assert_eq!(analysis.processed.samples[0], vec![0.0; 2048]);
}

#[test]
fn different_project_resets_state_and_alias_of_open_directory_reuses_database() {
    let project = TestProject::new();
    let other = TestProject::new();
    let mut service = imported(&project, &["Fp1"], 2048);
    let before = service.snapshot();
    let alias = run(&mut service, AppCommand::OpenProject(project.0.join(".")));
    assert!(Arc::ptr_eq(
        alias.raw.as_ref().unwrap(),
        before.raw.as_ref().unwrap()
    ));
    let switched = run(&mut service, AppCommand::OpenProject(other.0.clone()));
    assert!(switched.subjects.is_empty());
    assert!(switched.raw.is_none());
    assert!(switched.selected_subject.is_none());
    assert!(switched.plots.waveform.is_none());
}

#[test]
fn dto_keeps_literal_channel_label_and_single_sample_view() {
    let project = TestProject::new();
    let mut service = imported(&project, &["<script>&"], 2048);
    let snapshot = run(
        &mut service,
        AppCommand::SetView(ViewRequest {
            window_seconds: 1.0 / 256.0,
            ..Default::default()
        }),
    );
    let trace = &snapshot.plots.waveform.as_ref().unwrap().traces[0];
    assert_eq!(trace.channel, "<script>&"); // 标签是文本，绝不作为可执行图像标记解析。
    assert_eq!(trace.points.len(), 1);
}

#[test]
fn analysis_at_raw_tail_clamps_view_to_resampled_grid_without_discarding_success() {
    let project = TestProject::new();
    let mut service = imported(&project, &["Fp1"], 2048);
    run(
        &mut service,
        AppCommand::SetView(ViewRequest {
            start_seconds: 2047.0 / 256.0,
            ..Default::default()
        }),
    );
    let analyzed = run(&mut service, AppCommand::Analyze(analysis()));
    assert!(analyzed.analysis.is_some());
    assert_eq!(analyzed.view.start_seconds, 1023.0 / 128.0);
    assert_eq!(
        analyzed.plots.waveform.as_ref().unwrap().traces[0]
            .points
            .len(),
        1
    );
}

#[test]
fn all_channels_are_default_and_empty_selection_does_not_mean_all() {
    let project = TestProject::new();
    let labels = [
        "Fp1", "Fp2", "Cz", "O1", "O2", "F3", "F4", "F7", "F8", "P3", "P4", "Pz",
    ];
    let mut service = imported(&project, &labels, 2048);
    let imported = service.snapshot();
    assert_eq!(imported.view.channels, None);
    let waveform = imported.plots.waveform.as_ref().unwrap();
    assert_eq!(
        waveform
            .traces
            .iter()
            .map(|t| t.channel.as_str())
            .collect::<Vec<_>>(),
        labels
    );
    let analyzed = run(&mut service, AppCommand::Analyze(analysis()));
    assert_eq!(
        analyzed.plots.psd.as_ref().unwrap().traces.len(),
        labels.len()
    );
    let hidden = run(
        &mut service,
        AppCommand::SetView(ViewRequest {
            channels: Some(vec![]),
            ..analyzed.view.clone()
        }),
    );
    assert!(hidden.plots.waveform.is_none());
    assert!(hidden.plots.psd.is_none());
    assert!(hidden.plots.topomap.is_some()); // 空间图仍沿用所有分析导联。
    assert!(Arc::ptr_eq(
        analyzed.analysis.as_ref().unwrap(),
        hidden.analysis.as_ref().unwrap()
    ));
    let restored = run(
        &mut service,
        AppCommand::SetView(ViewRequest {
            channels: Some(labels.iter().map(|s| s.to_string()).collect()),
            ..hidden.view.clone()
        }),
    );
    assert_eq!(
        restored.plots.waveform.as_ref().unwrap().traces.len(),
        labels.len()
    );
    assert_eq!(
        restored.plots.psd.as_ref().unwrap().traces.len(),
        labels.len()
    );
    assert!(Arc::ptr_eq(
        analyzed.analysis.as_ref().unwrap(),
        restored.analysis.as_ref().unwrap()
    ));
    assert!(Arc::ptr_eq(
        imported.raw.as_ref().unwrap(),
        restored.raw.as_ref().unwrap()
    ));
}
