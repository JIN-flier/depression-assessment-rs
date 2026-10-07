//! 显式坐标适配。领域模型只保存电极标签，空间坐标由独立 layout 提供。
use serde::{Deserialize, Serialize};

/// 归一化头圆坐标：受试者左侧 x<0，鼻尖方向 y>0，观察者从头顶向下看。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElectrodePosition {
    pub channel: String,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElectrodeLayout {
    pub name: String,
    pub schematic: bool,
    pub positions: Vec<ElectrodePosition>,
}

impl ElectrodeLayout {
    /// 常用 19 导联的二维对称示意图，并非标准三维坐标的精确投影。
    /// 仅精确匹配标签；不猜测 "EEG Fp1-REF"、T7/T8 等命名或参考电极。
    /// 自定义 montage / 实测投影请传入自己的 layout，保证参考定义一致。
    #[must_use]
    pub fn schematic_10_20() -> Self {
        let entries = [
            ("Fp1", -0.30, 0.90),
            ("Fp2", 0.30, 0.90),
            ("F7", -0.85, 0.48),
            ("F3", -0.45, 0.45),
            ("Fz", 0.0, 0.50),
            ("F4", 0.45, 0.45),
            ("F8", 0.85, 0.48),
            ("T3", -0.95, 0.0),
            ("C3", -0.50, 0.0),
            ("Cz", 0.0, 0.0),
            ("C4", 0.50, 0.0),
            ("T4", 0.95, 0.0),
            ("T5", -0.85, -0.48),
            ("P3", -0.45, -0.45),
            ("Pz", 0.0, -0.50),
            ("P4", 0.45, -0.45),
            ("T6", 0.85, -0.48),
            ("O1", -0.30, -0.90),
            ("O2", 0.30, -0.90),
        ];
        Self {
            name: "10-20 schematic (19 channels)".into(),
            schematic: true,
            positions: entries
                .into_iter()
                .map(|(channel, x, y)| ElectrodePosition {
                    channel: channel.into(),
                    x,
                    y,
                })
                .collect(),
        }
    }
}
