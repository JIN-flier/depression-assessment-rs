//! 最小 OOXML 文档包，ZIP 使用标准 STORE（无压缩）方法。
//! 固定部件名，不支持任意路径或 ZIP64；CRC32/长度/偏移均由本地数据生成。
//! 这让 V1 不需要新增 cargo 包。后续图片/复杂排版可替换此编码器。
use super::{ExportError, MAX_OUTPUT_BYTES, archive::Paragraph};
use std::fmt::Write;

pub(super) fn encode(paragraphs: &[Paragraph]) -> Result<Vec<u8>, ExportError> {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>"#,
    );
    for p in paragraphs {
        let style = match p.level {
            2 => "Title",
            1 => "Heading1",
            _ => "Normal",
        };
        // 每个真实换行变为 w:br，不把换行依赖 Word 的空白折叠实现。
        write!(
            xml,
            "<w:p><w:pPr><w:pStyle w:val=\"{style}\"/></w:pPr><w:r>"
        )
        .map_err(|_| ExportError::TooLarge)?;
        for (i, line) in p.text.replace('\r', "").split('\n').enumerate() {
            if i > 0 {
                xml.push_str("<w:br/>");
            }
            write!(xml, "<w:t xml:space=\"preserve\">{}</w:t>", escape(line))
                .map_err(|_| ExportError::TooLarge)?;
        }
        xml.push_str("</w:r></w:p>");
        if xml.len() > MAX_OUTPUT_BYTES {
            return Err(ExportError::TooLarge);
        }
    }
    xml.push_str(r#"<w:sectPr><w:footerReference w:type="default" r:id="footer" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"/><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1134" w:right="1134" w:bottom="1134" w:left="1134" w:header="567" w:footer="567"/></w:sectPr></w:body></w:document>"#);
    zip(&[
        (
            "[Content_Types].xml",
            r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/footer.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml"/></Types>"#,
        ),
        (
            "_rels/.rels",
            r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="document" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#,
        ),
        (
            "word/_rels/document.xml.rels",
            r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="styles" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="footer" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer" Target="footer.xml"/></Relationships>"#,
        ),
        (
            "word/styles.xml",
            r#"<?xml version="1.0"?><w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Arial" w:hAnsi="Arial" w:eastAsia="Noto Sans CJK SC"/><w:sz w:val="22"/><w:lang w:val="zh-CN" w:eastAsia="zh-CN"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="120" w:line="320" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/><w:basedOn w:val="Normal"/><w:pPr><w:keepNext/><w:spacing w:before="0" w:after="240"/></w:pPr><w:rPr><w:b/><w:color w:val="000000"/><w:sz w:val="36"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:pPr><w:keepNext/><w:keepLines/><w:spacing w:before="200" w:after="120"/></w:pPr><w:rPr><w:b/><w:sz w:val="26"/></w:rPr></w:style></w:styles>"#,
        ),
        (
            "word/footer.xml",
            r#"<?xml version="1.0"?><w:ftr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>PAGE</w:instrText></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p></w:ftr>"#,
        ),
        ("word/document.xml", &xml),
    ])
}
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn u16le(out: &mut Vec<u8>, n: u16) {
    out.extend_from_slice(&n.to_le_bytes());
}
fn u32le(out: &mut Vec<u8>, n: u32) {
    out.extend_from_slice(&n.to_le_bytes());
}
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for b in bytes {
        crc ^= u32::from(*b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}
fn zip(files: &[(&str, &str)]) -> Result<Vec<u8>, ExportError> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for &(name, text) in files {
        let data = text.as_bytes();
        let crc = crc32(data);
        let size = u32::try_from(data.len()).map_err(|_| ExportError::TooLarge)?;
        let offset = u32::try_from(out.len()).map_err(|_| ExportError::TooLarge)?;
        let len = u16::try_from(name.len()).map_err(|_| ExportError::TooLarge)?;
        u32le(&mut out, 0x04034b50);
        u16le(&mut out, 20);
        u16le(&mut out, 0x0800);
        u16le(&mut out, 0);
        u16le(&mut out, 0);
        u16le(&mut out, 33); // 1980-01-01
        u32le(&mut out, crc);
        u32le(&mut out, size);
        u32le(&mut out, size);
        u16le(&mut out, len);
        u16le(&mut out, 0);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        u32le(&mut central, 0x02014b50);
        u16le(&mut central, 20);
        u16le(&mut central, 20);
        u16le(&mut central, 0x0800);
        u16le(&mut central, 0);
        u16le(&mut central, 0);
        u16le(&mut central, 33);
        u32le(&mut central, crc);
        u32le(&mut central, size);
        u32le(&mut central, size);
        u16le(&mut central, len);
        for _ in 0..4 {
            u16le(&mut central, 0);
        }
        u32le(&mut central, 0);
        u32le(&mut central, offset);
        central.extend_from_slice(name.as_bytes());
        if out.len() + central.len() > MAX_OUTPUT_BYTES {
            return Err(ExportError::TooLarge);
        }
    }
    let offset = out.len() as u32;
    let size = central.len() as u32;
    out.extend(central);
    u32le(&mut out, 0x06054b50);
    u16le(&mut out, 0);
    u16le(&mut out, 0);
    u16le(&mut out, files.len() as u16);
    u16le(&mut out, files.len() as u16);
    u32le(&mut out, size);
    u32le(&mut out, offset);
    u16le(&mut out, 0);
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn crc_matches_standard_check_vector_and_xml_escapes() {
        assert_eq!(crc32(b"123456789"), 0xcbf43926);
        assert_eq!(escape("<&\"'"), "&lt;&amp;&quot;&apos;");
    }
}
