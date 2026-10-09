//! P10 真实算法/存储/报告/文件闭环。只有网络 Provider 是替身。
mod support;
use eeg_application::*;
use std::{
    fs,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
use support::*;
struct Narrator;
impl ReportNarrator for Narrator {
    fn provider_name(&self) -> &'static str {
        "offline-test"
    }
    fn narrate(&self, input: &NarrationInput) -> Result<String, LlmError> {
        Ok(serde_json::json!({
            "summary":{"text":"已有分析结果如下：{{processed_quality_score}}。","fact_ids":["processed_quality_score"]},
            "description":{"text":"测量结果如下：{{channel_1.alpha.relative}}。","fact_ids":["channel_1.alpha.relative"]},
            "interpretation":input.interpretation,"limitations":input.limitations
        }).to_string())
    }
}
fn analyzed(p: &TestProject) -> ApplicationService {
    let mut s = imported(p, &["Fp1", "Fp2", "Cz"], 2048)
        .with_report_service(ReportService::new(Box::new(Narrator)));
    run(&mut s, AppCommand::Analyze(analysis()));
    s
}
fn request(p: &TestProject, name: &str, format: ExportFormat) -> AppCommand {
    AppCommand::ExportReport(ExportRequest {
        destination: p.0.join(name),
        format,
    })
}
#[test]
fn json_csv_export_without_llm_preserves_complete_results_and_original_samples() {
    let p = TestProject::new();
    let mut s = analyzed(&p);
    let before = s.snapshot();
    assert!(before.report.is_none());
    let after = run(&mut s, request(&p, "结果", ExportFormat::Json));
    let receipt = after.last_export.as_ref().unwrap();
    assert_eq!(receipt.destination, p.0.join("结果.json"));
    let bytes = fs::read(&receipt.destination).unwrap();
    assert_eq!(receipt.bytes_written, bytes.len() as u64);
    let archive: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(archive["schema_version"], "eeg-v1-export/1");
    assert!(archive["report"].is_null());
    let a = before.analysis.as_ref().unwrap();
    assert_eq!(
        archive["analysis"]["features"],
        serde_json::to_value(&a.features).unwrap()
    );
    assert_eq!(
        archive["analysis"]["raw_quality"],
        serde_json::to_value(&a.raw_quality).unwrap()
    );
    assert_eq!(
        archive["analysis"]["processed_quality"],
        serde_json::to_value(&a.processed_quality).unwrap()
    );
    assert_eq!(
        archive["analysis"]["parameters"]["pipeline"],
        serde_json::to_value(&a.request.pipeline).unwrap()
    );
    assert_eq!(
        archive["analysis"]["parameters"]["quality"],
        serde_json::to_value(&a.request.quality).unwrap()
    );
    assert_eq!(
        archive["analysis"]["parameters"]["features"],
        serde_json::to_value(&a.request.features).unwrap()
    );
    assert!(archive["analysis"]["raw"].get("samples").is_none());
    assert!(archive["analysis"]["processed"].get("samples").is_none());
    assert!(
        !archive["analysis"]["processed"]["processing_history"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(Arc::ptr_eq(
        before.raw.as_ref().unwrap(),
        after.raw.as_ref().unwrap()
    ));
    assert!(Arc::ptr_eq(
        before.analysis.as_ref().unwrap(),
        after.analysis.as_ref().unwrap()
    ));
    assert!(Arc::ptr_eq(&before.plots, &after.plots));
    run(&mut s, request(&p, "研究.csv", ExportFormat::Csv));
    let csv = fs::read_to_string(p.0.join("研究.csv")).unwrap();
    assert!(csv.starts_with('\u{feff}'));
    assert!(csv.contains("/analysis/features/spectral/psd_by_channel/Fp1/0"));
    assert!(csv.contains("uV^2/Hz"));
    assert!(csv.contains("/analysis/parameters/pipeline/steps/0/type"));
    assert!(csv.contains("/analysis/raw_quality/provenance/algorithm_version"));
}
#[test]
fn all_four_formats_publish_valid_documents_from_one_analysis() {
    let p = TestProject::new();
    let mut s = analyzed(&p);
    let before = run(&mut s, AppCommand::GenerateReport);
    for format in [
        ExportFormat::Json,
        ExportFormat::Csv,
        ExportFormat::Docx,
        ExportFormat::Pdf,
    ] {
        let result = s.execute(
            request(&p, &format!("report.{}", format.extension()), format),
            &mut |_| {},
        );
        // 机器没有安装字体时 PDF 以明确错误结束；有字体的 CI 与预览运行必须真实验证。
        if format == ExportFormat::Pdf
            && std::env::var_os("P10_EXPORT_PREVIEW_DIR").is_none()
            && let Err(error) = &result
        {
            let error = error.to_string();
            if error.contains("未找到 PDF 中文") {
                eprintln!("PDF 字体未安装：{error}");
                continue;
            }
            panic!("{error}");
        }
        let after = result.unwrap();
        let receipt = after.last_export.as_ref().unwrap();
        let bytes = fs::read(&receipt.destination).unwrap();
        assert!(!bytes.is_empty());
        assert!(Arc::ptr_eq(
            before.report.as_ref().unwrap(),
            after.report.as_ref().unwrap()
        ));
        assert!(Arc::ptr_eq(
            before.analysis.as_ref().unwrap(),
            after.analysis.as_ref().unwrap()
        ));
        match format {
            ExportFormat::Pdf => {
                assert!(bytes.starts_with(b"%PDF-1.7"));
                assert!(bytes.ends_with(b"%%EOF\n"));
            }
            ExportFormat::Docx => assert!(bytes.starts_with(b"PK\x03\x04")),
            ExportFormat::Json => {
                let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(
                    v["report"]["summary"],
                    before.report.as_ref().unwrap().narrative().summary()
                );
                assert_eq!(
                    v["report"]["interpretation"],
                    before.report.as_ref().unwrap().narrative().interpretation()
                );
            }
            _ => {}
        }
        if let Some(directory) = std::env::var_os("P10_EXPORT_PREVIEW_DIR") {
            fs::create_dir_all(&directory).unwrap();
            fs::copy(
                &receipt.destination,
                std::path::Path::new(&directory).join(format!("report.{}", format.extension())),
            )
            .unwrap();
        }
    }
}
#[test]
fn missing_report_invalid_path_existing_target_and_io_failures_are_recoverable() {
    let p = TestProject::new();
    let mut s = analyzed(&p);
    assert!(
        s.execute(request(&p, "no-report.pdf", ExportFormat::Pdf), &mut |_| {})
            .is_err()
    );
    assert!(!p.0.join("no-report.pdf").exists());
    let before = run(&mut s, request(&p, "saved.json", ExportFormat::Json));
    let original = fs::read(p.0.join("saved.json")).unwrap();
    for cmd in [
        request(&p, "saved.json", ExportFormat::Json),
        request(&p, "wrong.pdf", ExportFormat::Json),
        request(&p, "missing/subdir.json", ExportFormat::Json),
    ] {
        assert!(s.execute(cmd, &mut |_| {}).is_err());
        let after = s.snapshot();
        assert_eq!(before.last_export, after.last_export);
        assert!(Arc::ptr_eq(
            before.analysis.as_ref().unwrap(),
            after.analysis.as_ref().unwrap()
        ));
        assert_eq!(fs::read(p.0.join("saved.json")).unwrap(), original);
    }
    assert!(fs::read_dir(&p.0).unwrap().all(|e| {
        !e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".eeg-export-")
    }));
    let after = run(&mut s, request(&p, "retry.json", ExportFormat::Json));
    assert!(after.last_export.is_some());
    let after = run(&mut s, AppCommand::SetView(ViewRequest::default()));
    assert!(after.last_export.is_some());
    let after = run(&mut s, AppCommand::Analyze(analysis()));
    assert!(after.last_export.is_none());
}
#[test]
fn archive_rejects_cross_entities_nonfinite_psd_and_stale_report() {
    let p = TestProject::new();
    let mut s = analyzed(&p);
    let snap = run(&mut s, AppCommand::GenerateReport);
    let mut wrong = snap.clone();
    Arc::make_mut(wrong.analysis.as_mut().unwrap())
        .features
        .recording_id = RecordingId::new();
    assert!(build_export_bundle(&wrong).is_err());
    let mut wrong = snap.clone();
    Arc::make_mut(wrong.analysis.as_mut().unwrap())
        .features
        .spectral
        .as_mut()
        .unwrap()
        .psd_by_channel
        .get_mut("Fp1")
        .unwrap()[0] = f64::NAN;
    assert!(build_export_bundle(&wrong).is_err());
    let mut wrong = snap.clone();
    Arc::make_mut(wrong.analysis.as_mut().unwrap())
        .features
        .spectral
        .as_mut()
        .unwrap()
        .band_power_by_channel
        .get_mut("Fp1")
        .unwrap()
        .get_mut(&FrequencyBand::Alpha)
        .unwrap()
        .absolute += 1.0;
    assert!(build_export_bundle(&wrong).is_err());
    let mut wrong = snap.clone();
    Arc::make_mut(wrong.analysis.as_mut().unwrap())
        .processed_quality
        .overall_score = domain::UnitInterval::ZERO;
    assert!(build_export_bundle(&wrong).is_err());
}
#[test]
fn csv_quotes_unicode_multiline_labels_and_neutralizes_formula_strings() {
    let p = TestProject::new();
    let mut s = analyzed(&p);
    let mut snap = s.snapshot();
    snap.subjects[0]
        .metadata
        .insert("notes".into(), " \t=HYPERLINK(\"bad\")\r\n中文,备注".into());
    let bundle = build_export_bundle(&snap).unwrap();
    let exporter = FileReportExporter::default();
    exporter
        .export(
            &bundle,
            &ExportRequest {
                destination: p.0.join("safe.csv"),
                format: ExportFormat::Csv,
            },
        )
        .unwrap();
    let csv = fs::read_to_string(p.0.join("safe.csv")).unwrap();
    assert!(csv.contains("\"' \t=HYPERLINK(\"\"bad\"\")\r\n中文,备注\""));
    run(&mut s, AppCommand::GenerateReport);
}
struct SlowExporter {
    entered: mpsc::Sender<()>,
    release: std::sync::Mutex<mpsc::Receiver<()>>,
}
impl ReportExporter for SlowExporter {
    fn export(&self, _: &ExportBundle, r: &ExportRequest) -> Result<ExportReceipt, ExportError> {
        self.entered.send(()).unwrap();
        self.release.lock().unwrap().recv().unwrap();
        Ok(ExportReceipt {
            destination: r.destination.clone(),
            format: r.format,
            bytes_written: 42,
        })
    }
}
#[test]
fn slow_export_does_not_block_ui_and_busy_lasts_until_terminal_event() {
    let p = TestProject::new();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let s = analyzed(&p).with_report_exporter(Box::new(SlowExporter {
        entered: entered_tx,
        release: std::sync::Mutex::new(release_rx),
    }));
    let c = AppController::start(s).unwrap();
    let d = c.dispatcher();
    let start = Instant::now();
    d.submit(request(&p, "slow.json", ExportFormat::Json))
        .unwrap();
    assert!(start.elapsed() < Duration::from_millis(200));
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(matches!(
        d.submit(AppCommand::GenerateReport),
        Err(AppError::Busy)
    ));
    release_tx.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(AppEvent::Completed(s)) = c.try_next_event() {
            assert_eq!(s.last_export.unwrap().bytes_written, 42);
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(!d.is_busy());
}

#[test]
fn concurrent_exports_publish_once_and_never_overwrite_or_leave_temporary_files() {
    let p = TestProject::new();
    let s = analyzed(&p);
    let bundle = Arc::new(build_export_bundle(&s.snapshot()).unwrap());
    let destination = p.0.join("concurrent.json");
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let bundle = bundle.clone();
            let path = destination.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                FileReportExporter::default().export(
                    &bundle,
                    &ExportRequest {
                        destination: path,
                        format: ExportFormat::Json,
                    },
                )
            })
        })
        .collect();
    let mut successes = 0;
    for thread in threads {
        match thread.join().unwrap() {
            Ok(_) => successes += 1,
            Err(ExportError::AlreadyExists) => {}
            Err(e) => panic!("{e}"),
        }
    }
    assert_eq!(successes, 1);
    let saved: serde_json::Value =
        serde_json::from_slice(&fs::read(&destination).unwrap()).unwrap();
    assert_eq!(
        saved["subject"]["id"],
        s.snapshot().selected_subject.unwrap().to_string()
    );
    assert!(fs::read_dir(&p.0).unwrap().all(|e| {
        !e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".eeg-export-")
    }));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(destination).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn invalid_font_fails_before_any_target_is_created_and_other_formats_remain_available() {
    let p = TestProject::new();
    let mut s = analyzed(&p);
    let snap = run(&mut s, AppCommand::GenerateReport);
    let bundle = build_export_bundle(&snap).unwrap();
    let font = p.0.join("invalid.ttf");
    fs::write(&font, b"not a font").unwrap();
    let exporter = FileReportExporter {
        fonts: FontOptions {
            pdf_font: Some(font),
        },
    };
    assert!(matches!(
        exporter.export(
            &bundle,
            &ExportRequest {
                destination: p.0.join("invalid.pdf"),
                format: ExportFormat::Pdf
            }
        ),
        Err(ExportError::InvalidFont)
    ));
    assert!(!p.0.join("invalid.pdf").exists());
    exporter
        .export(
            &bundle,
            &ExportRequest {
                destination: p.0.join("font-independent.docx"),
                format: ExportFormat::Docx,
            },
        )
        .unwrap();
}
