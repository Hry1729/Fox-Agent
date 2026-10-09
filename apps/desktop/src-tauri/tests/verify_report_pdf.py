"""Independent QA for Fox's vector reports; this is not a product runtime.

Requires pypdf and optionally a caller-supplied Poppler pdftoppm executable.
No business inputs or reference answer are built into this verifier.
"""
from __future__ import annotations

import argparse
import csv
import hashlib
import json
import subprocess
from pathlib import Path

from pypdf import PdfReader


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("pdf", type=Path)
    parser.add_argument("--expect-text", action="append", default=[])
    parser.add_argument("--metrics-csv", type=Path)
    parser.add_argument("--metric", action="append", default=[])
    parser.add_argument("--poppler", type=Path)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    args.output_dir.mkdir(parents=True, exist_ok=True)
    data = args.pdf.read_bytes()
    reader = PdfReader(args.pdf, strict=True)
    text = "\n".join(page.extract_text() or "" for page in reader.pages)
    checks = {
        "sha256": hashlib.sha256(data).hexdigest(),
        "bytes": len(data),
        "pages": len(reader.pages),
        "header": data.startswith(b"%PDF-"),
        "textLayer": bool(text.strip()),
        "rasterImages": sum(len(page.images) for page in reader.pages),
        "expectedText": {value: value in text for value in args.expect_text},
        "fontPrograms": [],
    }
    assert checks["header"] and 0 < checks["pages"] <= 24
    assert checks["textLayer"] and checks["rasterImages"] == 0
    assert all(checks["expectedText"].values()), checks["expectedText"]
    for page in reader.pages:
        assert abs(float(page.mediabox.width) - 595.28) < 1
        assert abs(float(page.mediabox.height) - 841.89) < 1
        for resource in page["/Resources"]["/Font"].get_object().values():
            font = resource.get_object()
            assert "/ToUnicode" in font, "Text must have an explicit Unicode map"
            descendant = font["/DescendantFonts"][0].get_object()
            descriptor = descendant["/FontDescriptor"].get_object()
            key = "/FontFile2" if "/FontFile2" in descriptor else "/FontFile3"
            program = descriptor[key].get_object().get_data()
            assert 0 < len(program) < 1024 * 1024, "Expected bounded subset font"
            checks["fontPrograms"].append({"type": key, "bytes": len(program)})
    if args.metrics_csv:
        rows = list(csv.DictReader(args.metrics_csv.read_text(encoding="utf-8").splitlines()))
        for metric in args.metric:
            name, expected = metric.split("=", 1)
            row = next(row for row in rows if row["metric"] == name)
            assert row["value"] == expected and expected in text
        checks["metricsChecked"] = args.metric
    (args.output_dir / "extracted_text.txt").write_text(text, encoding="utf-8")
    if args.poppler:
        subprocess.run([str(args.poppler), "-scale-to", "1200", "-png", str(args.pdf), str(args.output_dir / "page")], check=True)
        checks["renderedPages"] = len(list(args.output_dir.glob("page-*.png")))
        assert checks["renderedPages"] == checks["pages"]
    (args.output_dir / "validation.json").write_text(json.dumps(checks, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps({key: value for key, value in checks.items() if key != "expectedText"}))


if __name__ == "__main__":
    main()
