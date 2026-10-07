//! Slint 像素后端。只消费应用层准备好的物理量 DTO，不执行 DSP/FFT。
//!
//! 每条波形/PSD 独立绘制到 SharedPixelBuffer<Rgba8Pixel>，然后直接
//! Image::from_rgba8；没有 SVG 编码/解析、临时文件或外部绘图库。
//! 通道名和坐标标签交给 Slint Text，以保留 Unicode、字号和可访问性；
//! 地形图文字也使用原生字体。按实际视区与 DPI 超采样，窗口布局不重算信号。
use crate::{ChannelPlot, render_size::RasterSize};
use eeg_application::{PlotPoint, PsdPlot, PsdScale, TopomapCoverage, TopomapPlot, WaveformPlot};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

const WHITE: Rgba8Pixel = color(255, 255, 255);
const GRID: Rgba8Pixel = color(229, 233, 239);
const INK: Rgba8Pixel = color(98, 107, 120);
const BLUE: Rgba8Pixel = color(37, 99, 235);
const fn color(r: u8, g: u8, b: u8) -> Rgba8Pixel {
    Rgba8Pixel { r, g, b, a: 255 }
}

pub(crate) fn waveform_rows(
    plot: &WaveformPlot,
    amplitude: f64,
    size: RasterSize,
) -> Vec<ChannelPlot> {
    plot.traces
        .iter()
        .map(|trace| {
            let mut canvas = Canvas::with_size(size, WHITE);
            canvas.axes();
            canvas.trace(
                &trace.points,
                (plot.start_seconds, plot.end_seconds),
                (-amplitude, amplitude),
            );
            ChannelPlot {
                label: trace.channel.clone().into(),
                pixels: Image::from_rgba8(canvas.buffer),
                x_start: format!("{} s", number(plot.start_seconds)).into(),
                x_mid: format!(
                    "{} s",
                    number((plot.start_seconds + plot.end_seconds) / 2.0)
                )
                .into(),
                x_end: format!("{} s", number(plot.end_seconds)).into(),
                y_upper: format!("{} uV", number(amplitude)).into(),
                y_lower: format!("{} uV", number(-amplitude)).into(),
            }
        })
        .collect()
}

pub(crate) fn psd_rows(plot: &PsdPlot, size: RasterSize) -> Vec<ChannelPlot> {
    plot.traces
        .iter()
        .map(|trace| {
            let mut canvas = Canvas::with_size(size, WHITE);
            let x = extent(trace.points.iter().map(|p| p.x));
            let y = extent(trace.points.iter().map(|p| p.y));
            let unit = match plot.scale {
                PsdScale::Linear => "uV²/Hz",
                PsdScale::Decibel { .. } => "dB",
            };
            canvas.axes();
            canvas.trace(&trace.points, x, y);
            ChannelPlot {
                label: trace.channel.clone().into(),
                pixels: Image::from_rgba8(canvas.buffer),
                x_start: format!("{} Hz", number(x.0)).into(),
                x_mid: format!("{} Hz", number((x.0 + x.1) / 2.0)).into(),
                x_end: format!("{} Hz", number(x.1)).into(),
                y_upper: format!("{} {unit}", number(y.1)).into(),
                y_lower: format!("{} {unit}", number(y.0)).into(),
            }
        })
        .collect()
}

/// 常量/单点 PSD 也要有有限且非零的坐标跨度，不能除零。
fn extent(values: impl Iterator<Item = f64>) -> (f64, f64) {
    let (min, max) = values.fold((f64::INFINITY, f64::NEG_INFINITY), |(min, max), v| {
        (min.min(v), max.max(v))
    });
    if !min.is_finite() || !max.is_finite() {
        return (0.0, 1.0);
    }
    if min == max {
        let pad = (min.abs() * 0.05).max(1.0);
        (min - pad, max + pad)
    } else {
        (min, max)
    }
}

/// 复用 P7 网格与原始电极色标。双线性采样只改善显示平滑度；
/// 覆盖策略由 DTO 指定：凸包模式保留掩膜，头圆模式填满圆内并在圆外透明。
pub(crate) fn topomap_buffer(
    plot: &TopomapPlot,
    size: RasterSize,
) -> SharedPixelBuffer<Rgba8Pixel> {
    let transparent = Rgba8Pixel {
        r: 255,
        g: 255,
        b: 255,
        a: 0,
    };
    let mut canvas = Canvas::with_size(size, transparent);
    let scale = f64::from(size.width) / 640.0;
    let at = |x: f64, y: f64| (x * scale, y * scale);
    let full = canvas.area();
    let pixels = canvas.buffer.make_mut_slice();
    for y in 0..size.height as usize {
        for x in 0..size.width as usize {
            let gx = ((x as f64 + 0.5) / scale - 64.0) / 420.0;
            let gy = ((y as f64 + 0.5) / scale - 30.0) / 420.0;
            if !(0.0..1.0).contains(&gx) || !(0.0..1.0).contains(&gy) {
                continue;
            }
            if let Some(value) = grid_value(plot, gx, gy) {
                pixels[y * size.width as usize + x] = heat(value, plot.color_min, plot.color_max);
            }
        }
    }
    // 头圆、朝上鼻尖与电极位置按同一几何比例映射，线宽按逻辑像素保持一致。
    let steps = (std::f64::consts::TAU * 210.0 * scale).ceil().max(360.0) as usize;
    for i in 0..steps {
        let a = i as f64 / steps as f64 * std::f64::consts::TAU;
        let b = (i + 1) as f64 / steps as f64 * std::f64::consts::TAU;
        canvas.line(
            at(274.0 + 210.0 * a.cos(), 240.0 - 210.0 * a.sin()),
            at(274.0 + 210.0 * b.cos(), 240.0 - 210.0 * b.sin()),
            INK,
            full,
        );
    }
    canvas.line(at(259.0, 31.0), at(274.0, 10.0), INK, full);
    canvas.line(at(274.0, 10.0), at(289.0, 31.0), INK, full);
    for electrode in &plot.electrodes {
        let (x, y) = at(274.0 + electrode.x * 210.0, 240.0 - electrode.y * 210.0);
        canvas.dot(x, y, INK, full);
    }
    for y in 0..size.height as i32 {
        let base_y = f64::from(y) / scale;
        if !(50.0..410.0).contains(&base_y) {
            continue;
        }
        let pixel = heat(1.0 - (base_y - 50.0) / 360.0, 0.0, 1.0);
        for x in (535.0 * scale).ceil() as i32..(553.0 * scale).ceil() as i32 {
            canvas.set(x, y, pixel);
        }
    }
    canvas.buffer
}

fn grid_value(plot: &TopomapPlot, x: f64, y: f64) -> Option<f64> {
    if plot.coverage == TopomapCoverage::HeadCircle {
        let nx = x * 2.0 - 1.0;
        let ny = y * 2.0 - 1.0;
        let radius = nx.hypot(ny);
        // 按连续圆形裁剪，避免把边缘网格色块带到圆外。
        if radius > 1.0 {
            return None;
        }
        if let Some(value) = bilinear_value(plot, x, y) {
            return Some(value);
        }
        // 极粗网格在圆边缘可能没有带权重的有效邻格。沿半径向内移动
        // 一个网格对角线的距离，再采样圆内已有值，保证圆内不留透明缝。
        // 此回退只使用展示网格，不重跑 IDW、FFT 或任何分析计算。
        let inner_radius = (1.0 - std::f64::consts::SQRT_2 / plot.resolution as f64).max(0.0);
        let factor = if radius > inner_radius {
            inner_radius / radius
        } else {
            1.0
        };
        return bilinear_value(plot, (nx * factor + 1.0) * 0.5, (ny * factor + 1.0) * 0.5);
    }
    bilinear_value(plot, x, y)
}

fn bilinear_value(plot: &TopomapPlot, x: f64, y: f64) -> Option<f64> {
    let n = plot.resolution;
    let gx = (x * n as f64 - 0.5).clamp(0.0, (n - 1) as f64);
    let gy = (y * n as f64 - 0.5).clamp(0.0, (n - 1) as f64);
    let x0 = gx.floor() as usize;
    let y0 = gy.floor() as usize;
    let x1 = (x0 + 1).min(n - 1);
    let y1 = (y0 + 1).min(n - 1);
    let tx = gx - x0 as f64;
    let ty = gy - y0 as f64;
    // 先按最大绝对值归一化再加权，避免合法大功率的中间求和溢出。
    let magnitude = plot.color_min.abs().max(plot.color_max.abs());
    let mut sum = 0.0;
    let mut weight_sum = 0.0;
    for (row, col, weight) in [
        (y0, x0, (1.0 - tx) * (1.0 - ty)),
        (y0, x1, tx * (1.0 - ty)),
        (y1, x0, (1.0 - tx) * ty),
        (y1, x1, tx * ty),
    ] {
        if weight == 0.0 {
            continue;
        }
        let Some(value) = plot.values[row * n + col] else {
            if plot.coverage == TopomapCoverage::ElectrodeHull {
                return None;
            }
            // 头圆模式的 None 只在圆外；重新归一化圆内邻格权重，防止
            // 边缘因一个圆外邻格而整像素透明，也不把透明值当成零功率。
            continue;
        };
        weight_sum += weight;
        if magnitude > 0.0 {
            sum += weight * (value / magnitude);
        }
    }
    (weight_sum > 0.0).then(|| {
        ((sum / weight_sum).clamp(-1.0, 1.0) * magnitude).clamp(plot.color_min, plot.color_max)
    })
}

fn heat(value: f64, min: f64, max: f64) -> Rgba8Pixel {
    let t = if min == max {
        0.5
    } else {
        ((value - min) / (max - min)).clamp(0.0, 1.0)
    };
    // 冷蓝 → 浅色 → 暖红；所有图沿用 P7 的原始电极固定色标。
    let (a, b, t) = if t < 0.5 {
        ([37.0, 99.0, 235.0], [245.0, 247.0, 250.0], t * 2.0)
    } else {
        ([245.0, 247.0, 250.0], [220.0, 65.0, 60.0], t * 2.0 - 1.0)
    };
    color(
        (a[0] + (b[0] - a[0]) * t) as u8,
        (a[1] + (b[1] - a[1]) * t) as u8,
        (a[2] + (b[2] - a[2]) * t) as u8,
    )
}

#[derive(Clone, Copy)]
struct Rect {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}
struct Canvas {
    buffer: SharedPixelBuffer<Rgba8Pixel>,
    density: f64,
}
impl Canvas {
    fn new(width: u32, height: u32, background: Rgba8Pixel) -> Self {
        let mut buffer = SharedPixelBuffer::new(width, height);
        buffer.make_mut_slice().fill(background);
        Self {
            buffer,
            density: 1.0,
        }
    }
    fn with_size(size: RasterSize, background: Rgba8Pixel) -> Self {
        let mut canvas = Self::new(size.width, size.height, background);
        canvas.density = size.density;
        canvas
    }
    fn area(&self) -> Rect {
        Rect {
            left: 0.0,
            top: 0.0,
            right: f64::from(self.buffer.width() - 1),
            bottom: f64::from(self.buffer.height() - 1),
        }
    }
    fn set(&mut self, x: i32, y: i32, pixel: Rgba8Pixel) {
        if x >= 0 && y >= 0 && x < self.buffer.width() as i32 && y < self.buffer.height() as i32 {
            let offset = y as usize * self.buffer.width() as usize + x as usize;
            self.buffer.make_mut_slice()[offset] = pixel;
        }
    }
    fn blend(&mut self, x: i32, y: i32, pixel: Rgba8Pixel, coverage: f64, clip: Rect) {
        if (x as f64) < clip.left
            || (x as f64) > clip.right
            || (y as f64) < clip.top
            || (y as f64) > clip.bottom
        {
            return;
        }
        let offset = y as usize * self.buffer.width() as usize + x as usize;
        let dst = &mut self.buffer.make_mut_slice()[offset];
        let alpha = coverage.clamp(0.0, 1.0);
        let mix =
            |a: u8, b: u8| (f64::from(a) * (1.0 - alpha) + f64::from(b) * alpha).round() as u8;
        dst.r = mix(dst.r, pixel.r);
        dst.g = mix(dst.g, pixel.g);
        dst.b = mix(dst.b, pixel.b);
        dst.a = dst.a.max((alpha * 255.0) as u8);
    }
    /// 先裁剪，再按主轴逐像素扫描线段的有限宽度。覆盖率来自像素中心
    /// 到线段的距离，线宽随 DPI/超采样换算，缩小显示后仍是约 1.2 逻辑像素。
    fn line(&mut self, a: (f64, f64), b: (f64, f64), pixel: Rgba8Pixel, rect: Rect) {
        self.stroke(a, b, pixel, rect, 1.2 * self.density);
    }
    fn stroke(&mut self, a: (f64, f64), b: (f64, f64), pixel: Rgba8Pixel, rect: Rect, width: f64) {
        let Some((a, b)) = clip_line(a, b, rect) else {
            return;
        };
        let dx = b.0 - a.0;
        let dy = b.1 - a.1;
        let length2 = dx * dx + dy * dy;
        let horizontal = dx.abs() >= dy.abs();
        let (major_a, major_b, minor_a, minor_b) = if horizontal {
            (a.0, b.0, a.1, b.1)
        } else {
            (a.1, b.1, a.0, b.0)
        };
        let radius = width / 2.0;
        let reach = (radius + 0.5) * std::f64::consts::SQRT_2;
        for m in (major_a.min(major_b) - radius - 0.5).floor() as i32
            ..=(major_a.max(major_b) + radius + 0.5).ceil() as i32
        {
            let t = if major_a == major_b {
                0.0
            } else {
                ((f64::from(m) - major_a) / (major_b - major_a)).clamp(0.0, 1.0)
            };
            let center = minor_a + t * (minor_b - minor_a);
            for n in (center - reach).floor() as i32..=(center + reach).ceil() as i32 {
                let (x, y) = if horizontal { (m, n) } else { (n, m) };
                let px = f64::from(x);
                let py = f64::from(y);
                let t = if length2 == 0.0 {
                    0.0
                } else {
                    (((px - a.0) * dx + (py - a.1) * dy) / length2).clamp(0.0, 1.0)
                };
                let distance = (px - a.0 - t * dx).hypot(py - a.1 - t * dy);
                let coverage = (radius + 0.5 - distance).clamp(0.0, 1.0);
                if coverage > 0.0 {
                    self.blend(x, y, pixel, coverage, rect);
                }
            }
        }
    }
    fn dot(&mut self, x: f64, y: f64, pixel: Rgba8Pixel, rect: Rect) {
        if !x.is_finite()
            || !y.is_finite()
            || x < rect.left
            || x > rect.right
            || y < rect.top
            || y > rect.bottom
        {
            return;
        }
        self.stroke((x, y), (x, y), pixel, rect, 3.0 * self.density);
    }
    fn axes(&mut self) {
        let area = self.area();
        for tick in 0..=4 {
            let t = tick as f64 / 4.0;
            let px = area.left + (area.right - area.left) * t;
            let py = area.top + (area.bottom - area.top) * t;
            self.stroke(
                (px, area.top),
                (px, area.bottom),
                GRID,
                area,
                0.6 * self.density,
            );
            self.stroke(
                (area.left, py),
                (area.right, py),
                GRID,
                area,
                0.6 * self.density,
            );
        }
    }
    fn trace(&mut self, points: &[PlotPoint], x: (f64, f64), y: (f64, f64)) {
        let area = self.area();
        let position = |p: &PlotPoint| {
            (
                area.left + ((p.x - x.0) / (x.1 - x.0)).clamp(-1e6, 1e6) * (area.right - area.left),
                area.bottom
                    - ((p.y - y.0) / (y.1 - y.0)).clamp(-1e6, 1e6) * (area.bottom - area.top),
            )
        };
        if points.len() == 1 {
            let p = position(&points[0]);
            self.dot(p.0, p.1, BLUE, area);
        }
        for pair in points.windows(2) {
            self.line(position(&pair[0]), position(&pair[1]), BLUE, area);
        }
    }
}

/// Liang–Barsky 裁剪。拒绝非有限坐标；输出限定在已分配缓冲内。
fn clip_line(a: (f64, f64), b: (f64, f64), rect: Rect) -> Option<((f64, f64), (f64, f64))> {
    if ![a.0, a.1, b.0, b.1].iter().all(|v| v.is_finite()) {
        return None;
    }
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let (mut low, mut high) = (0.0_f64, 1.0_f64);
    for (p, q) in [
        (-dx, a.0 - rect.left),
        (dx, rect.right - a.0),
        (-dy, a.1 - rect.top),
        (dy, rect.bottom - a.1),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                low = low.max(t);
            } else {
                high = high.min(t);
            }
            if low > high {
                return None;
            }
        }
    }
    Some((
        (a.0 + low * dx, a.1 + low * dy),
        (a.0 + high * dx, a.1 + high * dy),
    ))
}
fn number(v: f64) -> String {
    if v != 0.0 && !(0.01..1e5).contains(&v.abs()) {
        format!("{v:.1E}")
    } else {
        format!("{v:.2}")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use eeg_application::{BandPowerMeasure, FrequencyBand, RecordingId};
    const WIDTH: u32 = 960;
    const HEIGHT: u32 = 80;
    const AREA: Rect = Rect {
        left: 0.0,
        top: 0.0,
        right: (WIDTH - 1) as f64,
        bottom: (HEIGHT - 1) as f64,
    };

    fn blue(pixel: &Rgba8Pixel) -> bool {
        pixel.b > pixel.r.saturating_add(30) && pixel.b > pixel.g.saturating_add(30)
    }

    #[test]
    fn waveform_clips_at_physical_range_and_preserves_input() {
        let points = vec![
            PlotPoint { x: 0.0, y: 1000.0 },
            PlotPoint { x: 1.0, y: -1000.0 },
        ];
        let original = points.clone();
        let mut canvas = Canvas::new(WIDTH, HEIGHT, WHITE);
        canvas.trace(&points, (0.0, 1.0), (-1.0, 1.0));
        assert_eq!(points, original);
        assert!(canvas.buffer.as_slice().iter().any(blue));
        // 两个端点都超量程，真实连接只在中间穿过可视区。不能把端点
        // clamp 到上下边界后伪造一条从左上到右下的长斜线。
        for (i, pixel) in canvas.buffer.as_slice().iter().enumerate() {
            if blue(pixel) {
                let x = (i % WIDTH as usize) as f64;
                let y = (i / WIDTH as usize) as f64;
                assert!((AREA.left..=AREA.right).contains(&x));
                assert!((AREA.top..=AREA.bottom).contains(&y));
                assert!((x - (AREA.left + AREA.right) / 2.0).abs() < 3.0);
            }
        }
        let mut outside = Canvas::new(WIDTH, HEIGHT, WHITE);
        outside.trace(
            &[
                PlotPoint { x: 0.0, y: 1000.0 },
                PlotPoint { x: 1.0, y: 1000.0 },
            ],
            (0.0, 1.0),
            (-1.0, 1.0),
        );
        assert!(!outside.buffer.as_slice().iter().any(blue));
    }

    #[test]
    fn single_sample_and_amplitude_scaling_are_visible_in_pixel_buffer() {
        let points = vec![PlotPoint { x: 0.5, y: 25.0 }];
        let mut a = Canvas::new(WIDTH, HEIGHT, WHITE);
        let mut b = Canvas::new(WIDTH, HEIGHT, WHITE);
        a.trace(&points, (0.0, 1.0), (-50.0, 50.0));
        b.trace(&points, (0.0, 1.0), (-100.0, 100.0));
        let center_y = |buffer: &SharedPixelBuffer<Rgba8Pixel>| {
            let rows: Vec<_> = buffer
                .as_slice()
                .iter()
                .enumerate()
                .filter(|(_, p)| blue(p))
                .map(|(i, _)| (i / WIDTH as usize) as f64)
                .collect();
            assert!(!rows.is_empty());
            rows.iter().sum::<f64>() / rows.len() as f64
        };
        assert!((center_y(&a.buffer) - AREA.bottom / 4.0).abs() < 1.0);
        assert!((center_y(&b.buffer) - AREA.bottom * 0.375).abs() < 1.0);
        assert_ne!(a.buffer.as_bytes(), b.buffer.as_bytes());
    }

    #[test]
    fn constant_psd_has_finite_range_and_nan_segments_are_rejected() {
        let range = extent([0.0, 0.0].into_iter());
        assert_eq!(range, (-1.0, 1.0));
        let mut canvas = Canvas::new(WIDTH, HEIGHT, WHITE);
        canvas.trace(
            &[PlotPoint { x: 0.0, y: 0.0 }, PlotPoint { x: 1.0, y: 0.0 }],
            (0.0, 1.0),
            range,
        );
        assert!(canvas.buffer.as_slice().iter().any(blue));
        assert!(clip_line((f64::NAN, 0.0), (1.0, 1.0), AREA).is_none());
        assert!(clip_line((0.0, f64::INFINITY), (1.0, 1.0), AREA).is_none());
    }

    #[test]
    fn topomap_keeps_mask_transparent_and_uses_fixed_color_scale() {
        let plot = TopomapPlot {
            recording_id: RecordingId::new(),
            band: FrequencyBand::Alpha,
            measure: BandPowerMeasure::Relative,
            layout_name: "test".into(),
            schematic_layout: true,
            electrodes: vec![],
            skipped_channels: vec![],
            resolution: 2,
            coverage: TopomapCoverage::ElectrodeHull,
            values: vec![None, Some(0.25), Some(0.75), None],
            color_min: 0.25,
            color_max: 0.75,
        };
        let buffer = topomap_buffer(
            &plot,
            RasterSize {
                width: 640,
                height: 480,
                density: 1.0,
            },
        );
        assert_eq!(buffer.size().width, 640);
        assert_eq!(buffer.as_slice()[150 * 640 + 240].a, 0);
        assert_eq!(buffer.as_slice()[100 * 640 + 440], heat(0.25, 0.25, 0.75));
        assert_eq!(buffer.as_slice()[400 * 640 + 100], heat(0.75, 0.25, 0.75));
        assert_eq!(heat(2.0, 2.0, 2.0), heat(0.5, 0.0, 1.0));
        assert_eq!(plot.values, vec![None, Some(0.25), Some(0.75), None]);
    }

    #[test]
    fn antialiased_stroke_has_same_logical_width_at_two_dpi_levels() {
        let points = [PlotPoint { x: 0.5, y: -1.0 }, PlotPoint { x: 0.5, y: 1.0 }];
        for dpi in [1.0, 1.25, 2.0] {
            let size = RasterSize::for_view(700.0, 25.0, dpi).unwrap();
            let mut canvas = Canvas::with_size(size, WHITE);
            canvas.trace(&points, (0.0, 1.0), (-1.0, 1.0));
            assert_eq!(
                (canvas.buffer.width(), canvas.buffer.height()),
                (size.width, size.height)
            );
            let row = (size.height as usize / 2) * size.width as usize;
            let coverage: f64 = canvas.buffer.as_slice()[row..row + size.width as usize]
                .iter()
                .map(|p| f64::from(255 - p.r) / f64::from(255 - BLUE.r))
                .sum();
            assert!(
                (coverage / size.density - 1.2).abs() < 0.05,
                "DPI {dpi}: line coverage {coverage}, density {}",
                size.density
            );
        }
    }

    #[test]
    fn bilinear_sampling_preserves_mask_and_interpolates_only_existing_values() {
        let mut plot = TopomapPlot {
            recording_id: RecordingId::new(),
            band: FrequencyBand::Alpha,
            measure: BandPowerMeasure::Absolute,
            layout_name: "test".into(),
            schematic_layout: true,
            electrodes: vec![],
            skipped_channels: vec![],
            resolution: 2,
            coverage: TopomapCoverage::ElectrodeHull,
            values: vec![Some(0.0), Some(2.0), Some(4.0), Some(6.0)],
            color_min: 0.0,
            color_max: 6.0,
        };
        assert_eq!(grid_value(&plot, 0.5, 0.5), Some(3.0));
        assert_eq!(grid_value(&plot, 0.375, 0.375), Some(1.5));
        assert_eq!(grid_value(&plot, 0.25, 0.25), Some(0.0));
        plot.values[0] = None;
        assert_eq!(grid_value(&plot, 0.5, 0.5), None); // 绝不跨掩码边界混色。
        assert_eq!(grid_value(&plot, 0.75, 0.25), Some(2.0)); // 零权重 None 不影响有效格点。
        plot.values.fill(Some(f64::MAX));
        plot.color_min = f64::MAX;
        plot.color_max = f64::MAX;
        assert_eq!(grid_value(&plot, 0.5, 0.5), Some(f64::MAX));
    }

    #[test]
    fn head_circle_fills_every_inner_pixel_including_mask_boundary_at_multiple_sizes() {
        // 圆外格点保留 None，圆内使用常量值；如果透明邻格被当成零功率，
        // 或边缘双线性权重没有归一化，此测试会出现透明缝或错误颜色。
        for resolution in [2, 3, 21, 256] {
            let mut plot = TopomapPlot {
                recording_id: RecordingId::new(),
                band: FrequencyBand::Alpha,
                measure: BandPowerMeasure::Relative,
                layout_name: "circle".into(),
                schematic_layout: true,
                electrodes: vec![],
                skipped_channels: vec![],
                resolution,
                coverage: TopomapCoverage::HeadCircle,
                values: vec![],
                color_min: 0.4,
                color_max: 0.4,
            };
            for row in 0..resolution {
                for col in 0..resolution {
                    let x = (col as f64 + 0.5) * 2.0 / resolution as f64 - 1.0;
                    let y = (row as f64 + 0.5) * 2.0 / resolution as f64 - 1.0;
                    plot.values.push((x.hypot(y) <= 1.0).then_some(0.4));
                }
            }
            let original = plot.clone();
            for density in [1.0, 2.0] {
                let width = (640.0 * density) as u32;
                let height = (480.0 * density) as u32;
                let buffer = topomap_buffer(
                    &plot,
                    RasterSize {
                        width,
                        height,
                        density,
                    },
                );
                for row in 0..height {
                    for col in 0..width {
                        let x = ((f64::from(col) + 0.5) / density - 274.0) / 210.0;
                        let y = ((f64::from(row) + 0.5) / density - 240.0) / 210.0;
                        if x.hypot(y) <= 1.0 {
                            assert_eq!(
                                grid_value(&plot, (x + 1.0) * 0.5, (y + 1.0) * 0.5),
                                Some(0.4)
                            );
                            assert_eq!(
                                buffer.as_slice()[(row * width + col) as usize].a,
                                255,
                                "empty circle pixel: grid {resolution}, density {density}, ({col},{row})"
                            );
                        }
                    }
                }
                // 远离头圆线条/鼻尖/色标的圆外位置仍透明。
                assert_eq!(
                    buffer.as_slice()
                        [(40.0 * density) as usize * width as usize + (75.0 * density) as usize]
                        .a,
                    0
                );
            }
            assert_eq!(plot, original, "像素显示不得修改网格或原始色标");
            assert_eq!(grid_value(&plot, 0.0, 0.0), None);
        }
    }
}
