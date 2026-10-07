#![allow(dead_code)]
use eeg_application::*;
use std::{f64::consts::TAU, fmt::Write, fs, path::PathBuf};

// 不引入 tempfile：UUID 独立目录隔离并行用例，Drop 在数据库释放后清理。
pub struct TestProject(pub PathBuf);
impl TestProject {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!("p8-test-{}", RecordingId::new()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    pub fn csv(&self, labels: &[&str], count: usize, rate: f64) -> PathBuf {
        let path = self.0.join(format!("{}.csv", RecordingId::new()));
        let mut text = format!("{}\n", labels.join(","));
        for i in 0..count {
            for j in 0..labels.len() {
                if j > 0 {
                    text.push(',');
                }
                let value = 5.0 + (20.0 + j as f64) * (TAU * 10.0 * i as f64 / rate).sin();
                let _ = write!(text, "{value:.7}");
            }
            text.push('\n');
        }
        fs::write(&path, text).unwrap();
        path
    }
}
impl Drop for TestProject {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
pub fn run(service: &mut ApplicationService, command: AppCommand) -> AppSnapshot {
    service.execute(command, &mut |_| {}).unwrap()
}
pub fn setup(project: &TestProject) -> ApplicationService {
    let mut service = ApplicationService::default();
    run(&mut service, AppCommand::OpenProject(project.0.clone()));
    run(
        &mut service,
        AppCommand::CreateSubject(SubjectDraft {
            age: Some(32),
            sex: Sex::Female,
            notes: "合成测试".into(),
        }),
    );
    service
}
pub fn imported(project: &TestProject, labels: &[&str], count: usize) -> ApplicationService {
    let mut service = setup(project);
    import_csv(&mut service, project.csv(labels, count, 256.0), 256.0);
    service
}
pub fn import_csv(service: &mut ApplicationService, path: PathBuf, rate: f64) -> AppSnapshot {
    let format = ImportForm {
        format_index: 1,
        sampling_rate: rate.to_string(),
        unit: "uV".into(),
        csv_headers: true,
        labels: String::new(),
        mat_variable: "data".into(),
        mat_rate_variable: String::new(),
        mat_samples_by_channels: false,
    }
    .parse()
    .unwrap();
    run(
        service,
        AppCommand::ImportEeg {
            path,
            format,
            metadata: RecordingMetadata::default(),
        },
    )
}
pub fn analysis() -> AnalysisRequest {
    ProcessingForm {
        dc: true,
        bandpass: true,
        low_hz: "0.5".into(),
        high_hz: "30".into(),
        notch: true,
        notch_hz: "50".into(),
        resample: true,
        target_hz: "128".into(),
        exclude_bad_channels: false,
    }
    .parse()
    .unwrap()
}
