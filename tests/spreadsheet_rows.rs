#[path = "common/spreadsheet.rs"] mod common;
use common::*;
use docx_fill::{FillError, FillErrorId, FillOptions, WorkbookTemplate};
use serde_json::json;

fn fill(rows: &str, data: serde_json::Value) -> String {
    let template=WorkbookTemplate::parse(package(rows),&FillOptions::default()).unwrap();
    xml(&template.render(&data).unwrap(),"xl/worksheets/sheet1.xml").to_string()
}

fn fill_parts(parts: Vec<(String, Vec<u8>)>, data: serde_json::Value) -> Result<String, FillError> {
    let template=WorkbookTemplate::parse(parts,&FillOptions::default())?;
    Ok(xml(&template.render(&data)?,"xl/worksheets/sheet1.xml").to_string())
}

fn edit(parts: &mut [(String, Vec<u8>)], name: &str, from: &str, to: &str) {
    let text=String::from_utf8(parts.iter().find(|(path,_)|path==name).unwrap().1.clone()).unwrap().replacen(from,to,1);
    replace(parts,name,text);
}

// Inserts worksheet elements before and after sheetData.
fn with_sheet(rows: &str, before: &str, after: &str) -> Vec<(String, Vec<u8>)> {
    let mut parts=package(rows);
    edit(&mut parts,"xl/worksheets/sheet1.xml","<sheetData>",&format!("{before}<sheetData>"));
    edit(&mut parts,"xl/worksheets/sheet1.xml","</worksheet>",&format!("{after}</worksheet>"));
    parts
}

const LOOP_ROW_2: &str = r#"<row r="1"><c r="A1" t="inlineStr"><is><t>Title</t></is></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>{#items}{.}</t></is></c><c r="B2" t="inlineStr"><is><t>{/items}</t></is></c></row>"#;
const COLOR_SCALE: &str = r#"<cfRule type="colorScale" priority="1"><colorScale><cfvo type="min"/><cfvo type="max"/><color rgb="FFFF0000"/><color rgb="FF00FF00"/></colorScale></cfRule>"#;

#[test]
fn row_loop_fills_shared_scope_and_shifts_footer_formula() {
    let rows=format!(r#"<row r="1">{}</row><row r="2" ht="24">{}{}{}</row><row r="3"><c r="C3"><f>SUM(B2:B2)</f><v>0</v></c></row>"#,inline("A1","Title"),inline("A2","{#items}{name}"),inline("B2","{amount}"),inline("C2","{/items}"));
    let sheet=fill(&rows,json!({"items":[{"name":"One","amount":2},{"name":"Two","amount":3}]}));
    assert!(sheet.contains("One")); assert!(sheet.contains("Two"));
    assert!(!sheet.contains("{#")); assert!(!sheet.contains("{/"));
    assert!(sheet.contains("<row r=\"4\""));
    assert!(sheet.contains("<f>SUM(B2:B3)</f>"),"{sheet}");
    assert_eq!(sheet.matches("ht=\"24\"").count(),2);
}

#[test]
fn empty_row_loop_removes_rows_and_moves_footer() {
    let rows=format!(r#"<row r="1">{}</row><row r="2">{}{}</row><row r="3">{}</row>"#,inline("A1","Title"),inline("A2","{#items}{name}"),inline("B2","{/items}"),inline("A3","Footer"));
    let sheet=fill(&rows,json!({"items":[]}));
    assert!(sheet.contains("<row r=\"2\"><c r=\"A2\""),"{sheet}");
    assert!(!sheet.contains("<row r=\"3\""));
    assert!(sheet.contains("Footer"));
}

#[test]
fn empty_first_loop_and_sparse_static_rows_keep_expected_positions() {
    let rows=format!(r#"<row r="1">{}{}</row><row r="2">{}</row><row r="5">{}</row>"#,inline("A1","{#items}"),inline("B1","{/items}"),inline("A2","Footer"),inline("A5","Sparse"));
    let sheet=fill(&rows,json!({"items":[]}));
    assert!(sheet.contains("<row r=\"1\""),"{sheet}");
    assert!(sheet.contains("<row r=\"4\""),"{sheet}");
}

#[test]
fn inline_loops_repeat_only_their_body() {
    let rows=format!(r#"<row r="1">{}</row>"#,inline("A1","Names: {#items}[{.}]{/items}."));
    assert!(fill(&rows,json!({"items":["A","B"]})).contains("Names: [A][B]."));
}

#[test]
fn loop_expressions_filters_and_missing_hooks_work() {
    let rows=format!(r#"<row r="1">{}{}</row>"#,inline("A1","{#items}{name} - {root} - {$index + 1}"),inline("B1","{/items}"));
    let template=WorkbookTemplate::parse(package(&rows),&FillOptions{expressions:true,..FillOptions::default()}).unwrap();
    let out=template.render(&json!({"root":"R","items":[{"name":"N"}]})).unwrap();
    assert!(xml(&out,"xl/worksheets/sheet1.xml").contains("N - R - 1"));
}

#[test]
fn structural_edits_with_drawings_fail_explicitly() {
    let mut parts=package(&format!(r#"<row r="1">{}{}</row>"#,inline("A1","{#items}"),inline("B1","{/items}")));
    let original=String::from_utf8(parts[4].1.clone()).unwrap().replace("</worksheet>","<drawing r:id=\"drawing\"/></worksheet>");
    replace(&mut parts,"xl/worksheets/sheet1.xml",original);
    let template=WorkbookTemplate::parse(parts,&FillOptions::default()).unwrap();
    assert!(template.render(&json!({"items":[1,2]})).is_err());
}

#[test]
fn native_values_next_to_loop_markers_and_single_quoted_attributes() {
    let rows=format!(r#"<row r = '1'>{}{}</row>"#,inline("A1","{#items}{amount}"),inline("B1","{/items}"));
    let sheet=fill(&rows,json!({"items":[{"amount":3},{"amount":4}]}));
    assert!(sheet.contains("r = '2'"),"{sheet}");
    assert!(sheet.contains("<v>3</v>")); assert!(sheet.contains("<v>4</v>"));
}

#[test]
fn blank_row_before_a_loop_is_kept_once() {
    let rows=format!(r#"<row r="1">{}</row><row r="3">{}{}</row><row r="4">{}</row>"#,inline("A1","Title"),inline("A3","{#items}{name}"),inline("B3","{/items}"),inline("A4","Footer"));
    let sheet=fill(&rows,json!({"items":[{"name":"One"},{"name":"Two"}]}));
    assert!(sheet.contains(r#"<row r="3"><c r="A3""#),"{sheet}");
    assert!(sheet.contains(r#"<row r="5"><c r="A5""#),"{sheet}");
}

#[test]
fn repeated_row_keeps_one_active_cell_and_lists_every_copy() {
    let rows=format!(r#"<row r="1">{}{}</row>"#,inline("A1","{#items}{.}"),inline("B1","{/items}"));
    let parts=with_sheet(&rows,r#"<sheetViews><sheetView workbookViewId="0"><selection activeCell="A1" sqref="A1"/></sheetView></sheetViews>"#,"");
    let sheet=fill_parts(parts,json!({"items":["a","b"]})).unwrap();
    assert!(sheet.contains(r#"<selection activeCell="A1" sqref="A1 A2"/>"#),"{sheet}");
}

#[test]
fn empty_loop_removes_emptied_formatting_and_moves_selection() {
    let parts=with_sheet(LOOP_ROW_2,r#"<sheetViews><sheetView workbookViewId="0"><selection activeCell="B2" sqref="B2"/></sheetView></sheetViews>"#,&format!(r#"<conditionalFormatting sqref="B2">{COLOR_SCALE}</conditionalFormatting>"#));
    let sheet=fill_parts(parts,json!({"items":[]})).unwrap();
    assert!(sheet.contains(r#"<selection activeCell="A1" sqref="A1"/>"#),"{sheet}");
    assert!(!sheet.contains("conditionalFormatting"),"{sheet}");
    assert!(!sheet.contains("cfRule"),"{sheet}");
}

#[test]
fn empty_loop_drops_a_formatting_range_on_its_row() {
    let parts=with_sheet(LOOP_ROW_2,"",&format!(r#"<conditionalFormatting sqref="B2:C2 A1">{COLOR_SCALE}</conditionalFormatting>"#));
    let sheet=fill_parts(parts,json!({"items":[]})).unwrap();
    assert!(sheet.contains(r#"<conditionalFormatting sqref="A1">"#),"{sheet}");
}

#[test]
fn empty_loop_shrinks_a_footer_range() {
    let rows=format!(r#"{LOOP_ROW_2}<row r="3"><c r="B3"><f>SUM(B1:B2)+SUM(B2:B2)</f><v>0</v></c></row>"#);
    let sheet=fill_parts(package(&rows),json!({"items":[]})).unwrap();
    assert!(sheet.contains("<f>SUM(B1:B1)+SUM(#REF!)</f>"),"{sheet}");
}

#[test]
fn exclamation_marks_in_strings_are_not_sheet_references() {
    let rows=format!(r#"{LOOP_ROW_2}<row r="3"><c r="B3"><f>"Hi!"&amp;A1</f></c></row>"#);
    let sheet=fill_parts(package(&rows),json!({"items":[1,2]})).unwrap();
    assert!(sheet.contains(r#"<f>"Hi!"&amp;A1</f>"#),"{sheet}");
}

#[test]
fn whole_row_references_grow_and_whole_column_references_stay() {
    let rows=format!(r#"<row r="1">{}{}</row><row r="2"><c r="A2"><f>SUM(1:1)+SUM($1:$1)+SUM(C:C)</f></c></row>"#,inline("A1","{#items}{.}"),inline("B1","{/items}"));
    let sheet=fill(&rows,json!({"items":[1,2]}));
    assert!(sheet.contains("<f>SUM(1:2)+SUM($1:$2)+SUM(C:C)</f>"),"{sheet}");
}

#[test]
fn structural_edits_with_chart_sheets_fail_explicitly() {
    let rows=format!(r#"<row r="1">{}{}</row>"#,inline("A1","{#items}{.}"),inline("B1","{/items}"));
    let mut parts=package(&rows);
    edit(&mut parts,"[Content_Types].xml","</Types>",r#"<Override PartName="/xl/chartsheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.chartsheet+xml"/></Types>"#);
    edit(&mut parts,"xl/workbook.xml","</sheets>",r#"<sheet name="Chart" sheetId="2" r:id="s2"/></sheets>"#);
    edit(&mut parts,"xl/_rels/workbook.xml.rels","</Relationships>",&format!(r#"<Relationship Id="s2" Type="{REL}/chartsheet" Target="chartsheets/sheet2.xml"/></Relationships>"#));
    parts.push(("xl/chartsheets/sheet2.xml".into(),format!(r#"<chartsheet xmlns="{NS}" xmlns:r="{REL}"><drawing r:id="d1"/></chartsheet>"#).into_bytes()));
    parts.push(("xl/charts/chart1.xml".into(),br#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:val><c:numRef><c:f>Data!$A$1:$A$1</c:f></c:numRef></c:val></c:chartSpace>"#.to_vec()));
    assert_eq!(fill_parts(parts,json!({"items":[1,2]})).unwrap_err().id,FillErrorId::SpreadsheetReference);
}

#[test]
fn structural_edits_with_pivot_caches_fail_explicitly() {
    let rows=format!(r#"<row r="1">{}{}</row>"#,inline("A1","{#items}{.}"),inline("B1","{/items}"));
    let mut parts=package(&rows);
    edit(&mut parts,"xl/workbook.xml","</sheets>",r#"</sheets><pivotCaches><pivotCache cacheId="1" r:id="p1"/></pivotCaches>"#);
    assert_eq!(fill_parts(parts.clone(),json!({"items":[1,2]})).unwrap_err().id,FillErrorId::SpreadsheetReference);
    assert!(fill_parts(parts,json!({"items":[1]})).is_ok());
}

#[test]
fn large_loop_with_formulas_renders_in_linear_time() {
    let rows=format!(r#"<row r="1">{}<c r="B1"><f>A1*2</f><v>0</v></c>{}</row>"#,inline("A1","{#items}{.}"),inline("C1","{/items}"));
    let template=WorkbookTemplate::parse(package(&rows),&FillOptions::default()).unwrap();
    let items:Vec<_>=(0..16_000).collect();
    let started=std::time::Instant::now();
    template.render(&json!({"items":items})).unwrap();
    assert!(started.elapsed().as_secs()<5,"{:?}",started.elapsed());
}

#[test]
fn single_quoted_cell_references_are_renumbered() {
    let rows=r#"<row r="1"><c r='A1' t="inlineStr"><is><t>{#items}{.}</t></is></c><c r='B1' t="inlineStr"><is><t>{/items}</t></is></c></row>"#;
    let sheet=fill(rows,json!({"items":["a","b"]}));
    assert!(sheet.contains("<c r='A2'"),"{sheet}");
}

#[test]
fn merged_range_reduced_to_one_cell_is_dropped() {
    let rows=format!(r#"{LOOP_ROW_2}<row r="3">{}</row>"#,inline("A3","Footer"));
    let parts=with_sheet(&rows,"",r#"<mergeCells count="1"><mergeCell ref="A2:A3"/></mergeCells>"#);
    let sheet=fill_parts(parts,json!({"items":[]})).unwrap();
    assert!(!sheet.contains("<mergeCell "),"{sheet}");
}
