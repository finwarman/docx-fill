#!/usr/bin/env python3
"""Optional independent checks. Requires openpyxl; run from the repository root."""
from datetime import datetime
import json
from pathlib import Path
import subprocess
import tempfile
import zipfile
import xml.etree.ElementTree as ET
from openpyxl import Workbook, load_workbook
from openpyxl.chart import BarChart, Reference


def fill(folder, name, workbook, data):
    source = folder / f"{name}-template.xlsx"
    values = folder / f"{name}.json"
    output = folder / f"{name}-filled.xlsx"
    workbook.save(source)
    values.write_text(json.dumps(data))
    subprocess.run(["cargo", "run", "--quiet", "--example", "fill_xlsx", "--", str(source), str(values), str(output)], check=True)
    with zipfile.ZipFile(output) as archive:
        for name in archive.namelist():
            if name.endswith((".xml", ".rels", ".vml")):
                ET.fromstring(archive.read(name))
    return load_workbook(output)


with tempfile.TemporaryDirectory(prefix="docx-fill-xlsx-") as temp:
    folder = Path(temp)
    book = Workbook()
    sheet = book.active
    sheet.title = "Data"
    for column, value in enumerate(["{number}", "{date}", "{link}", "{~html}"], 1):
        sheet.cell(1, column, value)
    result = fill(folder, "types", book, {
        "number": {"type": "cell", "value": 12.5, "numberFormat": "0.00", "font": {"bold": True}},
        "date": {"type": "date", "value": "2024-02-29"},
        "link": {"type": "cell", "value": "Example", "hyperlink": "https://example.com", "comment": {"text": "Checked", "author": "Test"}},
        "html": "<b>Hello</b> <i>world</i>",
    })
    sheet = result["Data"]
    assert sheet["A1"].value == 12.5 and sheet["A1"].data_type == "n"
    assert sheet["A1"].font.bold and sheet["A1"].number_format == "0.00"
    assert sheet["B1"].value == datetime(2024, 2, 29)
    assert sheet["C1"].hyperlink.target == "https://example.com"
    assert sheet["C1"].comment.text == "Checked"
    assert sheet["D1"].value == "Hello world"

    book = Workbook()
    sheet = book.active
    sheet.title = "Sales"
    sheet["A1"] = "{$sales}"
    sheet.append(["Category", "Old values"])
    sheet.append(["Old", 0])
    chart = BarChart()
    chart.add_data(Reference(sheet, min_col=2, min_row=2, max_row=3), titles_from_data=True)
    chart.set_categories(Reference(sheet, min_col=1, min_row=3, max_row=3))
    sheet.add_chart(chart, "D2")
    result = fill(folder, "chart", book, {"sales": {"categories": ["A", "B"], "series": [{"title": "Revenue", "data": [2, 4]}]}})
    assert len(result["Sales"]._charts) == 1
    data_sheets = [sheet for sheet in result if sheet.title.startswith("DocxFillChart")]
    assert len(data_sheets) == 1 and data_sheets[0].sheet_state == "hidden"
    assert data_sheets[0]["B2"].value == 4

print("OpenPyXL checks passed: typed values, dates, styles, hyperlinks, notes, HTML, chart caches and data sheets.")
