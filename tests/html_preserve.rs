mod common;

use common::{document, elements, paragraph, paragraph_texts, text};
use docx_fill::{FillOptions, Template};
use serde_json::{Value, json};

fn render(body: &str, data: Value) -> String {
    let options = FillOptions::default().read_json(&json!({"html":{"onInvalidBlock":"preserve"}})).unwrap();
    let xml = Template::parse(&document(body), &options).unwrap().try_render(&data).unwrap();
    Template::parse(&xml, &FillOptions::core()).expect("balanced output XML");
    let mut depth = 0;
    for tag in xml.split('<').skip(1) {
        if tag.starts_with("/w:p>") { depth -= 1; }
        else if tag.starts_with("w:p>") || tag.starts_with("w:p ") { depth += 1; }
        assert!((0..=1).contains(&depth), "nested paragraphs: {xml}");
    }
    assert_eq!(depth, 0);
    xml
}

#[test]
fn keeps_text_around_blocks() {
    let xml = render(&paragraph(&["prefix {~html} suffix"]), json!({"html":"<p>A</p><p>B</p>"}));
    assert_eq!(paragraph_texts(&xml), ["prefix ", "A", "B", " suffix"]);
}

#[test]
fn preserves_block_placeholders() {
    let xml = render(&paragraph(&["prefix {~~html} suffix"]), json!({"html":"<b>body</b>"}));
    assert_eq!(paragraph_texts(&xml), ["prefix ", "body", " suffix"]);
    assert!(elements(&xml, "w:p")[1].contains("<w:b/>"));
}

#[test]
fn inserts_tables_between_text() {
    let xml = render(&paragraph(&["before{~html}after"]), json!({"html":"<table><tr><td>cell</td></tr></table>"}));
    assert!(xml.contains("<w:tbl>"));
    assert_eq!(paragraph_texts(&xml), ["before", "cell", "after"]);
    assert!(xml.find("before").unwrap() < xml.find("<w:tbl>").unwrap());
    assert!(xml.find("</w:tbl>").unwrap() < xml.find("after").unwrap());
}

#[test]
fn splits_multiple_values() {
    let xml = render(&paragraph(&["prefix {~inline}{~a} middle {~~b} suffix"]), json!({"inline":"<b>I</b>","a":"<p>A</p>","b":"<p>B</p>"}));
    assert_eq!(paragraph_texts(&xml), ["prefix I", "A", " middle ", "B", " suffix"]);
    assert!(elements(&xml, "w:p")[0].contains("<w:b/>"));
}

#[test]
fn preserves_repeated_blocks() {
    let xml = render(&paragraph(&["start{#items}{~html}{/items}end"]), json!({"items":[{"html":"<p>A</p>"},{"html":"<p>B</p>"}]}));
    assert_eq!(paragraph_texts(&xml), ["start", "A", "B", "end"]);
}

#[test]
fn keeps_paragraph_and_run_style() {
    let body = "<w:p><w:pPr><w:pStyle w:val=\"Body\"/><w:spacing w:after=\"120\"/></w:pPr><w:r><w:rPr><w:b/></w:rPr><w:t>before</w:t></w:r><w:r><w:rPr><w:i/></w:rPr><w:t>{~html}</w:t></w:r><w:r><w:rPr><w:u w:val=\"single\"/></w:rPr><w:t>after</w:t></w:r></w:p>";
    let xml = render(body, json!({"html":"<p>body</p>"}));
    let paragraphs = elements(&xml, "w:p");
    assert_eq!(paragraphs.len(), 3);
    assert!(paragraphs.iter().all(|p| p.contains("w:pStyle w:val=\"Body\"") && p.contains("w:spacing w:after=\"120\"")));
    assert!(paragraphs[0].contains("<w:b/>") && paragraphs[1].contains("<w:i/>") && paragraphs[2].contains("<w:u w:val=\"single\"/>"));
}

#[test]
fn omits_empty_boundaries() {
    let xml = render(&paragraph(&[" {~empty}{~html} "]), json!({"empty":"", "html":"<p>body</p>"}));
    assert_eq!(paragraph_texts(&xml), ["body"]);
    let xml = render(&paragraph(&["a{~~empty}b"]), json!({"empty":""}));
    assert_eq!(paragraph_texts(&xml), ["ab"]);
}

#[test]
fn leaves_other_modes_unchanged() {
    let body = document(&paragraph(&["before{~html}after"]));
    let data = json!({"html":"<p>A</p><p>B</p>"});
    let xml = Template::parse(&body, &FillOptions::default()).unwrap().try_render(&data).unwrap();
    assert_eq!(paragraph_texts(&xml), ["beforeABafter"]);
    let options = FillOptions::default().read_json(&json!({"html":{"onInvalidBlock":"block"}})).unwrap();
    let xml = Template::parse(&body, &options).unwrap().try_render(&data).unwrap();
    assert_eq!(paragraph_texts(&xml), ["A", "B"]);
    assert_eq!(text(&xml), "AB");
}

#[test]
fn nests_tables_inside_cells() {
    let body = "<w:tbl><w:tr><w:tc><w:p><w:r><w:t>before{~html}after</w:t></w:r></w:p></w:tc></w:tr></w:tbl>";
    let xml = render(body, json!({"html":"<table><tr><td>inner</td></tr></table>"}));
    assert_eq!(xml.matches("<w:tbl>").count(), 2);
    assert!(xml.contains("before</w:t></w:r></w:p><w:tbl>"));
    assert!(xml.contains("</w:tbl><w:p><w:r><w:t xml:space=\"preserve\">after"));
}

#[test]
fn separates_adjacent_tables() {
    let table = "<table><tr><td>A</td></tr></table>";
    let xml = render(&paragraph(&["{~a}{~b}"]), json!({"a":table,"b":table}));
    assert_eq!(xml.matches("<w:tbl>").count(), 2);
    assert!(xml.contains("</w:tbl><w:p/><w:tbl>"));
}

#[test]
fn retains_hyperlinks_and_ranges() {
    let body = "<w:p><w:bookmarkStart w:id=\"1\" w:name=\"range\"/><w:hyperlink r:id=\"rId1\"><w:r><w:rPr><w:i/></w:rPr><w:t>before{~html}after</w:t></w:r></w:hyperlink><w:bookmarkEnd w:id=\"1\"/></w:p>";
    let xml = render(body, json!({"html":"<p>body</p>"}));
    assert_eq!(paragraph_texts(&xml), ["before", "body", "after"]);
    assert_eq!(xml.matches("<w:hyperlink r:id=\"rId1\">").count(), 2);
    assert_eq!(xml.matches("<w:bookmarkStart").count(), 1);
    assert_eq!(xml.matches("<w:bookmarkEnd").count(), 1);
}

#[test]
fn keeps_section_on_last_part() {
    let body = "<w:p><w:pPr><w:sectPr><w:pgSz w:w=\"10000\" w:h=\"15000\"/></w:sectPr></w:pPr><w:r><w:t>before{~html}after</w:t></w:r></w:p>";
    let xml = render(body, json!({"html":"<p>body</p>"}));
    let paragraphs = elements(&xml, "w:p");
    assert_eq!(xml.matches("<w:sectPr>").count(), 1);
    assert!(paragraphs[2].contains("<w:sectPr>"));
    assert!(!paragraphs[0].contains("<w:sectPr>") && !paragraphs[1].contains("<w:sectPr>"));
}

#[test]
fn ignores_markers_in_attributes() {
    let body = "<w:p data-marker=\"\u{e000}0\u{e001}\"><w:r><w:t>before{~html}after</w:t></w:r></w:p>";
    let xml = render(body, json!({"html":"<p>body</p>"}));
    assert_eq!(text(&xml), "beforebodyafter");
}
