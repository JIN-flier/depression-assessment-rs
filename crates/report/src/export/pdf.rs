//! PDF 1.7：A4、真实字体宽度换行、标题保留下一行、自动分页、页脚页码。
//! Unicode 使用独立 CID + ToUnicode，非 BMP 用 UTF-16 代理对映射，可搜索/复制。
//! ASCII 使用 PDF 标准 Helvetica，中文使用本地 TrueType 全量嵌入；无需联网。
use super::{ExportError, FontOptions, MAX_OUTPUT_BYTES, archive::Paragraph, font::Font};
use std::{collections::BTreeMap, fmt::Write};
const LEFT: f64 = 50.0;
const RIGHT: f64 = 545.0;
const TOP: f64 = 785.0;
const BOTTOM: f64 = 62.0;

pub(super) fn encode(
    paragraphs: &[Paragraph],
    options: &FontOptions,
) -> Result<Vec<u8>, ExportError> {
    let font = Font::load(options)?;
    let mut chars = BTreeMap::new();
    for p in paragraphs {
        for ch in p
            .text
            .chars()
            .filter(|c| latin_code(*c).is_none() && !matches!(*c, '\n' | '\r' | '\t'))
        {
            if !chars.contains_key(&ch) {
                let cid = u16::try_from(chars.len() + 1).map_err(|_| ExportError::TooLarge)?;
                let glyph = font.glyph(ch)?;
                chars.insert(ch, (cid, glyph, font.width(glyph)?));
            }
        }
    }
    let mut pages = vec![String::new()];
    let mut y = TOP;
    for p in paragraphs {
        let size = match p.level {
            2 => 18.0,
            1 => 13.0,
            _ => 10.5,
        };
        let lines = wrap(&p.text, size, &chars);
        let needed = if p.level > 0 {
            size * 1.6 + 22.0
        } else {
            size * 1.6
        };
        if y - needed < BOTTOM {
            pages.push(String::new());
            y = TOP;
        }
        for line in lines {
            if y - size * 1.6 < BOTTOM {
                pages.push(String::new());
                y = TOP;
            }
            let page = pages.last_mut().ok_or(ExportError::InvalidArchive)?;
            draw(page, &line, size, LEFT, y, &chars)?;
            y -= size * 1.6;
        }
        y -= if p.level > 0 { 8.0 } else { 5.0 };
        if pages.len() > 4096 {
            return Err(ExportError::TooLarge);
        }
    }
    let count = pages.len();
    for (i, page) in pages.iter_mut().enumerate() {
        draw(
            page,
            &format!("{} / {}", i + 1, count),
            9.0,
            275.0,
            32.0,
            &chars,
        )?;
    }
    // 固定对象 ID 是本编码器内部的引用表；后续 page/content 追加在其后。
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(), // 1
        Vec::new(), // 2 pages，在分配完 page ID 后填入
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec(), // 3
        b"<< /Type /Font /Subtype /Type0 /BaseFont /EEGFont /Encoding /Identity-H /DescendantFonts [5 0 R] /ToUnicode 9 0 R >>".to_vec(), // 4
    ];
    let widths = chars
        .values()
        .map(|(cid, _, w)| format!("{cid} [{w:.4}] "))
        .collect::<String>();
    objects.push(format!("<< /Type /Font /Subtype /CIDFontType2 /BaseFont /EEGFont /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor 6 0 R /CIDToGIDMap 8 0 R /DW 1000 /W [{widths}] >>").into_bytes());
    let bbox = font.bbox.map(|v| font.scale(v));
    objects.push(format!("<< /Type /FontDescriptor /FontName /EEGFont /Flags 4 /FontBBox [{} {} {} {}] /ItalicAngle 0 /Ascent {} /Descent {} /CapHeight {} /StemV 80 /FontFile2 7 0 R >>",bbox[0],bbox[1],bbox[2],bbox[3],font.scale(font.ascent),font.scale(font.descent),font.scale(font.ascent)).into_bytes());
    objects.push(stream(
        &font.bytes,
        &format!("/Length1 {}", font.bytes.len()),
    ));
    let mut gids = vec![0u8; (chars.len() + 1) * 2];
    for (cid, glyph, _) in chars.values() {
        gids[*cid as usize * 2..*cid as usize * 2 + 2].copy_from_slice(&glyph.to_be_bytes());
    }
    objects.push(stream(&gids, ""));
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /EEGUnicode def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    let entries: Vec<_> = chars.iter().collect();
    for group in entries.chunks(100) {
        writeln!(cmap, "{} beginbfchar", group.len()).map_err(|_| ExportError::TooLarge)?;
        for (ch, (cid, _, _)) in group {
            let mut utf = [0; 2];
            let hex = ch
                .encode_utf16(&mut utf)
                .iter()
                .map(|v| format!("{v:04X}"))
                .collect::<String>();
            writeln!(cmap, "<{cid:04X}> <{hex}>").map_err(|_| ExportError::TooLarge)?;
        }
        cmap.push_str("endbfchar\n");
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend");
    objects.push(stream(cmap.as_bytes(), ""));
    let mut kids = String::new();
    for page in pages {
        let id = objects.len() + 1;
        write!(kids, "{id} 0 R ").map_err(|_| ExportError::TooLarge)?;
        objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595.28 841.89] /Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> /Contents {} 0 R >>",id+1).into_bytes());
        objects.push(stream(page.as_bytes(), ""));
    }
    objects[1] = format!("<< /Type /Pages /Count {count} /Kids [{kids}] >>").into_bytes();
    let mut out = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::new();
    for (i, obj) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(obj);
        out.extend_from_slice(b"\nendobj\n");
        if out.len() > MAX_OUTPUT_BYTES {
            return Err(ExportError::TooLarge);
        }
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for p in offsets {
        out.extend_from_slice(format!("{p:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    Ok(out)
}
fn stream(bytes: &[u8], extra: &str) -> Vec<u8> {
    let mut out = format!("<< /Length {} {extra} >>\nstream\n", bytes.len()).into_bytes();
    out.extend_from_slice(bytes);
    out.extend_from_slice(b"\nendstream");
    out
}
// Helvetica 标准 ASCII advance width，单位 1/1000 em。换行和绘制共用度量。
fn ascii_width(ch: char) -> f64 {
    const W: [u16; 95] = [
        278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556,
        556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722,
        722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722,
        667, 944, 667, 667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556,
        556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500,
        500, 334, 260, 334, 584,
    ];
    if (' '..='~').contains(&ch) {
        f64::from(W[ch as usize - 32])
    } else {
        // WinAnsi Latin-1 度量；不能用统一宽度近似 æ / Æ 等宽字形，否则会越过页边。
        const LATIN: [u16; 96] = [
            278, 333, 556, 556, 556, 556, 260, 556, 333, 737, 370, 556, 584, 333, 737, 333, 400,
            584, 333, 333, 333, 556, 537, 278, 333, 333, 365, 556, 834, 834, 834, 611, 667, 667,
            667, 667, 667, 667, 1000, 722, 667, 667, 667, 667, 278, 278, 278, 278, 722, 722, 778,
            778, 778, 778, 778, 584, 778, 722, 722, 722, 722, 667, 667, 611, 556, 556, 556, 556,
            556, 556, 889, 500, 556, 556, 556, 556, 278, 278, 278, 278, 556, 556, 556, 556, 556,
            556, 556, 584, 611, 556, 556, 556, 556, 500, 556, 500,
        ];
        if ('\u{a0}'..='\u{ff}').contains(&ch) {
            f64::from(LATIN[ch as usize - 160])
        } else {
            278.0
        }
    }
}
// 标准 Helvetica 的 WinAnsi 码位可承担常见 Latin 单位字符；中文字体
// 通常刻意不含 Latin glyph，因此不能把缺少的上标误判为缺少中文字体。
fn latin_code(ch: char) -> Option<u8> {
    if (' '..='~').contains(&ch) || ('\u{a0}'..='\u{ff}').contains(&ch) {
        return Some(ch as u8);
    }
    None
}
type Chars = BTreeMap<char, (u16, u16, f64)>;
fn width(c: char, chars: &Chars) -> f64 {
    if latin_code(c).is_some() {
        ascii_width(c)
    } else {
        chars.get(&c).map_or(1000.0, |v| v.2)
    }
}
fn wrap(s: &str, size: f64, chars: &Chars) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut w = 0.0;
    for c in s.chars() {
        if c == '\r' {
            continue;
        }
        if c == '\n' {
            lines.push(std::mem::take(&mut line));
            w = 0.0;
            continue;
        }
        let c = if c == '\t' { ' ' } else { c };
        let advance = width(c, chars) * size / 1000.0;
        if w + advance > RIGHT - LEFT && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
            w = 0.0;
        }
        line.push(c);
        w += advance;
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}
fn draw(
    out: &mut String,
    s: &str,
    size: f64,
    mut x: f64,
    y: f64,
    chars: &Chars,
) -> Result<(), ExportError> {
    // 按字体合并 runs，避免每个字符一个 PDF text object 导致文件膨胀。
    let mut run = String::new();
    let mut advance = 0.0;
    let mut ascii = None;
    let flush =
        |out: &mut String, run: &mut String, ascii: bool, x: f64| -> Result<(), ExportError> {
            writeln!(
                out,
                "BT /F{} {size:.2} Tf 1 0 0 1 {x:.3} {y:.3} Tm <{}> Tj ET",
                if ascii { 1 } else { 2 },
                run
            )
            .map_err(|_| ExportError::TooLarge)?;
            run.clear();
            Ok(())
        };
    for c in s.chars() {
        if ascii.is_some_and(|a| a != latin_code(c).is_some()) {
            flush(out, &mut run, ascii.unwrap_or(true), x)?;
            x += advance;
            advance = 0.0;
        }
        ascii = Some(latin_code(c).is_some());
        if let Some(code) = latin_code(c) {
            write!(run, "{code:02X}").map_err(|_| ExportError::TooLarge)?;
        } else {
            let cid = chars.get(&c).ok_or(ExportError::MissingGlyph(c))?.0;
            write!(run, "{cid:04X}").map_err(|_| ExportError::TooLarge)?;
        }
        advance += width(c, chars) * size / 1000.0;
    }
    if let Some(ascii) = ascii {
        flush(out, &mut run, ascii, x)?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lines_use_font_metrics_and_preserve_all_characters() {
        let chars = BTreeMap::from([('中', (1, 1, 1000.0))]);
        let input = "中".repeat(200);
        let lines = wrap(&input, 10.0, &chars);
        assert_eq!(lines.concat(), input);
        assert!(lines.iter().all(|l| l.chars().count() * 10 <= 495));
        let ascii = "W".repeat(200);
        let lines = wrap(&ascii, 10.0, &chars);
        assert_eq!(lines.concat(), ascii);
        assert!(
            lines
                .iter()
                .all(|l| l.len() as f64 * ascii_width('W') * 0.01 <= 495.0)
        );
    }
}
