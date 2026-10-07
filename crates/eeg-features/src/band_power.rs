//! 离散 PSD 的积分约定单独封装；不依赖 FFT、录制、质量评分或 UI。
use crate::{FeatureError, FeatureResult, FrequencyRange};
use std::ops::Range;

/// 用同一频率网格解析分子/分母，避免相邻频段重复计入边界 bin。
/// 不裁剪超 Nyquist 的频段，也不将没有 bin 的窄频段输出成假零值。
pub(crate) fn bins(
    frequencies: &[f64],
    range: FrequencyRange,
    nyquist: f64,
    bin_width: f64,
) -> FeatureResult<Range<usize>> {
    let start = frequencies.partition_point(|&frequency| frequency < range.low_hz);
    let end = if range.high_hz == nyquist {
        frequencies.len()
    } else {
        frequencies.partition_point(|&frequency| frequency < range.high_hz)
    };
    if start >= end {
        return Err(FeatureError::UnresolvedRange {
            low_hz: range.low_hz,
            high_hz: range.high_hz,
            bin_width_hz: bin_width,
        });
    }
    Ok(start..end)
}

/// 矩形积分 sum(PSD[k])*df。单边谱的 DC/Nyquist 已正确缩放，
/// 再采用梯形端点半权会丢失这两个 bin 的能量，故这里保留完整 bin。
pub(crate) fn integrate(psd: &[f64], bins: Range<usize>, bin_width: f64) -> FeatureResult<f64> {
    let power = psd[bins].iter().sum::<f64>() * bin_width;
    if !power.is_finite() || power < 0.0 {
        return Err(FeatureError::Numerical("band power integration"));
    }
    Ok(power)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adjacent_ranges_partition_bins_and_include_nyquist_once() {
        let f = [0.0, 1.0, 2.0, 3.0, 4.0];
        let lower = bins(
            &f,
            FrequencyRange {
                low_hz: 0.0,
                high_hz: 2.0,
            },
            4.0,
            1.0,
        )
        .unwrap();
        let upper = bins(
            &f,
            FrequencyRange {
                low_hz: 2.0,
                high_hz: 4.0,
            },
            4.0,
            1.0,
        )
        .unwrap();
        assert_eq!(lower, 0..2);
        assert_eq!(upper, 2..5);
        assert_eq!(integrate(&[1.0; 5], lower, 1.0).unwrap(), 2.0);
        assert_eq!(integrate(&[1.0; 5], upper, 1.0).unwrap(), 3.0);
    }

    #[test]
    fn no_bin_is_an_explicit_error() {
        assert!(matches!(
            bins(
                &[0.0, 2.0, 4.0],
                FrequencyRange {
                    low_hz: 0.5,
                    high_hz: 1.5
                },
                4.0,
                2.0
            ),
            Err(FeatureError::UnresolvedRange { .. })
        ));
    }
}
