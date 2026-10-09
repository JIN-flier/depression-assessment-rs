//! 受限 TrueType reader：仅解析 PDF 所需的表目录、cmap 4/12 和横向度量。
//! 不执行 hinting、不解压字体、不解析轮廓；字体的 glyf 原封不动嵌入 PDF。
//! 每一次外部偏移读取都通过 slice::get 检查，损坏/截断文件返回错误而非 panic。
use super::{ExportError, FontOptions};
use std::{collections::BTreeMap, io::Read, path::PathBuf};
const MAX_FONT_BYTES: u64 = 32 * 1024 * 1024;

pub(super) struct Font {
    pub bytes: Vec<u8>,
    tables: BTreeMap<[u8; 4], (usize, usize)>,
    pub units: u16,
    pub bbox: [i16; 4],
    pub ascent: i16,
    pub descent: i16,
    metrics: u16,
    glyphs: u16,
}
impl Font {
    pub fn load(options: &FontOptions) -> Result<Self, ExportError> {
        let configured = options
            .pdf_font
            .clone()
            .or_else(|| std::env::var_os("EEG_REPORT_FONT").map(PathBuf::from));
        let path = configured
            .or_else(|| {
                [
                    "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
                    "/usr/share/fonts/truetype/wqy/wqy-microhei.ttf",
                    "C:/Windows/Fonts/simhei.ttf",
                    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
                ]
                .iter()
                .map(PathBuf::from)
                .find(|p| p.is_file())
            })
            .ok_or(ExportError::FontUnavailable)?;
        let file = std::fs::File::open(path)?;
        if file.metadata()?.len() > MAX_FONT_BYTES {
            return Err(ExportError::TooLarge);
        }
        let mut bytes = Vec::new();
        file.take(MAX_FONT_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_FONT_BYTES {
            return Err(ExportError::TooLarge);
        }
        Self::parse(bytes)
    }
    pub fn parse(bytes: Vec<u8>) -> Result<Self, ExportError> {
        if bytes.get(..4) != Some(&[0, 1, 0, 0]) {
            return Err(ExportError::InvalidFont);
        }
        let count = u16be(&bytes, 4)? as usize;
        let mut tables = BTreeMap::new();
        for i in 0..count {
            let p = 12 + i * 16;
            let tag: [u8; 4] = bytes
                .get(p..p + 4)
                .ok_or(ExportError::InvalidFont)?
                .try_into()
                .map_err(|_| ExportError::InvalidFont)?;
            let offset = u32be(&bytes, p + 8)? as usize;
            let len = u32be(&bytes, p + 12)? as usize;
            bytes
                .get(offset..offset.checked_add(len).ok_or(ExportError::InvalidFont)?)
                .ok_or(ExportError::InvalidFont)?;
            if tables.insert(tag, (offset, len)).is_some() {
                return Err(ExportError::InvalidFont);
            }
        }
        let mut f = Self {
            bytes,
            tables,
            units: 0,
            bbox: [0; 4],
            ascent: 0,
            descent: 0,
            metrics: 0,
            glyphs: 0,
        };
        f.table(b"glyf")?;
        f.table(b"loca")?; // CFF/TTC/WOFF 不冒充 TrueType。
        let head = f.table(b"head")?;
        f.units = u16be(head, 18)?;
        let head = f.table(b"head")?;
        let bbox = [
            i16be(head, 36)?,
            i16be(head, 38)?,
            i16be(head, 40)?,
            i16be(head, 42)?,
        ];
        f.bbox = bbox;
        let hhea = f.table(b"hhea")?;
        let ascent = i16be(hhea, 4)?;
        let descent = i16be(hhea, 6)?;
        let metrics = u16be(hhea, 34)?;
        f.ascent = ascent;
        f.descent = descent;
        f.metrics = metrics;
        f.glyphs = u16be(f.table(b"maxp")?, 4)?;
        if f.units == 0 || f.metrics == 0 || f.metrics > f.glyphs {
            return Err(ExportError::InvalidFont);
        }
        let need = usize::from(f.metrics) * 4 + usize::from(f.glyphs - f.metrics) * 2;
        if f.table(b"hmtx")?.len() < need {
            return Err(ExportError::InvalidFont);
        }
        // fsType restricted-license 与 bitmap-only 字体不嵌入。全部嵌入满足 no-subsetting。
        if let Ok(os2) = f.table(b"OS/2")
            && u16be(os2, 8)? & 0x0202 != 0
        {
            return Err(ExportError::InvalidFont);
        }
        Ok(f)
    }
    fn table(&self, tag: &[u8; 4]) -> Result<&[u8], ExportError> {
        let &(p, n) = self.tables.get(tag).ok_or(ExportError::InvalidFont)?;
        self.bytes.get(p..p + n).ok_or(ExportError::InvalidFont)
    }
    pub fn glyph(&self, ch: char) -> Result<u16, ExportError> {
        let cmap = self.table(b"cmap")?;
        let count = u16be(cmap, 2)? as usize;
        // 优先 format 12 支持非 BMP；只读取 Unicode 平台或 Microsoft Unicode 映射。
        for desired in [12, 4] {
            for i in 0..count {
                let p = 4 + i * 8;
                let platform = u16be(cmap, p)?;
                let encoding = u16be(cmap, p + 2)?;
                if platform != 0 && !(platform == 3 && matches!(encoding, 1 | 10)) {
                    continue;
                }
                let offset = u32be(cmap, p + 4)? as usize;
                let sub = cmap.get(offset..).ok_or(ExportError::InvalidFont)?;
                if u16be(sub, 0)? != desired {
                    continue;
                }
                let len = if desired == 12 {
                    u32be(sub, 4)? as usize
                } else {
                    u16be(sub, 2)? as usize
                };
                let sub = sub.get(..len).ok_or(ExportError::InvalidFont)?;
                let glyph = if desired == 12 {
                    glyph12(sub, ch as u32)?
                } else {
                    glyph4(sub, ch as u32)?
                };
                if glyph != 0 && glyph < u32::from(self.glyphs) {
                    return Ok(glyph as u16);
                }
            }
        }
        Err(ExportError::MissingGlyph(ch))
    }
    pub fn width(&self, glyph: u16) -> Result<f64, ExportError> {
        if glyph >= self.glyphs {
            return Err(ExportError::InvalidFont);
        }
        let metric = glyph.min(self.metrics - 1) as usize;
        Ok(f64::from(u16be(self.table(b"hmtx")?, metric * 4)?) * 1000.0 / f64::from(self.units))
    }
    pub fn scale(&self, value: i16) -> f64 {
        f64::from(value) * 1000.0 / f64::from(self.units)
    }
}
fn glyph12(s: &[u8], ch: u32) -> Result<u32, ExportError> {
    let n = u32be(s, 12)? as usize;
    if n > s.len().saturating_sub(16) / 12 {
        return Err(ExportError::InvalidFont);
    }
    for i in 0..n {
        let p = 16 + i * 12;
        let start = u32be(s, p)?;
        let end = u32be(s, p + 4)?;
        if ch >= start && ch <= end {
            return u32be(s, p + 8)?
                .checked_add(ch - start)
                .ok_or(ExportError::InvalidFont);
        }
    }
    Ok(0)
}
fn glyph4(s: &[u8], ch: u32) -> Result<u32, ExportError> {
    if ch > 0xffff {
        return Ok(0);
    }
    let n = u16be(s, 6)? as usize / 2;
    if n == 0 || 16 + n * 8 > s.len() {
        return Err(ExportError::InvalidFont);
    }
    for i in 0..n {
        let end = u16be(s, 14 + i * 2)? as u32;
        let start = u16be(s, 16 + n * 2 + i * 2)? as u32;
        if ch < start || ch > end {
            continue;
        }
        let delta = u16be(s, 16 + n * 4 + i * 2)?;
        let range_p = 16 + n * 6 + i * 2;
        let range = u16be(s, range_p)? as usize;
        let glyph = if range == 0 {
            (ch as u16).wrapping_add(delta)
        } else {
            let g = u16be(s, range_p + range + (ch - start) as usize * 2)?;
            if g == 0 { 0 } else { g.wrapping_add(delta) }
        };
        return Ok(u32::from(glyph));
    }
    Ok(0)
}
fn u16be(s: &[u8], p: usize) -> Result<u16, ExportError> {
    Ok(u16::from_be_bytes(
        s.get(p..p.checked_add(2).ok_or(ExportError::InvalidFont)?)
            .ok_or(ExportError::InvalidFont)?
            .try_into()
            .map_err(|_| ExportError::InvalidFont)?,
    ))
}
fn i16be(s: &[u8], p: usize) -> Result<i16, ExportError> {
    Ok(u16be(s, p)? as i16)
}
fn u32be(s: &[u8], p: usize) -> Result<u32, ExportError> {
    Ok(u32::from_be_bytes(
        s.get(p..p.checked_add(4).ok_or(ExportError::InvalidFont)?)
            .ok_or(ExportError::InvalidFont)?
            .try_into()
            .map_err(|_| ExportError::InvalidFont)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arbitrary_truncated_fonts_fail_without_panicking() {
        for n in 0..256 {
            let mut v = vec![0; n];
            if n >= 4 {
                v[..4].copy_from_slice(&[0, 1, 0, 0]);
            }
            assert!(Font::parse(v).is_err());
        }
    }
    #[test]
    fn cmap12_supports_surrogates_and_checks_offsets() {
        let mut v = vec![0; 28];
        v[12..16].copy_from_slice(&1u32.to_be_bytes());
        v[16..20].copy_from_slice(&0x1f600u32.to_be_bytes());
        v[20..24].copy_from_slice(&0x1f600u32.to_be_bytes());
        v[24..28].copy_from_slice(&7u32.to_be_bytes());
        assert_eq!(glyph12(&v, 0x1f600).unwrap(), 7);
        assert!(glyph12(&v[..27], 0x1f600).is_err());
    }
    // 合成表目录专门测试 reader，不作为可渲染字体使用，不依赖系统字体安装。
    fn fixture(fs_type: u16) -> Vec<u8> {
        let mut head = vec![0; 54];
        head[18..20].copy_from_slice(&1000u16.to_be_bytes());
        let mut hhea = vec![0; 36];
        hhea[34..36].copy_from_slice(&2u16.to_be_bytes());
        let mut maxp = vec![0; 6];
        maxp[4..6].copy_from_slice(&2u16.to_be_bytes());
        let mut hmtx = vec![0; 8];
        hmtx[4..6].copy_from_slice(&600u16.to_be_bytes());
        let mut os2 = vec![0; 10];
        os2[8..10].copy_from_slice(&fs_type.to_be_bytes());
        let mut sub = vec![0; 32];
        for (offset, value) in [
            (0, 4u16),
            (2, 32),
            (6, 4),
            (14, 0x4e2d),
            (16, 0xffff),
            (20, 0x4e2d),
            (22, 0xffff),
            (24, 1u16.wrapping_sub(0x4e2d)),
            (26, 1),
        ] {
            sub[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
        }
        let mut cmap = vec![0; 12];
        cmap[2..4].copy_from_slice(&1u16.to_be_bytes());
        cmap[4..6].copy_from_slice(&3u16.to_be_bytes());
        cmap[6..8].copy_from_slice(&1u16.to_be_bytes());
        cmap[8..12].copy_from_slice(&12u32.to_be_bytes());
        cmap.extend(sub);
        let tables = [
            (*b"head", head),
            (*b"hhea", hhea),
            (*b"maxp", maxp),
            (*b"hmtx", hmtx),
            (*b"cmap", cmap),
            (*b"OS/2", os2),
            (*b"glyf", vec![]),
            (*b"loca", vec![]),
        ];
        let mut bytes = vec![0; 12 + 16 * tables.len()];
        bytes[..4].copy_from_slice(&[0, 1, 0, 0]);
        bytes[4..6].copy_from_slice(&(tables.len() as u16).to_be_bytes());
        for (i, (tag, data)) in tables.into_iter().enumerate() {
            let offset = bytes.len() as u32;
            let p = 12 + i * 16;
            bytes[p..p + 4].copy_from_slice(&tag);
            bytes[p + 8..p + 12].copy_from_slice(&offset.to_be_bytes());
            bytes[p + 12..p + 16].copy_from_slice(&(data.len() as u32).to_be_bytes());
            bytes.extend(data);
        }
        bytes
    }
    #[test]
    fn true_type_mapping_width_and_embedding_permission_are_checked() {
        let font = Font::parse(fixture(0)).unwrap();
        assert_eq!(font.glyph('中').unwrap(), 1);
        assert_eq!(font.width(1).unwrap(), 600.0);
        assert!(matches!(
            font.glyph('文'),
            Err(ExportError::MissingGlyph('文'))
        ));
        assert!(font.width(2).is_err());
        assert!(matches!(
            Font::parse(fixture(2)),
            Err(ExportError::InvalidFont)
        ));
        assert!(matches!(
            Font::parse(fixture(0x200)),
            Err(ExportError::InvalidFont)
        ));
        assert!(Font::parse(fixture(0x100)).is_ok()); // 全量嵌入允许禁止 subsetting 的字体。
        let mut bytes = fixture(0);
        bytes[20..24].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(Font::parse(bytes).is_err());
        let valid = fixture(0);
        for n in 0..valid.len() {
            assert!(Font::parse(valid[..n].to_vec()).is_err());
        }
    }
}
