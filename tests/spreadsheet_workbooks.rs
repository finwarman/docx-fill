#[path = "common/spreadsheet.rs"]
mod common;
use common::*;
use docx_fill::{FillOptions, SheetInstance, SpreadsheetTagKind, WorkbookTemplate};
use serde_json::json;
fn read<'a>(p: &'a [(String, std::borrow::Cow<'_, [u8]>)], name: &str) -> &'a str {
    xml(p, name)
}
#[test]
fn copied_sheets_render_with_independent_data_and_parent_fallback() {
    let p = package(&format!(
        r#"<row r="1">{}</row>"#,
        inline("A1", "{name} {parent}")
    ));
    let mut t = WorkbookTemplate::parse(p, &FillOptions::default()).unwrap();
    t.repeat_sheet(
        "Data",
        vec![
            SheetInstance {
                name: "North".into(),
                data: json!({"name":"N"}),
            },
            SheetInstance {
                name: "South".into(),
                data: json!({"name":"S"}),
            },
        ],
    )
    .unwrap();
    let out = t.render(&json!({"name":"Root","parent":"P"})).unwrap();
    let wb = read(&out, "xl/workbook.xml");
    assert!(wb.contains("North"));
    assert!(wb.contains("South"));
    let texts: Vec<_> = out
        .iter()
        .filter(|(p, _)| p.contains("worksheets/") && p.ends_with(".xml"))
        .map(|(_, b)| std::str::from_utf8(b).unwrap())
        .collect();
    assert!(texts.iter().any(|v| v.contains("N P")));
    assert!(texts.iter().any(|v| v.contains("S P")));
}
#[test]
fn renders_sheet_name_tags_and_validates_duplicates() {
    let mut p = package("");
    let wb = String::from_utf8(p[2].1.clone())
        .unwrap()
        .replace("name=\"Data\"", "name=\"{name}\"");
    replace(&mut p, "xl/workbook.xml", wb);
    let t = WorkbookTemplate::parse(p, &FillOptions::default()).unwrap();
    assert!(
        read(
            &t.render(&json!({"name":"Annual"})).unwrap(),
            "xl/workbook.xml"
        )
        .contains("name=\"Annual\"")
    );
    assert!(t.render(&json!({"name":"A/B"})).is_err());
}
#[test]
fn exposes_nested_tag_names_and_cell_addresses() {
    let p = package(&format!(
        r#"<row r="2">{}{}</row>"#,
        inline("A2", "{#items}{name}"),
        inline("B2", "{/items}")
    ));
    let t = WorkbookTemplate::parse(p, &FillOptions::default()).unwrap();
    let tags = t.tags();
    assert_eq!(tags[0].name, "Data");
    assert_eq!(tags[0].tags[0].kind, SpreadsheetTagKind::Loop);
    assert_eq!(tags[0].tags[0].cell, "A2");
    assert_eq!(tags[0].tags[0].children[0].name, "name");
}

#[test]
fn aggregates_header_limit_errors_across_worksheets() {
    let mut p = package("");
    let workbook = String::from_utf8(p[2].1.clone())
        .unwrap()
        .replace(
            "</sheets>",
            r#"<sheet name="Notes" sheetId="2" r:id="s2"/></sheets>"#,
        );
    replace(&mut p, "xl/workbook.xml", workbook);
    let rels = String::from_utf8(p[3].1.clone())
        .unwrap()
        .replace(
            "</Relationships>",
            r#"<Relationship Id="s2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/notes.xml"/></Relationships>"#,
        );
    replace(&mut p, "xl/_rels/workbook.xml.rels", rels);
    let types = String::from_utf8(p[0].1.clone())
        .unwrap()
        .replace(
            "</Types>",
            r#"<Override PartName="/xl/worksheets/notes.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#,
        );
    replace(&mut p, "[Content_Types].xml", types);
    for path in ["xl/worksheets/sheet1.xml", "xl/worksheets/notes.xml"] {
        let sheet = format!(
            r#"<worksheet xmlns="{NS}"><sheetData/><headerFooter><oddHeader>{{header}}</oddHeader></headerFooter></worksheet>"#
        );
        if path.ends_with("notes.xml") {
            p.push((path.into(), sheet.into_bytes()));
        } else {
            replace(&mut p, path, sheet);
        }
    }

    let t = WorkbookTemplate::parse(p, &FillOptions::default()).unwrap();
    let error = t.render(&json!({"header": "x".repeat(256)})).unwrap_err();
    let parts: Vec<_> = error.diagnostics().into_iter()
        .filter_map(|diagnostic| diagnostic.part.as_deref()).collect();
    assert_eq!(parts.len(), 2);
    assert!(parts.contains(&"xl/worksheets/sheet1.xml"));
    assert!(parts.contains(&"xl/worksheets/notes.xml"));
}

#[test]
fn macro_enabled_workbook_preserves_vba_bytes() {
    let mut p = package(&format!(r#"<row r="1">{}</row>"#, inline("A1", "{v}")));
    let types = String::from_utf8(p[0].1.clone()).unwrap().replace(
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml",
        "application/vnd.ms-excel.sheet.macroEnabled.main+xml",
    );
    replace(&mut p, "[Content_Types].xml", types);
    p.push(("xl/vbaProject.bin".into(), vec![1, 2, 3, 0, 255]));
    let t = WorkbookTemplate::parse(p, &FillOptions::default()).unwrap();
    let out = t.render(&json!({"v":"OK"})).unwrap();
    assert_eq!(
        out.iter()
            .find(|(p, _)| p == "xl/vbaProject.bin")
            .unwrap()
            .1
            .as_ref(),
        [1, 2, 3, 0, 255]
    );
}
#[test]
fn strict_namespaces_fill_without_dropping_unrelated_parts() {
    let mut p = package(&format!(r#"<row r="1">{}</row>"#, inline("A1", "{v}")));
    for (_, b) in &mut p {
        if let Ok(s) = std::str::from_utf8(b) {
            *b = s
                .replace(
                    "http://schemas.openxmlformats.org/spreadsheetml/2006/main",
                    "http://purl.oclc.org/ooxml/spreadsheetml/main",
                )
                .replace(
                    "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
                    "http://purl.oclc.org/ooxml/officeDocument/relationships",
                )
                .into_bytes();
        }
    }
    let t = WorkbookTemplate::parse(p, &FillOptions::default()).unwrap();
    assert!(
        read(
            &t.render(&json!({"v":"Strict"})).unwrap(),
            "xl/worksheets/sheet1.xml"
        )
        .contains("Strict")
    );
}

#[test]
fn retain_sheets_prunes_removed_sheet_graph_and_remaps_view_names() {
    let mut p = package("");
    let workbook = String::from_utf8(p[2].1.clone())
        .unwrap()
        .replace(
            "<sheets>",
            r#"<bookViews><workbookView activeTab="1"/></bookViews><sheets>"#,
        )
        .replace(
            "</sheets>",
            r#"<sheet name="Notes" sheetId="2" r:id="s2"/></sheets><definedNames><definedName name="dataName" localSheetId="0">Data!$A$1</definedName><definedName name="notesName" localSheetId="1">Notes!$A$1</definedName></definedNames>"#,
        );
    replace(&mut p, "xl/workbook.xml", workbook);
    let rels = String::from_utf8(p[3].1.clone())
        .unwrap()
        .replace(
            "</Relationships>",
            r#"<Relationship Id="s2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/notes.xml"/></Relationships>"#,
        );
    replace(&mut p, "xl/_rels/workbook.xml.rels", rels);
    let types = String::from_utf8(p[0].1.clone())
        .unwrap()
        .replace(
            "</Types>",
            r#"<Override PartName="/xl/worksheets/notes.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#,
        );
    replace(&mut p, "[Content_Types].xml", types);
    p.push((
        "xl/worksheets/notes.xml".into(),
        format!(r#"<worksheet xmlns="{NS}"><sheetData/></worksheet>"#).into_bytes(),
    ));

    let mut template = WorkbookTemplate::parse(p, &FillOptions::default()).unwrap();
    template.retain_sheets(["Notes"]).unwrap();
    let out = template.render(&json!({})).unwrap();
    let workbook = read(&out, "xl/workbook.xml");
    assert!(!workbook.contains("name=\"Data\""));
    assert!(workbook.contains("activeTab=\"0\""));
    assert!(workbook.contains("name=\"notesName\" localSheetId=\"0\""));
    assert!(!workbook.contains("dataName"));
    assert!(
        out.iter()
            .any(|(path, _)| path == "xl/worksheets/notes.xml")
    );
    assert!(
        !out.iter()
            .any(|(path, _)| path == "xl/worksheets/sheet1.xml")
    );
    assert!(out.iter().any(|(path, _)| path == "xl/media/unchanged.bin"));
}

#[test]
fn workbook_structure_protection_blocks_sheet_edits() {
    let mut p = package("");
    let workbook = String::from_utf8(p[2].1.clone()).unwrap().replace(
        "<sheets>",
        "<workbookProtection lockStructure=\"1\"/><sheets>",
    );
    replace(&mut p, "xl/workbook.xml", workbook);
    let mut template = WorkbookTemplate::parse(p, &FillOptions::default()).unwrap();
    assert!(template.retain_sheets(["Data"]).is_err());
    assert!(
        template
            .repeat_sheet(
                "Data",
                vec![SheetInstance {
                    name: "Copy".into(),
                    data: json!({}),
                }],
            )
            .is_err()
    );
}

#[test]
fn unchanged_strict_parts_remain_identical_and_namespace_text_is_not_rewritten() {
    let strict = "http://purl.oclc.org/ooxml/spreadsheetml/main";
    let mut p = package("");
    for (_, bytes) in &mut p {
        if let Ok(text) = std::str::from_utf8(bytes) {
            *bytes = text
                .replace(NS, strict)
                .replace(
                    REL,
                    "http://purl.oclc.org/ooxml/officeDocument/relationships",
                )
                .into_bytes();
        }
    }
    let original = p.clone();
    let t = WorkbookTemplate::parse(p, &FillOptions::default()).unwrap();
    let out = t.render(&json!({})).unwrap();
    for ((a, b), (c, d)) in original.iter().zip(&out) {
        assert_eq!(a, c);
        assert_eq!(b.as_slice(), d.as_ref());
    }
    let mut p = package(&format!(r#"<row r="1">{}</row>"#, inline("A1", "{v}")));
    for (_, bytes) in &mut p {
        if let Ok(text) = std::str::from_utf8(bytes) {
            *bytes = text
                .replace(NS, strict)
                .replace(
                    REL,
                    "http://purl.oclc.org/ooxml/officeDocument/relationships",
                )
                .into_bytes();
        }
    }
    let t = WorkbookTemplate::parse(p, &FillOptions::default()).unwrap();
    let out = t.render(&json!({"v":strict})).unwrap();
    assert!(read(&out, "xl/worksheets/sheet1.xml").contains(&format!(">{strict}</")));
}
