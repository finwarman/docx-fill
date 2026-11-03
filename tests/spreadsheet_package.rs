#[path = "common/spreadsheet.rs"] mod common;
use common::*;
use docx_fill::{FillErrorId, FillOptions, WorkbookTemplate};
use serde_json::json;

#[test]
fn fills_a_real_workbook_package_and_borrows_opaque_parts() {
    let parts = package(&format!(r#"<row r="1">{}</row>"#, inline("A1", "Hello {name}")));
    let template = WorkbookTemplate::parse(parts, &FillOptions::default()).unwrap();
    assert_eq!(template.sheets()[0].name, "Data");
    let filled = template.render(&json!({"name":"A & B"})).unwrap();
    assert!(xml(&filled,"xl/worksheets/sheet1.xml").contains("Hello A &amp; B"));
    let (_, bytes) = filled.iter().find(|(path, _)| path == "xl/media/unchanged.bin").unwrap();
    assert!(matches!(bytes, std::borrow::Cow::Borrowed(_)));
    assert_eq!(&bytes[..], &[0,255,42,1]);
}

#[test]
fn no_tags_preserves_every_part_byte_for_byte() {
    let parts = package(r#"<row r="3"><c r="A3"><f>1+1</f><v>2</v></c></row>"#);
    let template = WorkbookTemplate::parse(parts.clone(), &FillOptions::default()).unwrap();
    let filled = template.render(&json!({})).unwrap();
    assert_eq!(parts.len(),filled.len());
    for ((a,b),(c,d)) in parts.iter().zip(filled.iter()) { assert_eq!(a,c); assert_eq!(b.as_slice(),d.as_ref()); }
}

#[test]
fn discovers_custom_part_paths_and_namespace_prefixes() {
    let mut parts = package(&format!(r#"<row r="1">{}</row>"#,inline("A1","{value}")));
    for (path, bytes) in &mut parts {
        *path = path.replace("xl/workbook.xml", "book/main.xml").replace("xl/_rels/workbook.xml.rels", "book/_rels/main.xml.rels").replace("xl/worksheets/sheet1.xml", "sheets/data.xml");
        if let Ok(text) = std::str::from_utf8(bytes) { *bytes = text.replace("xl/workbook.xml","book/main.xml").replace("xl/worksheets/sheet1.xml","sheets/data.xml").replace("worksheets/sheet1.xml","../sheets/data.xml").replace("xmlns:r=","xmlns:relationship=").replace("r:id=","relationship:id=").into_bytes(); }
    }
    let template = WorkbookTemplate::parse(parts,&FillOptions::default()).unwrap();
    assert_eq!(template.sheets()[0].part,"sheets/data.xml");
    assert!(xml(&template.render(&json!({"value":3})).unwrap(),"sheets/data.xml").contains("<v>3</v>"));
}

#[test]
fn refuses_duplicate_parts_unsafe_relationships_and_invalid_xml() {
    let mut duplicate=package(""); duplicate.push(duplicate[0].clone());
    assert_eq!(WorkbookTemplate::parse(duplicate,&FillOptions::default()).unwrap_err().id,FillErrorId::SpreadsheetPackage);
    let mut escaping=package("");
    replace(&mut escaping,"_rels/.rels",format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="office" Type="{REL}/officeDocument" Target="../../bad.xml"/></Relationships>"#));
    assert!(WorkbookTemplate::parse(escaping,&FillOptions::default()).is_err());
    let mut broken=package("");replace(&mut broken,"xl/worksheets/sheet1.xml","<worksheet>".into());
    assert!(WorkbookTemplate::parse(broken,&FillOptions::default()).is_err());
}

#[test]
fn clears_existing_formula_caches_and_removes_calculation_chain() {
    let mut parts=package(&format!(r#"<row r="1">{}<c r="B1"><f>A1+1</f><v>2</v></c>{}</row>"#,inline("A1","{value}"),inline("C1","{formula}")));
    let rels=String::from_utf8(parts.iter().find(|(p,_)|p=="xl/_rels/workbook.xml.rels").unwrap().1.clone()).unwrap().replace("</Relationships>",&format!(r#"<Relationship Id="calc" Type="{REL}/calcChain" Target="calcChain.xml"/></Relationships>"#));
    replace(&mut parts,"xl/_rels/workbook.xml.rels",rels);
    parts.push(("xl/calcChain.xml".into(),format!(r#"<calcChain xmlns="{NS}"><c r="B1" i="1"/></calcChain>"#).into_bytes()));
    let template=WorkbookTemplate::parse(parts,&FillOptions::default()).unwrap();
    let filled=template.render(&json!({"value":9,"formula":{"type":"formula","value":"A1+2","result":11}})).unwrap();
    let sheet=xml(&filled,"xl/worksheets/sheet1.xml");
    assert!(sheet.contains("<f>A1+1</f></c>"));
    assert!(sheet.contains("<v>11</v>"));
    assert!(!filled.iter().any(|(p,_)|p=="xl/calcChain.xml"));
    assert!(!xml(&filled,"xl/_rels/workbook.xml.rels").contains("calcChain"));
    assert!(xml(&filled,"xl/workbook.xml").contains("fullCalcOnLoad=\"1\""));
}
