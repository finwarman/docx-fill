#[path = "common/spreadsheet.rs"]
mod common;
use common::*;
use docx_fill::{FillErrorId, FillOptions, SheetInstance, WorkbookTemplate};
use serde_json::json;

fn loop_rows() -> String {
    format!(r#"<row r="1">{}{}</row><row r="2">{}</row>"#, inline("A1", "{#items}{.}"), inline("B1", "{/items}"), inline("A2", "Tail"))
}

fn formula_row(formula: &str) -> String {
    format!(r#"<row r="1"><c r="A1"><f>{formula}</f><v>3</v></c></row>"#)
}

fn workbook(sheets: &[(&str, String)]) -> WorkbookTemplate {
    let mut parts = package("");
    let mut entries = String::new();
    let mut relationships = String::new();
    let mut types = String::new();
    for (index, (name, rows)) in sheets.iter().enumerate() {
        let id = index + 1;
        let path = format!("xl/worksheets/sheet{id}.xml");
        entries.push_str(&format!(r#"<sheet name="{name}" sheetId="{id}" r:id="s{id}"/>"#));
        relationships.push_str(&format!(r#"<Relationship Id="s{id}" Type="{REL}/worksheet" Target="worksheets/sheet{id}.xml"/>"#));
        types.push_str(&format!(r#"<Override PartName="/{path}" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#));
        let contents = format!(r#"<worksheet xmlns="{NS}"><sheetData>{rows}</sheetData></worksheet>"#).into_bytes();
        if index == 0 {
            parts[4].1 = contents;
        } else {
            parts.push((path, contents));
        }
    }
    replace(&mut parts, "xl/workbook.xml", format!(r#"<workbook xmlns="{NS}" xmlns:r="{REL}"><sheets>{entries}</sheets></workbook>"#));
    replace(&mut parts, "xl/_rels/workbook.xml.rels", format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{relationships}</Relationships>"#));
    replace(&mut parts, "[Content_Types].xml", format!(r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>{types}</Types>"#));
    WorkbookTemplate::parse(parts, &FillOptions::default()).unwrap()
}

#[test]
fn ignores_outside_3d_sheets() {
    let template = workbook(&[
        ("Before", loop_rows()),
        ("Jan", formula_row("SUM(Jan:Feb!A2)")),
        ("Feb", String::new()),
        ("After", loop_rows()),
    ]);
    let output = template.render(&json!({"items":[1,2]})).unwrap();
    assert!(xml(&output, "xl/worksheets/sheet2.xml").contains("SUM(Jan:Feb!A2)"));
    for path in ["xl/worksheets/sheet1.xml", "xl/worksheets/sheet4.xml"] {
        assert!(xml(&output, path).contains(r#"<c r="A3""#));
    }
}

#[test]
fn checks_middle_sheet_order() {
    let template = workbook(&[
        ("Zulu", formula_row("SUM(Zulu:Beta!A2)")),
        ("Alpha", loop_rows()),
        ("Beta", String::new()),
    ]);
    assert_eq!(template.render(&json!({"items":[1,2]})).unwrap_err().id, FillErrorId::SpreadsheetReference);
}

#[test]
fn transforms_uniform_3d_span() {
    for (formula, expected) in [
        ("SUM(Zulu:Beta!A2)", "SUM(Zulu:Beta!A3)"),
        ("SUM(Zulu:Zulu!A2)", "SUM(Zulu:Zulu!A3)"),
        ("SUM('Beta:Zulu'!$A$2)", "SUM('Beta:Zulu'!$A$3)"),
    ] {
        let template = workbook(&[
            ("Zulu", loop_rows()),
            ("Alpha", loop_rows()),
            ("Beta", loop_rows()),
            ("Formula", formula_row(formula)),
        ]);
        let output = template.render(&json!({"items":[1,2]})).unwrap();
        let formula = xml(&output, "xl/worksheets/sheet4.xml").replace("&apos;", "'");
        assert!(formula.contains(expected), "{formula}");
    }
}

#[test]
fn resolves_renamed_3d_endpoints() {
    let template = workbook(&[
        ("{first}", formula_row("SUM('{first}:{last}'!A2)+SUM('[Book.xlsx]{first}:{last}'!A2)+N(\"{first}:{last}!A2\")")),
        ("{last}", String::new()),
        ("Other", loop_rows()),
    ]);
    let output = template.render(&json!({"first":"Jan one","last":"Feb two","items":[1,2]})).unwrap();
    let formula = xml(&output, "xl/worksheets/sheet1.xml").replace("&apos;", "'");
    assert!(formula.contains("SUM('Jan one:Feb two'!A2)"), "{formula}");
    assert!(formula.contains("SUM('[Book.xlsx]{first}:{last}'!A2)"), "{formula}");
    assert!(formula.contains("N(&quot;{first}:{last}!A2&quot;)"), "{formula}");
}

#[test]
fn resolves_copied_3d_endpoints() {
    let mut template = workbook(&[
        ("Template", loop_rows()),
        ("Formula", formula_row("SUM(Jan:Feb!A2)")),
    ]);
    template.repeat_sheet("Template", vec![
        SheetInstance { name: "Jan".into(), data: json!({"items":[1,2]}) },
        SheetInstance { name: "Feb".into(), data: json!({"items":[3,4]}) },
    ]).unwrap();
    let output = template.render(&json!({"items":[0]})).unwrap();
    assert!(xml(&output, "xl/worksheets/sheet2.xml").contains("SUM(Jan:Feb!A3)"));
}

#[test]
fn leaves_external_3d_refs_alone() {
    for formula in [
        "SUM([Book.xlsx]Jan:Feb!A2)",
        "SUM('[Book.xlsx]Jan:Feb'!A2)",
        r#"SUM('C:\Reports\[Book.xlsx]Jan:Feb'!A2)"#,
        "SUM('https://example.com/[Book.xlsx]Jan:Feb'!A2)",
    ] {
        let template = workbook(&[("Other", loop_rows()), ("Formula", formula_row(formula))]);
        let output = template.render(&json!({"items":[1,2]})).unwrap();
        assert!(xml(&output, "xl/worksheets/sheet2.xml").contains(formula));
    }
}

#[test]
fn renames_unicode_3d_names() {
    let template = workbook(&[
        ("Ö_{first}", formula_row("SUM('ö_{first}:End'!A2)")),
        ("End", String::new()),
        ("Other", loop_rows()),
    ]);
    let output = template.render(&json!({"first":"Jan's","items":[1,2]})).unwrap();
    let formula = xml(&output, "xl/worksheets/sheet1.xml").replace("&apos;", "'");
    assert!(formula.contains("SUM('Ö_Jan''s:End'!A2)"), "{formula}");
}

#[test]
fn rejects_unavailable_endpoints() {
    let template = workbook(&[("Jan", formula_row("SUM(Jan:Missing!A2)")), ("Other", loop_rows())]);
    assert_eq!(template.render(&json!({"items":[1,2]})).unwrap_err().id, FillErrorId::SpreadsheetReference);
}
