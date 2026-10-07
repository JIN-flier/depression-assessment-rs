//! 二维 IDW(p=2) 展示插值，支持电极凸包或整个头圆的覆盖策略。
//! 覆盖策略只影响展示网格，不改变电极原值、频段功率或分析特征。
use crate::{
    error::{configuration, invalid},
    validation as v, *,
};
use domain::EegFeatures;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn prepare(
    input: &EegFeatures,
    layout: &ElectrodeLayout,
    request: &TopomapRequest,
    limits: VisualizationLimits,
) -> VisualizationResult<TopomapPlot> {
    if request.resolution < 2 {
        return Err(configuration("topomap resolution must be >= 2"));
    }
    let cells = v::product(
        request.resolution,
        request.resolution,
        limits.max_grid_cells,
        "topomap grid",
    )?;
    v::name(&layout.name)?;
    v::budget(
        layout.positions.len(),
        limits.max_channels,
        "layout positions",
    )?;
    let mut positions = BTreeMap::new();
    let mut locations = BTreeSet::new();
    for position in &layout.positions {
        v::name(&position.channel)?;
        if !position.x.is_finite()
            || !position.y.is_finite()
            || position.x.hypot(position.y) > 1.0 + 1e-12
        {
            return Err(invalid(
                "electrode positions must lie in the finite unit disk",
            ));
        }
        // +0/-0 视为同一点；不同标签占用相同位置会导致不唯一的插值。
        let bits = |n: f64| if n == 0.0 { 0 } else { n.to_bits() };
        if positions
            .insert(position.channel.as_str(), position)
            .is_some()
            || !locations.insert((bits(position.x), bits(position.y)))
        {
            return Err(invalid("duplicate electrode labels or positions"));
        }
    }
    let spectral = v::spectral(input)?;
    v::budget(
        spectral.band_power_by_channel.len(),
        limits.max_channels,
        "band-power channels",
    )?;
    let input_values =
        spectral
            .band_power_by_channel
            .values()
            .try_fold(0usize, |count, bands| {
                count
                    .checked_add(bands.len())
                    .ok_or(VisualizationError::LimitExceeded("band powers"))
            })?;
    v::budget(input_values, limits.max_input_values, "band powers")?;
    let labels = v::select(
        spectral.band_power_by_channel.keys().cloned().collect(),
        &request.channels,
    )?;
    v::budget(labels.len(), limits.max_output_points, "topomap electrodes")?;
    let mut electrodes = Vec::new();
    let mut skipped_channels = Vec::new();
    for label in labels {
        let power = spectral.band_power_by_channel[&label]
            .get(&request.band)
            .ok_or_else(|| VisualizationError::MissingBand {
                channel: label.clone(),
                band: request.band,
            })?;
        if !power.absolute.is_finite() || power.absolute < 0.0 {
            return Err(invalid("band power must be finite and non-negative"));
        }
        let Some(position) = positions.get(label.as_str()) else {
            if request.missing_positions == MissingPositionPolicy::Error {
                return Err(VisualizationError::MissingPosition(label));
            }
            skipped_channels.push(label);
            continue;
        };
        let value = match request.measure {
            BandPowerMeasure::Absolute => power.absolute,
            BandPowerMeasure::Relative => power.relative.get(),
        };
        electrodes.push(TopomapElectrode {
            channel: label,
            x: position.x,
            y: position.y,
            value,
        });
    }
    let hull = convex_hull(&electrodes);
    if hull.len() < 3 {
        return Err(VisualizationError::InsufficientGeometry);
    }
    v::product(
        cells,
        electrodes.len(),
        limits.max_interpolation_operations,
        "topomap interpolation",
    )?;
    let color_min = electrodes
        .iter()
        .map(|e| e.value)
        .fold(f64::INFINITY, f64::min);
    let color_max = electrodes
        .iter()
        .map(|e| e.value)
        .fold(f64::NEG_INFINITY, f64::max);
    let mut values = Vec::with_capacity(cells);
    for row in 0..request.resolution {
        for column in 0..request.resolution {
            let (x, y) = grid_position(request.resolution, row, column);
            let covered = request.coverage == TopomapCoverage::HeadCircle || in_hull(&hull, (x, y));
            let value = if x.hypot(y) <= 1.0 && covered {
                Some(interpolate(&electrodes, x, y, color_min, color_max))
            } else {
                None
            };
            values.push(value);
        }
    }
    if values.iter().all(Option::is_none) {
        return Err(VisualizationError::EmptyRange);
    }
    Ok(TopomapPlot {
        recording_id: input.recording_id,
        band: request.band,
        measure: request.measure,
        layout_name: layout.name.clone(),
        schematic_layout: layout.schematic,
        electrodes,
        skipped_channels,
        resolution: request.resolution,
        coverage: request.coverage,
        values,
        color_min,
        color_max,
    })
}

/// 使用像素中心采样，不把圆边界或矩形角伪装成有效空间位置。
pub(crate) fn grid_position(resolution: usize, row: usize, column: usize) -> (f64, f64) {
    (
        -1.0 + 2.0 * (column as f64 + 0.5) / resolution as f64,
        1.0 - 2.0 * (row as f64 + 0.5) / resolution as f64,
    )
}

fn cross(o: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
}

/// 单调链凸包，去掉共线边上的中间点，避免零面积被误认为空间覆盖。
pub(crate) fn convex_hull(electrodes: &[TopomapElectrode]) -> Vec<(f64, f64)> {
    let mut points: Vec<_> = electrodes.iter().map(|e| (e.x, e.y)).collect();
    points.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    if points.len() < 3 {
        return points;
    }
    let chain = |iter: Vec<(f64, f64)>| {
        let mut side = Vec::new();
        for p in iter {
            while side.len() >= 2 && cross(side[side.len() - 2], side[side.len() - 1], p) <= 1e-12 {
                side.pop();
            }
            side.push(p);
        }
        side.pop();
        side
    };
    let mut lower = chain(points.clone());
    points.reverse();
    lower.extend(chain(points));
    lower
}

pub(crate) fn in_hull(hull: &[(f64, f64)], point: (f64, f64)) -> bool {
    hull.iter()
        .zip(hull.iter().cycle().skip(1))
        .all(|(&a, &b)| cross(a, b, point) >= -1e-12)
}

/// 数学等价于 1/d² 权重，用最小距离归一化以避免近电极时权重溢出。
/// 电极重合查询直接返回原值；先缩放功率再加权，避免大功率求和溢出。
fn interpolate(electrodes: &[TopomapElectrode], x: f64, y: f64, min: f64, max: f64) -> f64 {
    let distance = |e: &TopomapElectrode| (e.x - x).powi(2) + (e.y - y).powi(2);
    if let Some(e) = electrodes.iter().find(|e| distance(e) <= 1e-24) {
        return e.value;
    }
    if min == max {
        return min;
    }
    let nearest = electrodes
        .iter()
        .map(distance)
        .fold(f64::INFINITY, f64::min);
    let mut weight_sum = 0.0;
    let mut scaled_sum = 0.0;
    for e in electrodes {
        let weight = nearest / distance(e);
        weight_sum += weight;
        scaled_sum += weight * ((e.value - min) / (max - min));
    }
    (min + (max - min) * (scaled_sum / weight_sum).clamp(0.0, 1.0)).clamp(min, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn electrode(x: f64, y: f64, value: f64) -> TopomapElectrode {
        TopomapElectrode {
            channel: "test".into(),
            x,
            y,
            value,
        }
    }
    #[test]
    fn idw_matches_independent_distance_weights_and_recovers_electrodes() {
        let points = [
            electrode(-0.5, 0.0, 2.0),
            electrode(0.5, 0.0, 6.0),
            electrode(0.0, 0.5, 10.0),
        ];
        assert_eq!(interpolate(&points, -0.5, 0.0, 2.0, 10.0), 2.0);
        assert_eq!(interpolate(&points, 0.0, 0.0, 2.0, 10.0), 6.0);
        let weights = [1.0 / 0.40, 1.0 / 0.20, 1.0 / 0.10];
        let expected =
            (2.0 * weights[0] + 6.0 * weights[1] + 10.0 * weights[2]) / weights.iter().sum::<f64>();
        assert!((interpolate(&points, 0.1, 0.2, 2.0, 10.0) - expected).abs() < 1e-12);
    }
    #[test]
    fn large_constant_values_and_close_positions_stay_finite() {
        let mut points = [
            electrode(-0.5, 0.0, f64::MAX),
            electrode(0.5, 0.0, f64::MAX),
            electrode(0.0, 0.5, f64::MAX),
        ];
        assert_eq!(interpolate(&points, 0.0, 0.0, f64::MAX, f64::MAX), f64::MAX);
        points[0].value = 0.0;
        let value = interpolate(&points, -0.5 + 1e-10, 0.0, 0.0, f64::MAX);
        assert!(value.is_finite() && value >= 0.0);
    }
}
