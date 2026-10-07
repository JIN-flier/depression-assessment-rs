//! P7：无 UI 的 EEG 绘图数据准备与可替换的渲染后端。
//!
//! 只读取 P1 领域输入；不执行滤波、FFT、质量分析或诊断。波形单位归一化、
//! 显示降采样、PSD 显示变换与地形图插值仅服务于绘图，不能回写分析结果。
//! 同步服务应由 P8 应用层放到后台线程，Slint 仅消费结果或 SVG 图像。
//! SVG 后端使用标准库生成自包含矢量图，不引入新的第三方 Cargo 包。
//!
//! ```no_run
//! use domain::EegRecording;
//! use eeg_visualization::*;
//!
//! fn waveform_svg(recording: &EegRecording) -> VisualizationResult<String> {
//!     let service: Box<dyn EegVisualizer> = Box::new(VisualizationService::default());
//!     let renderer: Box<dyn PlotRenderer> = Box::new(SvgRenderer::default());
//!     let request = WaveformRequest {
//!         channels: vec!["Fp1".into(), "Fp2".into()],
//!         ..Default::default()
//!     };
//!     let plot = service.waveform(recording, &request)?;
//!     renderer.render(&EegPlot::Waveform(plot), SvgOptions::default())
//! }
//! ```
#![forbid(unsafe_code)]

mod config;
mod data;
mod error;
mod layout;
mod service;
mod svg;
mod topomap;
mod validation;

pub use config::*;
pub use data::*;
pub use error::{VisualizationError, VisualizationResult};
pub use layout::{ElectrodeLayout, ElectrodePosition};
pub use service::{EegVisualizer, VisualizationService};
pub use svg::{PlotRenderer, SvgRenderer};
