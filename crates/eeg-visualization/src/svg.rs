//! SVG 后端仅把准备好的 DTO 投影到屏幕，不接触原始 EEG 或特征提取器。
//! 输出无脚本、网络、字体或外部图像依赖，所有输入文本均经过 XML 转义。
use crate::{
    error::{configuration, invalid},
    validation as v, *,
};
use std::collections::BTreeSet;
use std::fmt::Write;

/// 可由 Plotters、Slint 像素缓冲或其他后端替换；返回内容的保存由应用层负责。
pub trait PlotRenderer: Send + Sync {
    fn render(&self, plot: &EegPlot, options: SvgOptions) -> VisualizationResult<String>;
}

#[derive(Debug, Clone, Default)]
pub struct SvgRenderer {
    limits: VisualizationLimits,
}

impl SvgRenderer {
    pub fn new(limits: VisualizationLimits) -> VisualizationResult<Self> {
        v::limits(limits)?;
        Ok(Self { limits })
    }
}

impl PlotRenderer for SvgRenderer {
    fn render(&self, plot: &EegPlot, options: SvgOptions) -> VisualizationResult<String> {
        if !(320..=8192).contains(&options.width) || !(240..=8192).contains(&options.height) {
            return Err(configuration(
                "SVG size must be width 320..8192, height 240..8192",
            ));
        }
        // DTO 允许回放/反序列化：即使绕过 service，也在分配大 SVG 前再次验证。
        let (title, id) = match plot {
            EegPlot::Waveform(p) => ("EEG waveform", p.recording_id),
            EegPlot::Psd(p) => ("EEG power spectral density", p.recording_id),
            EegPlot::Topomap(p) => ("EEG band-power topomap", p.recording_id),
        };
        let content = match plot {
            EegPlot::Waveform(p) => waveform(p, options, self.limits)?,
            EegPlot::Psd(p) => psd(p, options, self.limits)?,
            EegPlot::Topomap(p) => topomap(p, options, self.limits)?,
        };
        let mut svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\" role=\"img\"><title>{title}</title><rect width=\"100%\" height=\"100%\" fill=\"white\"/>",
            options.width, options.height, options.width, options.height
        );
        text_at(&mut svg, 20.0, 25.0, title, 18);
        text_at(&mut svg, 20.0, 45.0, &format!("Recording: {id}"), 11);
        svg.push_str(&content);
        svg.push_str("</svg>");
        Ok(svg)
    }
}

fn escape(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .fold(String::new(), |mut s, c| {
            s.push_str(match c {
                '&' => "&amp;",
                '<' => "&lt;",
                '>' => "&gt;",
                '"' => "&quot;",
                '\'' => "&apos;",
                _ => {
                    s.push(c);
                    return s;
                }
            });
            s
        })
}

fn text_at(svg: &mut String, x: f64, y: f64, text: &str, size: u32) {
    let _ = write!(
        svg,
        "<text x=\"{x:.3}\" y=\"{y:.3}\" font-family=\"sans-serif\" font-size=\"{size}\" fill=\"#172b4d\">{}</text>",
        escape(text)
    );
}

fn line(svg: &mut String, x1: f64, y1: f64, x2: f64, y2: f64) {
    let _ = write!(
        svg,
        "<line x1=\"{x1:.3}\" y1=\"{y1:.3}\" x2=\"{x2:.3}\" y2=\"{y2:.3}\" stroke=\"#ccd6e0\"/>"
    );
}

fn valid_points(points: &[PlotPoint]) -> VisualizationResult<()> {
    if points.is_empty()
        || points.iter().any(|p| !p.x.is_finite() || !p.y.is_finite())
        || points.windows(2).any(|p| p[0].x >= p[1].x)
    {
        return Err(invalid("curve points must be finite and increasing in x"));
    }
    Ok(())
}

fn extent(values: impl Iterator<Item = f64>) -> VisualizationResult<(f64, f64)> {
    let (mut min, mut max) = (f64::INFINITY, f64::NEG_INFINITY);
    for value in values {
        min = min.min(value);
        max = max.max(value);
    }
    if min == max {
        let pad = (min.abs() * 0.05).max(1e-6);
        min -= pad;
        max += pad;
    }
    if !min.is_finite() || !max.is_finite() || !(max - min).is_finite() || max <= min {
        return Err(VisualizationError::Numerical("axis range"));
    }
    Ok((min, max))
}

/// 所有通道共用 y 范围，但独立面板，以便比较振幅且避免堆叠覆盖。
fn curves(
    traces: &[(&str, &[PlotPoint], String)],
    x_range: (f64, f64),
    y_label: &str,
    x_label: &str,
    options: SvgOptions,
) -> VisualizationResult<String> {
    let n = traces.len();
    let panel_height = (f64::from(options.height) - 100.0) / n as f64;
    if n == 0 || panel_height < 90.0 {
        return Err(configuration(
            "increase SVG height or select fewer channels (90 px/channel)",
        ));
    }
    let y_range = extent(
        traces
            .iter()
            .flat_map(|(_, points, _)| points.iter().map(|p| p.y)),
    )?;
    let x_range = extent([x_range.0, x_range.1].into_iter())?;
    let left = 110.0;
    let right = f64::from(options.width) - 25.0;
    let mut svg = String::new();
    for (i, (channel, points, note)) in traces.iter().enumerate() {
        let top = 70.0 + i as f64 * panel_height;
        let bottom = top + panel_height - 35.0;
        text_at(
            &mut svg,
            left,
            top - 5.0,
            &format!("{channel} — {y_label} {note}"),
            12,
        );
        for tick in 0..=4 {
            let t = f64::from(tick) / 4.0;
            let y = bottom - t * (bottom - top);
            line(&mut svg, left, y, right, y);
            text_at(
                &mut svg,
                5.0,
                y + 4.0,
                &format!("{:.3e}", y_range.0 + t * (y_range.1 - y_range.0)),
                10,
            );
            let x = left + t * (right - left);
            line(&mut svg, x, top, x, bottom);
            text_at(
                &mut svg,
                x - 10.0,
                bottom + 16.0,
                &format!("{:.2}", x_range.0 + t * (x_range.1 - x_range.0)),
                10,
            );
        }
        let mut path = String::new();
        for (j, p) in points.iter().enumerate() {
            let x = left + (p.x - x_range.0) / (x_range.1 - x_range.0) * (right - left);
            let y = bottom - (p.y - y_range.0) / (y_range.1 - y_range.0) * (bottom - top);
            let command = if j == 0 { 'M' } else { 'L' };
            let _ = write!(path, "{command}{x:.3},{y:.3} ");
        }
        let _ = write!(
            svg,
            "<path d=\"{path}\" fill=\"none\" stroke=\"#1678b8\" stroke-width=\"1.2\"/>"
        );
        // 一点窗口没有线段，因此明确画点，避免“成功生成空白图”。
        if points.len() == 1 {
            let p = points[0];
            let x = left + (p.x - x_range.0) / (x_range.1 - x_range.0) * (right - left);
            let y = bottom - (p.y - y_range.0) / (y_range.1 - y_range.0) * (bottom - top);
            let _ = write!(
                svg,
                "<circle cx=\"{x:.3}\" cy=\"{y:.3}\" r=\"2\" fill=\"#1678b8\"/>"
            );
        }
    }
    text_at(
        &mut svg,
        left,
        f64::from(options.height) - 12.0,
        x_label,
        12,
    );
    Ok(svg)
}

fn waveform(
    plot: &WaveformPlot,
    options: SvgOptions,
    limits: VisualizationLimits,
) -> VisualizationResult<String> {
    v::budget(plot.traces.len(), limits.max_channels, "SVG channels")?;
    v::select(plot.traces.iter().map(|t| t.channel.clone()).collect(), &[])?;
    v::ordered_range(plot.start_seconds, plot.end_seconds)?;
    let total = plot.traces.iter().try_fold(0usize, |n, t| {
        n.checked_add(t.points.len())
            .ok_or(VisualizationError::LimitExceeded("SVG points"))
    })?;
    v::budget(total, limits.max_output_points, "SVG points")?;
    let mut traces = Vec::new();
    for trace in &plot.traces {
        v::name(&trace.channel)?;
        valid_points(&trace.points)?;
        if trace.source_samples < trace.points.len()
            || trace
                .points
                .iter()
                .any(|p| p.x < plot.start_seconds || p.x >= plot.end_seconds)
        {
            return Err(invalid("waveform points outside declared source/window"));
        }
        traces.push((
            trace.channel.as_str(),
            trace.points.as_slice(),
            format!(
                "({}/{} display/source samples)",
                trace.points.len(),
                trace.source_samples
            ),
        ));
    }
    curves(
        &traces,
        (plot.start_seconds, plot.end_seconds),
        "uV",
        "Time (s)",
        options,
    )
}

fn psd(
    plot: &PsdPlot,
    options: SvgOptions,
    limits: VisualizationLimits,
) -> VisualizationResult<String> {
    v::budget(plot.traces.len(), limits.max_channels, "SVG channels")?;
    v::select(plot.traces.iter().map(|t| t.channel.clone()).collect(), &[])?;
    if let PsdScale::Decibel { floor_uv2_per_hz } = plot.scale
        && (!floor_uv2_per_hz.is_finite() || floor_uv2_per_hz <= 0.0)
    {
        return Err(invalid("invalid PSD floor"));
    }
    let total = plot.traces.iter().try_fold(0usize, |n, t| {
        n.checked_add(t.points.len())
            .ok_or(VisualizationError::LimitExceeded("SVG points"))
    })?;
    v::budget(total, limits.max_output_points, "SVG points")?;
    let mut traces = Vec::new();
    for trace in &plot.traces {
        v::name(&trace.channel)?;
        valid_points(&trace.points)?;
        if trace.raw_uv2_per_hz.len() != trace.points.len()
            || trace.points.iter().any(|p| p.x < 0.0)
        {
            return Err(invalid("invalid PSD display shape"));
        }
        let mut floors = 0;
        for (&raw, p) in trace.raw_uv2_per_hz.iter().zip(&trace.points) {
            if !raw.is_finite() || raw < 0.0 {
                return Err(invalid("invalid raw PSD value"));
            }
            let expected = match plot.scale {
                PsdScale::Linear => raw,
                PsdScale::Decibel { floor_uv2_per_hz } => {
                    if raw < floor_uv2_per_hz {
                        floors += 1;
                    }
                    10.0 * raw.max(floor_uv2_per_hz).log10()
                }
            };
            if (expected - p.y).abs() > 1e-10 * expected.abs().max(1.0) {
                return Err(invalid("PSD display does not match raw power/scale"));
            }
        }
        if floors != trace.floored_bins {
            return Err(invalid("PSD floor count mismatch"));
        }
        let note = match plot.scale {
            PsdScale::Linear => String::new(),
            PsdScale::Decibel { floor_uv2_per_hz } => format!(
                "(floor {floor_uv2_per_hz:.2e}; {} clipped bins)",
                trace.floored_bins
            ),
        };
        traces.push((trace.channel.as_str(), trace.points.as_slice(), note));
    }
    let x_range = extent(traces.iter().flat_map(|(_, p, _)| p.iter().map(|p| p.x)))?;
    let unit = match plot.scale {
        PsdScale::Linear => "uV²/Hz",
        PsdScale::Decibel { .. } => "dB re 1 uV²/Hz",
    };
    curves(&traces, x_range, unit, "Frequency (Hz)", options)
}

/// 蓝→青绿→黄，非诊断等级。常量图使用中间色，保持色标原始 min=max。
fn color(value: f64, min: f64, max: f64) -> String {
    let t = if min == max {
        0.5
    } else {
        ((value - min) / (max - min)).clamp(0.0, 1.0)
    };
    let (a, b, u) = if t <= 0.5 {
        ([35.0, 70.0, 150.0], [30.0, 170.0, 150.0], t * 2.0)
    } else {
        ([30.0, 170.0, 150.0], [250.0, 220.0, 60.0], (t - 0.5) * 2.0)
    };
    format!(
        "#{:02x}{:02x}{:02x}",
        (a[0] + u * (b[0] - a[0])).round() as u8,
        (a[1] + u * (b[1] - a[1])).round() as u8,
        (a[2] + u * (b[2] - a[2])).round() as u8
    )
}

fn topomap(
    plot: &TopomapPlot,
    options: SvgOptions,
    limits: VisualizationLimits,
) -> VisualizationResult<String> {
    v::name(&plot.layout_name)?;
    let cells = v::product(
        plot.resolution,
        plot.resolution,
        limits.max_grid_cells,
        "SVG grid",
    )?;
    v::budget(
        plot.electrodes.len(),
        limits.max_channels.min(limits.max_output_points),
        "SVG electrodes",
    )?;
    v::budget(
        plot.skipped_channels.len(),
        limits.max_channels.min(limits.max_output_points),
        "SVG skipped channels",
    )?;
    if plot.resolution < 2
        || cells != plot.values.len()
        || plot.electrodes.len() < 3
        || !plot.color_min.is_finite()
        || !plot.color_max.is_finite()
        || plot.color_min < 0.0
        || plot.color_max < plot.color_min
    {
        return Err(invalid("invalid topomap shape or color range"));
    }
    let mut positions = BTreeSet::new();
    for e in &plot.electrodes {
        v::name(&e.channel)?;
        if !e.x.is_finite()
            || !e.y.is_finite()
            || e.x.hypot(e.y) > 1.0 + 1e-12
            || !e.value.is_finite()
            || e.value < plot.color_min
            || e.value > plot.color_max
        {
            return Err(invalid("invalid topomap electrode"));
        }
        let bits = |n: f64| if n == 0.0 { 0 } else { n.to_bits() };
        if !positions.insert((bits(e.x), bits(e.y))) {
            return Err(invalid("duplicate topomap positions"));
        }
    }
    for label in &plot.skipped_channels {
        v::name(label)?;
    }
    let labels: Vec<_> = plot
        .electrodes
        .iter()
        .map(|e| e.channel.clone())
        .chain(plot.skipped_channels.iter().cloned())
        .collect();
    v::budget(labels.len(), limits.max_channels, "SVG channels")?;
    v::select(labels, &[])?;
    let source_min = plot
        .electrodes
        .iter()
        .map(|e| e.value)
        .fold(f64::INFINITY, f64::min);
    let source_max = plot
        .electrodes
        .iter()
        .map(|e| e.value)
        .fold(f64::NEG_INFINITY, f64::max);
    if source_min != plot.color_min || source_max != plot.color_max {
        return Err(invalid("color range does not match electrode values"));
    }
    let hull = topomap::convex_hull(&plot.electrodes);
    if hull.len() < 3 {
        return Err(VisualizationError::InsufficientGeometry);
    }
    v::product(
        cells,
        hull.len(),
        limits.max_interpolation_operations,
        "SVG mask validation",
    )?;
    for (index, value) in plot.values.iter().enumerate() {
        if value.is_some() {
            let point = topomap::grid_position(
                plot.resolution,
                index / plot.resolution,
                index % plot.resolution,
            );
            if point.0.hypot(point.1) > 1.0
                || (plot.coverage == TopomapCoverage::ElectrodeHull
                    && !topomap::in_hull(&hull, point))
            {
                return Err(invalid("topomap value outside electrode coverage"));
            }
        }
    }
    if plot.values.iter().all(Option::is_none)
        || plot
            .values
            .iter()
            .flatten()
            .any(|value| !value.is_finite() || *value < plot.color_min || *value > plot.color_max)
        || (plot.measure == BandPowerMeasure::Relative && plot.color_max > 1.0)
    {
        return Err(invalid("invalid topomap values"));
    }
    let width = f64::from(options.width);
    let height = f64::from(options.height);
    let radius = ((width - 170.0) / 2.0).min((height - 180.0) / 2.0);
    let cx = (width - 110.0) / 2.0;
    let cy = 95.0 + radius;
    let cell_size = 2.0 * radius / plot.resolution as f64;
    let unit = match plot.measure {
        BandPowerMeasure::Absolute => "uV²",
        BandPowerMeasure::Relative => "ratio (0..1)",
    };
    // crispEdges 避免相邻色块在 SVG 栅格化时产生抗锯齿白缝。
    let coverage = match plot.coverage {
        TopomapCoverage::ElectrodeHull => "electrode convex hull",
        TopomapCoverage::HeadCircle => "head circle (display extrapolation)",
    };
    let mut svg = format!(
        "<desc>Layout: {}. IDW p=2, masked to {coverage}. Skipped channels: {}.</desc><defs><clipPath id=\"head-mask\"><circle cx=\"{cx:.3}\" cy=\"{cy:.3}\" r=\"{radius:.3}\"/></clipPath></defs><g clip-path=\"url(#head-mask)\" shape-rendering=\"crispEdges\">",
        escape(&plot.layout_name),
        escape(&plot.skipped_channels.join(", "))
    );
    for (i, value) in plot.values.iter().enumerate() {
        if let Some(value) = value {
            let (x, y) =
                topomap::grid_position(plot.resolution, i / plot.resolution, i % plot.resolution);
            let px = cx + x * radius - cell_size / 2.0;
            let py = cy - y * radius - cell_size / 2.0;
            let _ = write!(
                svg,
                "<rect x=\"{px:.3}\" y=\"{py:.3}\" width=\"{cell_size:.3}\" height=\"{cell_size:.3}\" fill=\"{}\"/>",
                color(*value, plot.color_min, plot.color_max)
            );
        }
    }
    svg.push_str("</g>");
    let _ = write!(
        svg,
        "<circle cx=\"{cx:.3}\" cy=\"{cy:.3}\" r=\"{radius:.3}\" fill=\"none\" stroke=\"#172b4d\" stroke-width=\"2\"/>"
    );
    let _ = write!(
        svg,
        "<path d=\"M{:.3},{:.3} L{:.3},{:.3} L{:.3},{:.3}\" fill=\"none\" stroke=\"#172b4d\" stroke-width=\"2\"/>",
        cx - 10.0,
        cy - radius,
        cx,
        cy - radius - 15.0,
        cx + 10.0,
        cy - radius
    );
    for e in &plot.electrodes {
        let x = cx + e.x * radius;
        let y = cy - e.y * radius;
        let _ = write!(
            svg,
            "<circle cx=\"{x:.3}\" cy=\"{y:.3}\" r=\"3\" fill=\"white\" stroke=\"#172b4d\"/>"
        );
        text_at(&mut svg, x + 4.0, y - 4.0, &e.channel, 10);
    }
    let bx = width - 100.0;
    for i in 0..64 {
        // 先算 [0,1] 比例，避免大功率乘以整数条目序号时中间值溢出。
        let value =
            plot.color_max - (plot.color_max - plot.color_min) * ((f64::from(i) + 0.5) / 64.0);
        let by = cy - radius + 2.0 * radius * f64::from(i) / 64.0;
        let bh = 2.0 * radius / 64.0;
        let _ = write!(
            svg,
            "<rect x=\"{bx:.3}\" y=\"{by:.3}\" width=\"18\" height=\"{bh:.3}\" shape-rendering=\"crispEdges\" fill=\"{}\"/>",
            color(value, plot.color_min, plot.color_max)
        );
    }
    text_at(
        &mut svg,
        bx - 10.0,
        cy - radius - 8.0,
        &format!("{:.3e}", plot.color_max),
        10,
    );
    text_at(
        &mut svg,
        bx - 10.0,
        cy + radius + 15.0,
        &format!("{:.3e}", plot.color_min),
        10,
    );
    text_at(
        &mut svg,
        20.0,
        65.0,
        &format!("{:?} {:?} — {unit}", plot.band, plot.measure),
        12,
    );
    text_at(&mut svg, 20.0, height - 48.0, &plot.layout_name, 11);
    let kind = if plot.schematic_layout {
        "Schematic coordinates. "
    } else {
        ""
    };
    text_at(
        &mut svg,
        20.0,
        height - 30.0,
        &format!("{kind}IDW display only; outside electrode hull is transparent."),
        10,
    );
    if !plot.skipped_channels.is_empty() {
        text_at(
            &mut svg,
            20.0,
            height - 12.0,
            &format!(
                "Skipped {} channels; see SVG description for labels.",
                plot.skipped_channels.len()
            ),
            10,
        );
    }
    Ok(svg)
}
