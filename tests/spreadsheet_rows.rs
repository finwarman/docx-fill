#[path = "common/spreadsheet.rs"] mod common;
use common::*;
use docx_fill::{FillOptions, WorkbookTemplate};
use serde_json::json;

fn fill(rows: &str, data: serde_json::Value) -> String {
    let template=WorkbookTemplate::parse(package(rows),&FillOptions::default()).unwrap();
    xml(&template.render(&data).unwrap(),"xl/worksheets/sheet1.xml").to_string()
}

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
