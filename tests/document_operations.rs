use docx_fill::{ContentBoundary, FilledDocument};
use serde_json::json;

fn doc(body: &str) -> FilledDocument {
    FilledDocument::new([("word/document.xml".into(), format!("<w:document xmlns:w=\"w\"><w:body>{body}<w:sectPr/></w:body></w:document>").into_bytes())])
}

fn xml(doc: FilledDocument, part: &str) -> String {
    String::from_utf8(doc.into_parts().into_iter().find(|(name, _)| name == part).unwrap().1).unwrap()
}

const PAGES: &str = "<w:p><w:pPr><w:keepNext/></w:pPr><w:r><w:rPr><w:b/></w:rPr><w:t>first</w:t><w:br w:type=\"page\"/><w:t>second</w:t><w:br w:type=\"page\"/><w:t>third</w:t></w:r></w:p>";

#[test]
fn drops_first_and_last_pages() {
    let mut first = doc(PAGES);
    assert_eq!(first.page_breaks().len(), 2);
    first.drop_content_between(ContentBoundary::Start, ContentBoundary::PageBreak(0)).unwrap();
    let first = xml(first, "word/document.xml");
    assert!(!first.contains("first") && first.contains("second") && first.contains("third"));
    assert!(first.contains("<w:b/>") && first.contains("<w:keepNext/>") && first.contains("<w:sectPr/>"));

    let mut last = doc(PAGES);
    last.drop_content_between(ContentBoundary::PageBreak(1), ContentBoundary::End).unwrap();
    let last = xml(last, "word/document.xml");
    assert!(last.contains("first") && last.contains("second") && !last.contains("third"));
    assert!(last.contains("<w:sectPr/>"));
}

#[test]
fn rejects_table_page_boundaries() {
    let mut document = doc(&format!("<w:tbl><w:tr><w:tc>{PAGES}</w:tc></w:tr></w:tbl>"));
    assert!(document.drop_content_between(ContentBoundary::Start, ContentBoundary::PageBreak(0)).is_err());
    assert_eq!(document.page_breaks().len(), 2);
}

#[test]
fn changes_hyperlink_base() {
    let mut document = doc("<w:p/>");
    document.set_hyperlink_base("https://example.com/?a=1&b=2");
    document.set_hyperlink_base("https://other.example/");
    let parts = document.into_parts();
    let read = |path: &str| String::from_utf8_lossy(&parts.iter().find(|(name, _)| name == path).unwrap().1).into_owned();

    let app = read("docProps/app.xml");
    assert_eq!(app.matches("<HyperlinkBase>").count(), 1);
    assert!(app.contains("https://other.example/"));
    assert!(read("_rels/.rels").contains("extended-properties"));
    assert!(read("[Content_Types].xml").contains("/docProps/app.xml"));
}

#[test]
fn uses_existing_properties_part() {
    let mut document = FilledDocument::new([
        ("_rels/.rels".into(), b"<Relationships><Relationship Id=\"app\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties\" Target=\"custom/app.xml\"/></Relationships>".to_vec()),
        ("custom/app.xml".into(), b"<ep:Properties xmlns:ep=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\"><ep:Company>Company</ep:Company></ep:Properties>".to_vec()),
    ]);
    document.set_hyperlink_base("https://example.com/");
    let xml = xml(document, "custom/app.xml");
    assert!(xml.contains("<ep:HyperlinkBase>https://example.com/</ep:HyperlinkBase>"));
    assert!(xml.contains("<ep:Company>Company</ep:Company>"));
}

#[test]
fn replaces_regex_captures() {
    let mut document = doc("<w:p><w:r><w:rPr><w:b/></w:rPr><w:t>猫 12 </w:t></w:r><w:r><w:t>cat 34</w:t></w:r></w:p>");
    let found = document.search_regex(r"(猫|cat) (\d+)").unwrap();
    assert_eq!(found[1].start.offset, 5);
    assert_eq!(document.replace_all_regex(r"(猫|cat) (\d+)", "$2 & $1").unwrap(), 2);
    assert_eq!(document.search("12 & 猫 34 & cat").len(), 1);
    let xml = xml(document, "word/document.xml");
    assert!(xml.contains("<w:b/>") && xml.contains("&amp;"));
}

#[test]
fn fills_empty_regex_matches() {
    let mut document = doc("<w:p/><w:p><w:pPr><w:keepNext/></w:pPr></w:p>");
    assert_eq!(document.replace_all_regex("^$", "new").unwrap(), 2);
    assert_eq!(document.search("new").len(), 2);
    assert!(document.replace_all_regex("[", "bad").is_err());
}

#[test]
fn exposes_document_json_edits() {
    let mut document = doc("<w:p><w:r><w:t>{#outer}{#inner}x{/inner}{/outer}</w:t></w:r></w:p>");
    let loops = document.run_json(&json!([{"op":"getLoopTags"}])).unwrap();
    assert_eq!(loops[0][0]["value"], "outer");
    assert_eq!(loops[0][1]["value"], "inner");
    let output = document.run_json(&json!([{"op":"setHyperlinkBase","url":"https://example.com"},{"op":"getPageBreaks"},{"op":"searchRegex","pattern":"outer"}])).unwrap();
    assert!(output[0].is_null() && output[1].as_array().unwrap().is_empty());
    assert_eq!(output[2].as_array().unwrap().len(), 2);
}
