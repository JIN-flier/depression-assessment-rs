//! 视区回报与绘图缓存。布局变化只重绘像素，不提交业务命令，不重跑 DSP/FFT。
//! Slint 的几何回调仅记录最新值；30ms UI timer 合并同一帧的宽/高变化，
//! 避免在布局求值中修改图像而产生递归布局。滚轮不会改变像素尺寸。
use crate::{AppWindow, presentation, render_size::RenderTargets};
use eeg_application::{AppSnapshot, PlotData};
use slint::ComponentHandle;
use std::sync::Arc;

#[derive(Default)]
pub(crate) struct PlotRenderer {
    geometry: [(f32, f32); 3],
    last_targets: Option<RenderTargets>,
    last_plots: Option<Arc<PlotData>>,
    last_amplitude: Option<f64>,
}
impl PlotRenderer {
    pub fn report_geometry(&mut self, kind: i32, width: f32, height: f32) {
        if width.is_finite()
            && height.is_finite()
            && width > 0.0
            && height > 0.0
            && let Some(slot) = usize::try_from(kind)
                .ok()
                .and_then(|index| self.geometry.get_mut(index))
        {
            *slot = (width, height);
        }
    }

    pub fn refresh(&mut self, window: &AppWindow, state: &AppSnapshot) {
        let count = state.plots.waveform.as_ref().map_or(0, |p| p.traces.len())
            + state.plots.psd.as_ref().map_or(0, |p| p.traces.len());
        let targets = RenderTargets::new(self.geometry, window.window().scale_factor(), count);
        // 幅度属于视图而非物理量 DTO；复用同一 DTO 时也不能漏掉量程更新。
        if self.last_amplitude == Some(state.view.amplitude_uv)
            && self.last_targets == Some(targets)
            && self
                .last_plots
                .as_ref()
                .is_some_and(|last| Arc::ptr_eq(last, &state.plots))
        {
            return;
        }
        presentation::render_plots(window, state, targets);
        self.last_targets = Some(targets);
        self.last_amplitude = Some(state.view.amplitude_uv);
        self.last_plots = Some(state.plots.clone());
    }
}
