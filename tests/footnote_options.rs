mod common;

use std::collections::BTreeMap;
use common::document;
use docx_fill::{FillOptions, PackageTemplate};
use serde_json::json;

fn render(options: serde_json::Value, text: &str) -> String {
    let parts = [("word/document.xml".to_string(), document("<w:p><w:r><w:t>{:footnote note}</w:t></w:r></w:p>").into_bytes())];
    let options = FillOptions::default().read_json(&options).unwrap();
    let template = PackageTemplate::parse(parts, &options).unwrap();
    let parts: BTreeMap<_, _> = template.render(&json!({ "note": text })).into_iter().collect();
    String::from_utf8_lossy(&parts["word/footnotes.xml"]).into_owned()
}

#[test]
fn keeps_default_text_escaped() {
    let xml = render(json!({}), "A & <B>");

    assert!(xml.contains("<w:vertAlign w:val=\"superscript\"/>"));
    assert!(xml.contains("<w:t xml:space=\"preserve\"> A &amp; &lt;B&gt;</w:t>"));
}

#[test]
fn accepts_raw_word_runs() {
    let runs = "<w:r><w:rPr><w:b/></w:rPr><w:t>Bold</w:t></w:r>";
    let xml = render(json!({ "footnotes": { "raw": true } }), runs);

    assert!(xml.contains(runs));
    assert!(!xml.contains("&lt;w:r&gt;"));
}

#[test]
fn applies_note_properties() {
    let xml = render(json!({ "footnotes": {
        "refProperties": "<w:b/>",
        "refRun": "<w:r><w:t>. </w:t></w:r>",
        "paragraphProperties": "<w:spacing w:after=\"200\"/>",
        "runProperties": "<w:i/>"
    }}), "Text");

    assert!(xml.contains("<w:pPr><w:spacing w:after=\"200\"/></w:pPr>"));
    assert!(xml.contains("<w:rPr><w:b/></w:rPr><w:footnoteRef/>"));
    assert!(xml.contains("<w:r><w:t>. </w:t></w:r><w:r><w:rPr><w:i/></w:rPr><w:t xml:space=\"preserve\">Text</w:t>"));
}
