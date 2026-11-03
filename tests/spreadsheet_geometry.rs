#[path = "common/spreadsheet.rs"]
mod common;
use common::*;
use docx_fill::{FillOptions, WorkbookTemplate};
use serde_json::json;
fn output(parts: Vec<(String, Vec<u8>)>, data: serde_json::Value) -> Vec<(String, Vec<u8>)> {
    WorkbookTemplate::parse(parts, &FillOptions::default())
        .unwrap()
        .render(&data)
        .unwrap()
        .into_iter()
        .map(|(p, b)| (p, b.into_owned()))
        .collect()
}
fn part<'a>(parts: &'a [(String, Vec<u8>)], path: &str) -> &'a str {
    std::str::from_utf8(&parts.iter().find(|(p, _)| p == path).unwrap().1).unwrap()
}
#[test]
fn horizontal_loop_repeats_cells_and_shifts_tail() {
    let rows = format!(
        r#"<row r="1">{}{}</row>"#,
        inline("A1", "{>items}{name}"),
        inline("B1", "Tail")
    );
    let out = output(
        package(&rows),
        json!({"items":[{"name":"One"},{"name":"Two"},{"name":"Three"}]}),
    );
    let s = part(&out, "xl/worksheets/sheet1.xml");
    assert!(s.contains("Three"));
    assert!(s.contains("r=\"D1\""), "{s}");
    assert!(!s.contains("{>"));
}
#[test]
fn horizontal_shift_updates_formula_references_to_static_tail() {
    let rows = format!(
        r#"<row r="1">{}<c r="B1"><v>5</v></c><c r="C1"><f>B1+1</f></c></row>"#,
        inline("A1", "{>items}{.}")
    );
    let out = output(package(&rows), json!({"items":[1,2,3]}));
    let s = part(&out, "xl/worksheets/sheet1.xml");
    assert!(s.contains("<c r=\"E1\"><f>D1+1</f>"), "{s}");
}
#[test]
fn row_expansion_updates_table_ranges_and_keeps_structure() {
    let rows = format!(
        r#"<row r="1">{}{}</row><row r="2">{}{}</row>"#,
        inline("A1", "Name"),
        inline("B1", "Value"),
        inline("A2", "{#items}{name}"),
        inline("B2", "{/items}")
    );
    let mut p = package(&rows);
    let s=String::from_utf8(p[4].1.clone()).unwrap().replace("</worksheet>",r#"<tableParts count="1"><tablePart xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" r:id="table"/></tableParts></worksheet>"#);
    replace(&mut p, "xl/worksheets/sheet1.xml", s);
    p.push(("xl/worksheets/_rels/sheet1.xml.rels".into(),format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="table" Type="{REL}/table" Target="../tables/table1.xml"/></Relationships>"#).into_bytes()));
    p.push(("xl/tables/table1.xml".into(),format!(r#"<table xmlns="{NS}" id="1" name="Data" displayName="Data" ref="A1:B2"><autoFilter ref="A1:B2"/><tableColumns count="2"><tableColumn id="1" name="Name"/><tableColumn id="2" name="Value"/></tableColumns></table>"#).into_bytes()));
    let out = output(p, json!({"items":[{"name":"A"},{"name":"B"},{"name":"C"}]}));
    let table = part(&out, "xl/tables/table1.xml");
    assert!(table.contains("ref=\"A1:B4\""), "{table}");
}
#[test]
fn cross_sheet_formulas_and_print_areas_follow_expanded_rows() {
    let rows = format!(
        r#"<row r="1">{}{}</row><row r="2">{}</row>"#,
        inline("A1", "{#items}{.}"),
        inline("B1", "{/items}"),
        inline("A2", "Tail")
    );
    let mut p = package(&rows);
    let wb=String::from_utf8(p[2].1.clone()).unwrap().replace("</sheets>",r#"<sheet name="Summary" sheetId="2" r:id="s2"/></sheets><definedNames><definedName name="_xlnm.Print_Area" localSheetId="0">'Data'!$A$1:$B$2</definedName></definedNames>"#);
    replace(&mut p, "xl/workbook.xml", wb);
    let rels=String::from_utf8(p[3].1.clone()).unwrap().replace("</Relationships>",&format!(r#"<Relationship Id="s2" Type="{REL}/worksheet" Target="worksheets/summary.xml"/></Relationships>"#));
    replace(&mut p, "xl/_rels/workbook.xml.rels", rels);
    let types=String::from_utf8(p[0].1.clone()).unwrap().replace("</Types>",r#"<Override PartName="/xl/worksheets/summary.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#);
    replace(&mut p, "[Content_Types].xml", types);
    p.push(("xl/worksheets/summary.xml".into(),format!(r#"<worksheet xmlns="{NS}"><sheetData><row r="1"><c r="A1"><f>Data!A2</f></c></row></sheetData></worksheet>"#).into_bytes()));
    let out = output(p, json!({"items":[1,2,3]}));
    let summary = part(&out, "xl/worksheets/summary.xml");
    assert!(summary.contains("Data!A4"), "{summary}");
    let wb = part(&out, "xl/workbook.xml");
    assert!(wb.contains("$B$4"), "{wb}");
}
