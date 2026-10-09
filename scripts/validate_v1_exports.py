#!/usr/bin/env python3
"""独立校验 P10 预览产物，仅读文件，无第三方 Python/Cargo 包。

用 zipfile / XML / csv 的标准解析器验证结构与 JSON 全量值的对应关系，
而非依赖生成器的实现细节。PDF 另外使用 Poppler 检查页数和正文可提取性。
运行：python3 scripts/validate_v1_exports.py /tmp/p10-export-preview
"""
import csv
import io
import json
from pathlib import Path
import shutil
import subprocess
import sys
import xml.etree.ElementTree as ET


def leaves(value, path=""):
    if isinstance(value, dict) and value:
        for key, child in value.items():
            escaped = key.replace("~", "~0").replace("/", "~1")
            yield from leaves(child, path + "/" + escaped)
    elif isinstance(value, list) and value:
        for index, child in enumerate(value):
            yield from leaves(child, path + "/" + str(index))
    else:
        yield path, value


def formula(value):
    return value.lstrip().startswith(("=", "+", "-", "@"))


def check(directory):
    archive = json.loads((directory / "report.json").read_text(encoding="utf-8"))
    assert archive["schema_version"] == "eeg-v1-export/1"
    assert "samples" not in archive["analysis"]["raw"]
    assert "samples" not in archive["analysis"]["processed"]
    rows = list(csv.DictReader(io.StringIO((directory / "report.csv").read_text(encoding="utf-8-sig"), newline="")))
    values = dict(leaves(archive))
    assert len(rows) == len(values)
    seen = set()
    for row in rows:
        path = row["path"]
        assert path not in seen
        seen.add(path)
        expected = values[path]
        assert row["schema_version"] == archive["schema_version"]
        assert row["subject_id"] == archive["subject"]["id"]
        assert row["recording_id"] == archive["analysis"]["processed"]["id"]
        if row["value_type"] == "string":
            text = row["value"]
            assert (row["spreadsheet_escaped"] == "true") == formula(expected)
            if row["spreadsheet_escaped"] == "true":
                assert text.startswith("'")
                text = text[1:]
            assert text == expected, path
        else:
            assert json.loads(row["value"]) == expected, path

    namespace = {"w": "http://schemas.openxmlformats.org/wordprocessingml/2006/main"}
    with __import__("zipfile").ZipFile(directory / "report.docx") as document:
        assert document.testzip() is None, "ZIP CRC 错误"
        for name in document.namelist():
            ET.fromstring(document.read(name))  # 所有 part 都是合法 XML
        root = ET.fromstring(document.read("word/document.xml"))
        paragraphs = []
        for paragraph in root.findall(".//w:p", namespace):
            parts = []
            for node in paragraph.iter():
                if node.tag == "{" + namespace["w"] + "}t":
                    parts.append(node.text or "")
                elif node.tag == "{" + namespace["w"] + "}br":
                    parts.append("\n")
            paragraphs.append("".join(parts))
        texts = "\n".join(paragraphs)
        for key in ("summary", "description", "interpretation"):
            assert archive["report"][key] in texts
        for limitation in archive["report"]["limitations"]:
            assert limitation in texts
        assert archive["analysis"]["raw"]["id"] in texts
        assert "Heading1" in document.read("word/styles.xml").decode("utf-8")

    if not shutil.which("pdfinfo") or not shutil.which("pdftotext"):
        raise RuntimeError("需要 Poppler pdfinfo 和 pdftotext 完成 PDF 独立校验")
    pdf = directory / "report.pdf"
    info = subprocess.check_output(["pdfinfo", str(pdf)], text=True)
    assert "Pages:" in info
    text = subprocess.check_output(["pdftotext", "-layout", str(pdf), "-"], text=True)
    normalize = lambda s: "".join(s.split())
    for key in ("summary", "description", "interpretation"):
        assert normalize(archive["report"][key]) in normalize(text), key
    for limitation in archive["report"]["limitations"]:
        assert normalize(limitation) in normalize(text)
    print(f"四格式校验通过：CSV {len(rows)} 行逐值匹配，DOCX ZIP/XML/正文正确，PDF 正文可提取")


if __name__ == "__main__":
    check(Path(sys.argv[1]))
