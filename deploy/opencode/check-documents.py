"""Read-only acceptance of the synthetic invoice and spreadsheet produced by the local agent."""
import argparse
from decimal import Decimal
import hashlib
import json
from pathlib import Path
import re
import subprocess
import xml.etree.ElementTree as ET
import zipfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    root = args.directory.resolve(strict=True)
    fixture = json.loads(args.fixture.read_text())
    assert fixture["synthetic_demo"] is True
    net = sum(Decimal(str(item["hours"])) * Decimal(str(item["hourly_rate"])) for item in fixture["items"])
    tax = net * Decimal(str(fixture["illustrative_tax_rate"]))
    gross = net + tax
    files = {name: root / name for name in ("invoice.docx", "invoice.pdf", "ledger.xlsx", "ledger.html")}
    before = {name: hashlib.sha256(path.read_bytes()).hexdigest() for name, path in files.items()}
    with zipfile.ZipFile(files["invoice.docx"]) as archive:
        doc = ET.fromstring(archive.read("word/document.xml"))
        doc_text = " ".join(doc.itertext())
    pdf_text = subprocess.check_output(["pdftotext", str(files["invoice.pdf"]), "-"], text=True)
    pdf_info = subprocess.check_output(["pdfinfo", str(files["invoice.pdf"])], text=True)
    pages = int(re.search(r"^Pages:\s+(\d+)", pdf_info, re.M)[1])
    assert pages == 1, f"invoice must be one page, got {pages}"
    for text in (doc_text, pdf_text):
        compact = re.sub(r"\s+", " ", text)
        for required in [fixture["invoice_number"], fixture["customer"]["company"], fixture["customer"]["contact"],
                fixture["customer"]["email"], fixture["issuer"]["name"], "FICTIONAL DEMO DATA", "No payment is due"]:
            assert required in compact, "invoice missing: " + required
        for amount in (net, tax, gross):
            assert f"{amount:,.2f}" in compact or f"{amount:.2f}" in compact, "invoice amount missing"
    ns = {"x": "http://schemas.openxmlformats.org/spreadsheetml/2006/main"}
    with zipfile.ZipFile(files["ledger.xlsx"]) as archive:
        sheet = ET.fromstring(archive.read("xl/worksheets/sheet1.xml"))
        formulas = [(c.attrib["r"], c.findtext("x:f", namespaces=ns), c.findtext("x:v", namespaces=ns))
            for c in sheet.findall(".//x:c", ns) if c.find("x:f", ns) is not None]
        for amount in (net, tax, gross):
            assert any(Decimal(value) == amount for _, _, value in formulas if value is not None), "calculated total missing"
        strings = b" ".join(archive.read(name) for name in archive.namelist() if name.startswith("xl/") and name.endswith(".xml")).decode()
        assert fixture["customer"]["company"] in strings and fixture["customer"]["email"] in strings
    after = {name: hashlib.sha256(path.read_bytes()).hexdigest() for name, path in files.items()}
    assert before == after, "acceptance check changed output files"
    report = {"verified": True, "synthetic_data": True, "invoice_pages": pages,
        "net": str(net), "tax": str(tax), "gross": str(gross), "formula_cells": len(formulas),
        "checks_are_read_only": True, "sha256": before}
    args.report.write_text(json.dumps(report, indent=2))
    print(json.dumps(report))


if __name__ == "__main__":
    main()
