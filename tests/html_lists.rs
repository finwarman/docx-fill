mod common;

use common::{document, paragraph_texts};
use docx_fill::{FillOptions, IncludeMode, IncludeStyles, PackageTemplate, Subtemplate, fill_part};
use serde_json::json;

const TAG: &str = "<w:p><w:r><w:t>{~~html}</w:t></w:r></w:p>";
const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn render(html: &str, extra: &[(&str, String)]) -> Vec<(String, String)> {
    let mut parts = vec![
        ("word/document.xml".to_string(), document(TAG).into_bytes()),
        ("[Content_Types].xml".to_string(), b"<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"xml\" ContentType=\"application/xml\"/></Types>".to_vec()),
    ];
    parts.extend(extra.iter().map(|(name, xml)| (name.to_string(), xml.as_bytes().to_vec())));
    let template = PackageTemplate::parse(parts, &FillOptions::default()).unwrap();
    template.render(&json!({"html": html})).into_iter().map(|(name, bytes)| (name, String::from_utf8_lossy(&bytes).into_owned())).collect()
}

fn part<'a>(parts: &'a [(String, String)], name: &str) -> &'a str {
    &parts.iter().find(|(path, _)| path == name).unwrap_or_else(|| panic!("missing {name}")).1
}

fn ids(xml: &str) -> Vec<String> {
    xml.split("<w:numId w:val=\"").skip(1).map(|value| value[..value.find('"').unwrap()].to_string()).collect()
}

#[test]
fn writes_native_bullet_lists() {
    let parts = render("<ul><li>A</li><li>B</li></ul>", &[]);
    let body = part(&parts, "word/document.xml");
    let numbering = part(&parts, "word/numbering.xml");

    assert_eq!(paragraph_texts(body), ["A", "B"]);
    let references = ids(body);
    assert_eq!(references.len(), 2);
    assert_eq!(references[0], references[1]);
    assert!(numbering.contains("<w:numFmt w:val=\"bullet\"/>"));
    assert!(part(&parts, "word/_rels/document.xml.rels").contains(&format!("Type=\"{R}/numbering\"")));
    assert!(part(&parts, "[Content_Types].xml").contains("/word/numbering.xml"));
}

#[test]
fn preserves_nested_restarts() {
    let parts = render("<ol start=\"3\"><li>A<ul><li>B</li></ul></li><li value=\"10\">C</li><li>D</li></ol>", &[]);
    let body = part(&parts, "word/document.xml");
    let numbering = part(&parts, "word/numbering.xml");
    let references = ids(body);

    assert_eq!(paragraph_texts(body), ["A", "B", "C", "D"]);
    assert_eq!(references.len(), 4);
    assert_ne!(references[0], references[1]);
    assert_ne!(references[0], references[2]);
    assert_eq!(references[2], references[3]);
    assert!(body.contains("<w:ilvl w:val=\"1\"/>"));
    assert!(numbering.contains("<w:start w:val=\"3\"/>") && numbering.contains("<w:start w:val=\"10\"/>"));
}

#[test]
fn preserves_existing_numbering() {
    let numbering = format!("<w:numbering xmlns:w=\"{W}\"><w:abstractNum w:abstractNumId=\"0\"><w:multiLevelType w:val=\"singleLevel\"/></w:abstractNum><w:num w:numId=\"1\"><w:abstractNumId w:val=\"0\"/></w:num></w:numbering>");
    let rels = format!("<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"numbering\" Type=\"{R}/numbering\" Target=\"lists.xml\"/></Relationships>");
    let parts = render("<ol><li>A</li></ol>", &[("word/lists.xml", numbering.clone()), ("word/_rels/document.xml.rels", rels)]);
    let lists = part(&parts, "word/lists.xml");

    assert!(lists.contains("<w:num w:numId=\"1\"><w:abstractNumId w:val=\"0\"/></w:num>"));
    assert_ne!(ids(part(&parts, "word/document.xml")), ["1"]);
    assert_eq!(lists.matches("<w:abstractNum w:abstractNumId=\"0\"").count(), 1);
    assert_eq!(parts.iter().filter(|(name, _)| name == "word/numbering.xml").count(), 0);
}

#[test]
fn keeps_standalone_markers() {
    let xml = fill_part(&document(TAG), &json!({"html": "<ol><li>A</li><li>B</li></ol>"}), &FillOptions::default()).unwrap();

    assert_eq!(paragraph_texts(&xml), ["1.A", "2.B"]);
}

#[test]
fn skips_numbering_without_lists() {
    let parts = render("<p>A</p>", &[]);

    assert!(!parts.iter().any(|(name, _)| name == "word/numbering.xml"));
}

#[test]
fn fills_an_empty_numbering_root() {
    let parts = render("<ol><li>A</li></ol>", &[("word/numbering.xml", format!("<w:numbering xmlns:w=\"{W}\"/>"))]);
    let xml = part(&parts, "word/numbering.xml");

    assert!(xml.contains("<w:abstractNum ") && xml.ends_with("</w:numbering>"));
}

#[test]
fn shares_header_definitions() {
    let header = format!("<w:hdr xmlns:w=\"{W}\">{TAG}</w:hdr>");
    let parts = render("<ul><li>A</li></ul>", &[("word/header1.xml", header)]);
    let body_ids = ids(part(&parts, "word/document.xml"));
    let header_ids = ids(part(&parts, "word/header1.xml"));
    let numbers = part(&parts, "word/numbering.xml");

    assert_ne!(body_ids, header_ids);
    for id in body_ids.iter().chain(&header_ids) {
        assert!(numbers.contains(&format!("<w:num w:numId=\"{id}\">")));
    }
}

#[test]
fn avoids_relationship_id_clash() {
    let rels = format!("<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rIdDocxFillNumbering\" Type=\"{R}/hyperlink\" Target=\"https://example.com\" TargetMode=\"External\"/></Relationships>");
    let parts = render("<ul><li>A</li></ul>", &[("word/_rels/document.xml.rels", rels)]);
    let rels = part(&parts, "word/_rels/document.xml.rels");

    assert!(rels.contains("Id=\"rIdDocxFillNumbering\" Type=") && rels.contains("Target=\"https://example.com\""));
    assert!(rels.contains(&format!("Type=\"{R}/numbering\"")));
    assert_eq!(rels.matches("Id=\"rIdDocxFillNumbering\"").count(), 1);
}

#[test]
fn keeps_numbering_schema_order() {
    let existing = format!("<w:numbering xmlns:w=\"{W}\"><w:abstractNum w:abstractNumId=\"5\"/><w:num w:numId=\"5\"><w:abstractNumId w:val=\"5\"/></w:num><w:numIdMacAtCleanup w:val=\"5\"/></w:numbering>");
    let parts = render("<ol type=\"A\"><li>A</li></ol>", &[("word/numbering.xml", existing)]);
    let xml = part(&parts, "word/numbering.xml");

    assert!(xml.contains("<w:numFmt w:val=\"upperLetter\"/>"));
    assert!(xml.rfind("<w:abstractNum ").unwrap() < xml.find("<w:num ").unwrap());
    assert!(xml.rfind("<w:num ").unwrap() < xml.find("<w:numIdMacAtCleanup").unwrap());
}

#[test]
fn imports_subtemplate_lists() {
    let options = FillOptions::default();
    let body = format!("{TAG}<w:p><w:r><w:t>{{:include doc}}</w:t></w:r></w:p>");
    let mut template = PackageTemplate::parse([("word/document.xml".into(), document(&body).into_bytes())], &options).unwrap();
    let sub = Subtemplate::parse([("word/document.xml".into(), document(TAG).into_bytes())], &options, IncludeMode::Templated, IncludeStyles::UseMain).unwrap();
    template.add_include("part", sub);
    let parts: Vec<_> = template.render(&json!({"html":"<ul><li>A</li></ul>","doc":"part"})).into_iter().map(|(name, bytes)| (name, String::from_utf8_lossy(&bytes).into_owned())).collect();
    let body = part(&parts, "word/document.xml");
    let numbers = part(&parts, "word/numbering.xml");

    assert_eq!(paragraph_texts(body), ["A", "A"]);
    for id in ids(body) {
        assert!(numbers.contains(&format!("<w:num w:numId=\"{id}\">")));
    }
}
