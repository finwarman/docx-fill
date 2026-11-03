#[path = "common/spreadsheet.rs"] mod common;
use common::*;
use docx_fill::{Delimiters, FillHooks, FillOptions, MissingValue, WorkbookTemplate};
use serde_json::{Value, json};

fn fill(text: &str, data: Value) -> String {
    let template=WorkbookTemplate::parse(package(&format!(r#"<row r="1">{}</row>"#,inline("A1",text))),&FillOptions::default()).unwrap();
    xml(&template.render(&data).unwrap(),"xl/worksheets/sheet1.xml").to_string()
}

#[test]
fn native_values_and_formula_strings_keep_their_types() {
    assert!(fill("{v}",json!({"v":12.5})).contains("<v>12.5</v>"));
    assert!(fill("{v}",json!({"v":false})).contains("t=\"b\"><v>0</v>"));
    assert!(fill("{v}",json!({"v":"=1+1"})).contains("=1+1</t>"));
    assert!(!fill("{v}",json!({"v":"=1+1"})).contains("<f>"));
    assert!(!fill("{v}",json!({"v":null})).contains("{v}"));
    assert!(fill("{v}",json!({"v":{"type":"formula","value":"1+1"}})).contains("<f>1+1</f>"));
}

#[test]
fn split_tags_and_prefixed_cells_fill_without_changing_cell_style() {
    let mut parts=package("");
    replace(&mut parts,"xl/worksheets/sheet1.xml",format!(r#"<s:worksheet xmlns:s="{NS}"><s:sheetData><s:row r="1"><s:c r="A1" s="3" t="inlineStr"><s:is><s:r><s:t>{{na</s:t></s:r><s:r><s:t>me}}</s:t></s:r></s:is></s:c></s:row></s:sheetData></s:worksheet>"#));
    let template=WorkbookTemplate::parse(parts,&FillOptions::default()).unwrap();
    let filled=template.render(&json!({"name":"A & B"})).unwrap();
    let sheet=xml(&filled,"xl/worksheets/sheet1.xml");
    assert!(sheet.contains("s=\"3\"")); assert!(sheet.contains("A &amp; B")); assert!(!sheet.contains("{na"));
}

#[test]
fn shared_strings_are_resolved_per_loop_item() {
    let rows=r#"<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row>"#;
    let mut parts=package(rows);
    let rels=String::from_utf8(parts[3].1.clone()).unwrap().replace("</Relationships>",&format!(r#"<Relationship Id="strings" Type="{REL}/sharedStrings" Target="sharedStrings.xml"/></Relationships>"#));
    replace(&mut parts,"xl/_rels/workbook.xml.rels",rels);
    parts.push(("xl/sharedStrings.xml".into(),format!(r#"<sst xmlns="{NS}" count="2" uniqueCount="2"><si><r><t>{{#items}}{{na</t></r><r><t>me}}</t></r></si><si><t>{{/items}}</t></si></sst>"#).into_bytes()));
    let template=WorkbookTemplate::parse(parts,&FillOptions::default()).unwrap();
    let filled=template.render(&json!({"items":[{"name":"One"},{"name":"Two"}]})).unwrap();
    let sheet=xml(&filled,"xl/worksheets/sheet1.xml");
    assert!(sheet.contains("One")); assert!(sheet.contains("Two")); assert!(!sheet.contains("{#items}"));
}

#[test]
fn missing_hook_runs_once_and_custom_delimiters_work() {
    struct Hook(usize);
    impl FillHooks for Hook {
        fn missing_value(&mut self,_part:&str,_tag:MissingValue<'_>)->Option<String> {self.0+=1;Some("missing".into())}
    }
    let options=FillOptions{delimiters:Delimiters{start:"[[".into(),end:"]]".into()},..FillOptions::default()};
    let template=WorkbookTemplate::parse(package(&format!(r#"<row r="1">{}</row>"#,inline("A1","[[v]]"))),&options).unwrap();
    let mut hook=Hook(0);
    let filled=template.render_with(&json!({}),&mut hook).unwrap();
    assert_eq!(hook.0,1); assert!(xml(&filled,"xl/worksheets/sheet1.xml").contains("missing"));
}

#[test]
fn unsupported_values_and_overlong_text_fail() {
    let template=WorkbookTemplate::parse(package(&format!(r#"<row r="1">{}</row>"#,inline("A1","{v}"))),&FillOptions::default()).unwrap();
    assert!(template.render(&json!({"v":{"unknown":3}})).is_err());
    assert!(template.render(&json!({"v":"x".repeat(32768)})).is_err());
    assert!(template.render(&json!({"v":{"type":"formula","value":""}})).is_err());
}

#[test]
fn literal_excel_escape_sequences_and_xml_characters_round_trip() {
    let sheet=fill("{v}",json!({"v":" _x0041_ & < > \n "}));
    assert!(sheet.contains("_x005F_x0041_"),"{sheet}");
    assert!(sheet.contains("xml:space=\"preserve\""),"{sheet}");
    assert!(sheet.contains("&amp; &lt; &gt;"));
}

#[test]
fn phonetic_runs_stay_out_of_inline_string_values() {
    let rows=r#"<row r="1"><c r="A1" t="inlineStr"><is><t>{v}</t><rPh sb="0" eb="3"><t>FURIGANA</t></rPh></is></c></row>"#;
    let template=WorkbookTemplate::parse(package(rows),&FillOptions::default()).unwrap();
    let sheet=xml(&template.render(&json!({"v":"Name"})).unwrap(),"xl/worksheets/sheet1.xml").to_string();
    assert!(sheet.contains("<t>Name</t>"),"{sheet}");
    assert!(!sheet.contains("FURIGANA"),"{sheet}");
}
