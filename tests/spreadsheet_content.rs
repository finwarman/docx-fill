#[path = "common/spreadsheet.rs"]
mod common;
use common::*;
use docx_fill::{FillOptions, WorkbookTemplate};
use serde_json::{Value, json};
fn fill(value: Value) -> Vec<(String, Vec<u8>)> {
    let parts = package(&format!(r#"<row r="1">{}</row>"#, inline("A1", "{v}")));
    WorkbookTemplate::parse(parts, &FillOptions::default())
        .unwrap()
        .render(&json!({"v":value}))
        .unwrap()
        .into_iter()
        .map(|(p, b)| (p, b.into_owned()))
        .collect()
}
fn part<'a>(parts: &'a [(String, Vec<u8>)], name: &str) -> &'a str {
    std::str::from_utf8(&parts.iter().find(|(p, _)| p == name).unwrap().1).unwrap()
}
#[test]
fn date_system_and_time_fraction_use_native_numeric_cells() {
    let out = fill(json!({"type":"date","value":"1900-03-01"}));
    assert!(
        part(&out, "xl/worksheets/sheet1.xml").contains("<v>61.0</v>")
            || part(&out, "xl/worksheets/sheet1.xml").contains("<v>61</v>")
    );
    let mut parts = package(&format!(r#"<row r="1">{}</row>"#, inline("A1", "{v}")));
    let wb = String::from_utf8(parts[2].1.clone())
        .unwrap()
        .replace("<sheets>", "<workbookPr date1904=\"1\"/><sheets>");
    replace(&mut parts, "xl/workbook.xml", wb);
    let template = WorkbookTemplate::parse(parts, &FillOptions::default()).unwrap();
    let out = template
        .render(&json!({"v":{"type":"date","value":"1904-01-01"}}))
        .unwrap();
    assert!(xml(&out, "xl/worksheets/sheet1.xml").contains("<v>0"));
}
#[test]
fn style_hyperlink_and_note_parts_are_valid_and_attached() {
    let out = fill(
        json!({"type":"cell","value":"Example","numberFormat":"@","font":{"bold":true,"color":"FF0000"},"hyperlink":"https://example.com","comment":{"text":"A note","author":"Author"}}),
    );
    let sheet = part(&out, "xl/worksheets/sheet1.xml");
    assert!(sheet.contains("hyperlink"));
    assert!(sheet.contains("xmlns:r="));
    assert!(sheet.contains("legacyDrawing"));
    let styles = out
        .iter()
        .find(|(p, _)| p.contains("styles") && p.ends_with(".xml"))
        .unwrap();
    assert!(std::str::from_utf8(&styles.1).unwrap().contains("<b/>"));
    let notes = out
        .iter()
        .find(|(p, _)| p.contains("comments") && p.ends_with(".xml"))
        .unwrap();
    let notes = std::str::from_utf8(&notes.1).unwrap();
    assert!(
        notes.contains("<authors><author>Author</author></authors>"),
        "{notes}"
    );
    assert!(notes.contains("ref=\"A1\""));
}
#[test]
fn rich_text_descriptor_preserves_different_run_styles() {
    let out = fill(
        json!({"type":"richText","runs":[{"text":"Bold","bold":true},{"text":" plain","italic":true}]}),
    );
    let s = part(&out, "xl/worksheets/sheet1.xml");
    assert!(s.contains("<b/>"));
    assert!(s.contains("<i/>"));
    assert!(s.contains(" plain"));
}
#[test]
fn html_maps_basic_formatting_to_spreadsheet_runs() {
    let p = package(&format!(r#"<row r="1">{}</row>"#, inline("A1", "{~h}")));
    let t = WorkbookTemplate::parse(p, &FillOptions::default()).unwrap();
    let out = t
        .render(&json!({"h":"<b>Hello</b> <i>world</i><br/>Line"}))
        .unwrap();
    let s = xml(&out, "xl/worksheets/sheet1.xml");
    assert!(s.contains("<b/>"), "{s}");
    assert!(s.contains("<i/>"), "{s}");
    assert!(s.contains("Line"));
}
#[test]
fn invalid_dates_and_unsupported_cell_descriptors_are_errors() {
    let p = package(&format!(r#"<row r="1">{}</row>"#, inline("A1", "{v}")));
    let t = WorkbookTemplate::parse(p, &FillOptions::default()).unwrap();
    for v in [
        json!({"type":"date","value":"1900-02-29"}),
        json!({"type":"time","value":"25:00:00"}),
        json!({"type":"date","value":"2024-02-30"}),
    ] {
        assert!(t.render(&json!({"v":v})).is_err());
    }
}

#[test]
fn html_span_styles_and_links_are_native_cell_features() {
    let p = package(&format!(r#"<row r="1">{}</row>"#, inline("A1", "{~h}")));
    let t = WorkbookTemplate::parse(p, &FillOptions::default()).unwrap();
    let out=t.render(&json!({"h":"<p><span style=\"font-weight:bold;color:#f00;font-size:14px\">Styled</span> <a href=\"https://example.com?a=1&amp;b=2\">link</a></p>"})).unwrap();
    let s = xml(&out, "xl/worksheets/sheet1.xml");
    assert!(s.contains("<b/>"), "{s}");
    assert!(s.contains("FFFF0000"), "{s}");
    assert!(s.contains("<hyperlink"), "{s}");
    let rel = xml(&out, "xl/worksheets/_rels/sheet1.xml.rels");
    assert!(rel.contains("a=1&amp;b=2"), "{rel}");
}
#[test]
fn validated_raw_runs_allow_rich_text_and_reject_other_xml() {
    let p = package(&format!(r#"<row r="1">{}</row>"#, inline("A1", "{v}")));
    let t = WorkbookTemplate::parse(p, &FillOptions::default()).unwrap();
    let out = t
        .render(&json!({"v":{"type":"richText","raw":"<r><rPr><b/></rPr><t>Raw</t></r>"}}))
        .unwrap();
    assert!(xml(&out, "xl/worksheets/sheet1.xml").contains("<b/>"));
    for raw in [
        "<script>bad</script>",
        "<r><t>&unknown;</t></r>",
        "<r><t>x</r>",
    ] {
        assert!(
            t.render(&json!({"v":{"type":"richText","raw":raw}}))
                .is_err()
        );
    }
}
#[test]
fn scalar_formats_and_shared_styles_are_deduplicated() {
    let p = package(&format!(
        r#"<row r="1">{}{}</row>"#,
        inline("A1", "{a}"),
        inline("B1", "{b}")
    ));
    let t = WorkbookTemplate::parse(p, &FillOptions::default()).unwrap();
    let out=t.render(&json!({"a":{"type":"currency","value":4.5,"fmt":"0.000"},"b":{"type":"currency","value":2,"fmt":"0.000"}})).unwrap();
    let styles = out
        .iter()
        .find(|(p, _)| p.contains("styles") && p.ends_with(".xml"))
        .unwrap();
    let s = std::str::from_utf8(&styles.1).unwrap();
    assert_eq!(s.matches("formatCode=\"0.000\"").count(), 1, "{s}");
}

#[test]
fn row_height_estimation_is_opt_in_and_preserves_explicit_heights() {
    let p = package(&format!(
        r#"<row r="1" ht="20" customHeight="1">{}</row>"#,
        inline("A1", "{v}")
    ));
    let mut t = WorkbookTemplate::parse(p, &FillOptions::default()).unwrap();
    let out = t.render(&json!({"v":"one\ntwo\nthree"})).unwrap();
    assert!(xml(&out, "xl/worksheets/sheet1.xml").contains("ht=\"20\""));
    t.set_row_height_mode(docx_fill::RowHeightMode::Estimate);
    let out = t.render(&json!({"v":"one\ntwo\nthree"})).unwrap();
    assert!(xml(&out, "xl/worksheets/sheet1.xml").contains("ht=\"45\""));
    let out = t.render(&json!({"v":{
        "type":"cell", "value":"x".repeat(25),
        "alignment":{"wrapText":true}
    }})).unwrap();
    assert!(xml(&out, "xl/worksheets/sheet1.xml").contains("ht=\"45\""));
}

#[test]
fn unsafe_numeric_precision_and_invalid_raw_properties_are_rejected() {
    let p = package(&format!(r#"<row r="1">{}</row>"#, inline("A1", "{v}")));
    let t = WorkbookTemplate::parse(p, &FillOptions::default()).unwrap();
    assert!(t.render(&json!({"v":1234567890123456u64})).is_err());
    for raw in [
        "<r><rPr><sz val=\"bad\"/></rPr><t>x</t></r>",
        "<r><rPr><color rgb=\"wrong\"/></rPr><t>x</t></r>",
    ] {
        assert!(
            t.render(&json!({"v":{"type":"richText","raw":raw}}))
                .is_err()
        );
    }
}
#[test]
fn existing_notes_move_with_inserted_rows() {
    let rows = format!(
        r#"<row r="1">{}</row><row r="2">{}</row>"#,
        inline("A1", "Top"),
        inline("A2", "{v}")
    );
    let t = WorkbookTemplate::parse(package(&rows), &FillOptions::default()).unwrap();
    let filled=t.render(&json!({"v":{"type":"cell","value":"Noted","comment":{"text":"Existing","author":"A"}}})).unwrap();
    let mut parts: Vec<_> = filled
        .into_iter()
        .map(|(p, b)| (p, b.into_owned()))
        .collect();
    let sheet = parts
        .iter_mut()
        .find(|(p, _)| p == "xl/worksheets/sheet1.xml")
        .unwrap();
    let mut source = String::from_utf8(sheet.1.clone()).unwrap();
    let start = source.find("<row r=\"1\"").unwrap();
    let end = start + source[start..].find("</row>").unwrap() + 6;
    source.replace_range(
        start..end,
        &format!(
            r#"<row r="1">{}{}</row>"#,
            inline("A1", "{#items}{.}"),
            inline("B1", "{/items}")
        ),
    );
    sheet.1 = source.into_bytes();
    let t = WorkbookTemplate::parse(parts, &FillOptions::default()).unwrap();
    let out = t.render(&json!({"items":[1,2,3]})).unwrap();
    let notes = out
        .iter()
        .find(|(p, _)| p.contains("comments") && p.ends_with(".xml"))
        .unwrap();
    assert!(
        std::str::from_utf8(&notes.1)
            .unwrap()
            .contains("ref=\"A4\"")
    );
}
