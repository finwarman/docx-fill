use docx_fill::FilledDocument;
use serde_json::json;

fn document(body: &str) -> FilledDocument {
    FilledDocument::new([("word/document.xml".into(), format!("<w:document xmlns:w=\"w\"><w:body>{body}</w:body></w:document>").into_bytes())])
}

const BOOKMARK: &str = "<w:p><w:bookmarkStart w:id=\"3\" w:name=\"Target\"/><w:r><w:t>hello</w:t></w:r><w:bookmarkEnd w:id=\"3\"/></w:p>";

#[test]
fn reads_bookmark_content() {
    let doc = document(BOOKMARK);
    let bookmarks = doc.bookmarks();

    assert_eq!(bookmarks.len(), 1);
    assert_eq!(bookmarks[0].name, "Target");
    assert_eq!(doc.get_xml(&bookmarks[0]).unwrap(), "<w:r><w:t>hello</w:t></w:r>");
}

#[test]
fn replaces_content_and_keeps_marks() {
    let mut doc = document(BOOKMARK);
    let bookmark = doc.bookmarks().remove(0);
    doc.replace_xml(&bookmark, "<w:r><w:rPr><w:b/></w:rPr><w:t>世界</w:t></w:r>").unwrap();

    assert!(doc.get_xml(&bookmark).unwrap().contains("世界"));
    assert_eq!(doc.bookmarks(), [bookmark]);
    assert_eq!(doc.search("世界").len(), 1);
}

#[test]
fn refuses_broken_replacements() {
    let mut doc = document(BOOKMARK);
    let bookmark = doc.bookmarks().remove(0);

    assert!(doc.replace_xml(&bookmark, "<w:r>").is_err());
    assert!(doc.get_xml(&bookmark).unwrap().contains("hello"));
}

#[test]
fn lists_header_bookmarks() {
    let mut doc = FilledDocument::new([
        ("word/document.xml".into(), BOOKMARK.as_bytes().to_vec()),
        ("word/header1.xml".into(), BOOKMARK.as_bytes().to_vec()),
    ]);

    assert_eq!(doc.bookmarks().len(), 2);
    assert!(doc.run_json(&json!([{"op":"getXml","bookmark":"Target"}])).is_err());
    let result = doc.run_json(&json!([{"op":"replaceXml","bookmark":"Target","part":"word/header1.xml","xml":"<w:r><w:t>header</w:t></w:r>"}])).unwrap();
    assert_eq!(result, json!([null]));
    assert_eq!(doc.get_xml(&doc.bookmarks()[0]).unwrap(), "<w:r><w:t>hello</w:t></w:r>");
}

#[test]
fn handles_empty_and_nested_marks() {
    let xml = "<w:p><w:bookmarkStart w:id=\"1\" w:name=\"Outer\"/><w:bookmarkStart w:id=\"2\" w:name=\"Inner\"/><w:bookmarkEnd w:id=\"2\"/><w:bookmarkEnd w:id=\"1\"/></w:p>";
    let mut doc = document(xml);
    let bookmarks = doc.bookmarks();

    assert_eq!(doc.get_xml(&bookmarks[1]).unwrap(), "");
    doc.replace_xml(&bookmarks[0], "<w:r><w:t>new</w:t></w:r>").unwrap();
    assert_eq!(doc.bookmarks().len(), 1);
    assert!(doc.get_xml(&bookmarks[1]).is_err());
}

#[test]
fn refuses_dangling_overlap() {
    let xml = "<w:p><w:bookmarkStart w:id=\"1\" w:name=\"A\"/><w:bookmarkStart w:id=\"2\" w:name=\"B\"/><w:bookmarkEnd w:id=\"1\"/><w:bookmarkEnd w:id=\"2\"/></w:p>";
    let mut doc = document(xml);
    let bookmark = doc.bookmarks().remove(0);

    assert!(doc.replace_xml(&bookmark, "").is_err());
    assert_eq!(doc.bookmarks().len(), 2);
}
