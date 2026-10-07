//! 逻辑视区 → 物理像素预算。与 Slint/EEG 无关，便于独立验证 DPI 与资源上限。
//! 2× 超采样减少斜线和头圆锯齿；线宽也按同一密度换算，避免高 DPI 时变细。

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RasterSize {
    pub width: u32,
    pub height: u32,
    /// 每个逻辑像素使用的缓冲像素数量（包含超采样及预算降级）。
    pub density: f64,
}
impl RasterSize {
    pub fn for_view(width: f32, height: f32, dpi: f32) -> Option<Self> {
        if ![width, height, dpi]
            .iter()
            .all(|v| v.is_finite() && *v > 0.0)
        {
            return None;
        }
        let mut density = f64::from(dpi) * 2.0;
        // 保持两轴相同比例，防止超宽/异常缩放值导致巨额分配。
        density = density
            .min(8192.0 / f64::from(width))
            .min(4096.0 / f64::from(height));
        let size = Self {
            width: (f64::from(width) * density).ceil().max(1.0) as u32,
            height: (f64::from(height) * density).ceil().max(1.0) as u32,
            density,
        };
        Some(size.limit_pixels(8 * 1024 * 1024))
    }
    pub fn limit_pixels(self, budget: usize) -> Self {
        let count = self.width as usize * self.height as usize;
        if count <= budget {
            return self;
        }
        let ratio = (budget as f64 / count as f64).sqrt();
        Self {
            width: (f64::from(self.width) * ratio).floor().max(1.0) as u32,
            height: (f64::from(self.height) * ratio).floor().max(1.0) as u32,
            density: self.density * ratio,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RenderTargets {
    pub waveform: RasterSize,
    pub psd: RasterSize,
    pub topomap: RasterSize,
}
impl RenderTargets {
    pub fn new(geometry: [(f32, f32); 3], dpi: f32, trace_count: usize) -> Self {
        let defaults = [(960.0, 80.0), (960.0, 80.0), (640.0, 480.0)];
        // 固定三个图表，缓存检查无需在每个 timer tick 分配临时 Vec。
        let sizes: [RasterSize; 3] = std::array::from_fn(|index| {
            let (w, h) = geometry[index];
            let (dw, dh) = defaults[index];
            RasterSize::for_view(w, h, dpi)
                .unwrap_or_else(|| RasterSize::for_view(dw, dh, 1.0).expect("finite fallback"))
        });
        // 波形 + PSD 的全部通道缓冲合计最多约 128 MiB；常见导联数保持 2× 超采样。
        let per_trace = (32 * 1024 * 1024 / trace_count.max(1)).min(1024 * 1024);
        Self {
            waveform: sizes[0].limit_pixels(per_trace),
            psd: sizes[1].limit_pixels(per_trace),
            topomap: sizes[2],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn physical_resolution_follows_view_size_and_fractional_dpi() {
        let a = RasterSize::for_view(700.0, 25.0, 1.0).unwrap();
        assert_eq!((a.width, a.height), (1400, 50));
        let b = RasterSize::for_view(700.0, 25.0, 2.0).unwrap();
        assert_eq!((b.width, b.height), (2800, 100));
        assert_eq!(b.density, a.density * 2.0);
        let c = RasterSize::for_view(900.5, 32.5, 1.25).unwrap();
        assert_eq!((c.width, c.height), (2252, 82));
    }
    #[test]
    fn invalid_geometry_is_rejected_and_large_views_are_bounded() {
        for (w, h, dpi) in [
            (0.0, 20.0, 1.0),
            (-1.0, 20.0, 1.0),
            (f32::NAN, 20.0, 1.0),
            (50.0, f32::INFINITY, 2.0),
            (50.0, 20.0, 0.0),
        ] {
            assert!(RasterSize::for_view(w, h, dpi).is_none());
        }
        let size = RasterSize::for_view(50_000.0, 50_000.0, 8.0).unwrap();
        assert!(size.width <= 8192 && size.height <= 4096);
        assert!(size.width as usize * size.height as usize <= 8 * 1024 * 1024);
        let targets = RenderTargets::new([(4000.0, 500.0); 3], 4.0, 512);
        assert!(
            (targets.waveform.width as usize * targets.waveform.height as usize) * 512
                <= 32 * 1024 * 1024
        );
    }
}
