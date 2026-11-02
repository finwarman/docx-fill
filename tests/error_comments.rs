mod common;

use std::collections::BTreeMap;
use common::{document, paragraph};
use docx_fill::{CommentDefaults, FillOptions, FilledDocument, PackageTemplate, Template};

fn parts(body: &str, header: &str) -> Vec<(String, Vec<u8>)> {
    vec![
        ("word/document.xml".into(), document(body).into_bytes()),
        ("word/header1.xml".into(), format!("<w:hdr xmlns:w=\"w\">{header}</w:hdr>").into_bytes()),
        ("word/_rels/document.xml.rels".into(), b"<Relationships></Relationships>".to_vec()),
        ("[Content_Types].xml".into(), b"<Types></Types>".to_vec()),
    ]
}

#[test]
fn annotates_body_and_headers() {
    let parts = parts(&paragraph(&["{body"]), &paragraph(&["{header"]));
    let error = PackageTemplate::parse(parts.clone(), &FillOptions::default()).unwrap_err();
    let mut document = FilledDocument::new(parts);
    let by = CommentDefaults { author:"Template check".into(), initials:None, date:None };

    assert_eq!(document.annotate_errors(&error, &by).unwrap(), 2);
    let parts: BTreeMap<_, _> = document.into_parts().into_iter().collect();
    assert!(String::from_utf8_lossy(&parts["word/document.xml"]).contains("commentRangeStart"));
    assert!(String::from_utf8_lossy(&parts["word/header1.xml"]).contains("commentRangeStart"));
    let comments = String::from_utf8_lossy(&parts["word/comments.xml"]);
    assert_eq!(comments.matches("<w:comment w:id=").count(), 2);
    assert!(comments.contains("Template check") && comments.contains("unclosed_tag"));
}

#[test]
fn converts_escaped_byte_offsets() {
    let parts = parts(&paragraph(&["é &amp; ", "{bad"]), "");
    let error = PackageTemplate::parse(parts.clone(), &FillOptions::default()).unwrap_err();
    let mut document = FilledDocument::new(parts);

    assert_eq!(document.annotate_errors(&error, &CommentDefaults::default()).unwrap(), 1);
    assert_eq!(document.comments()[0].range.text, "{bad");
}

#[test]
fn skips_errors_without_locations() {
    let error = Template::parse("<w:p>", &FillOptions::default()).unwrap_err();
    let mut document = FilledDocument::new(parts(&paragraph(&["unchanged"]), ""));

    assert_eq!(document.annotate_errors(&error, &CommentDefaults::default()).unwrap(), 0);
    assert!(document.comments().is_empty());
}

#[test]
fn annotates_multiple_errors() {
    let parts = parts(&paragraph(&["{{bad}}"]), "");
    let error = PackageTemplate::parse(parts.clone(), &FillOptions::default()).unwrap_err();
    let mut document = FilledDocument::new(parts);

    assert_eq!(document.annotate_errors(&error, &CommentDefaults::default()).unwrap(), 2);
    assert_eq!(document.comments().len(), 2);
}

#[test]
fn respects_tabs_between_runs() {
    let body = "<w:p><w:r><w:t>prefix</w:t><w:tab/></w:r><w:r><w:t>{bad</w:t></w:r></w:p>";
    let parts = parts(body, "");
    let error = PackageTemplate::parse(parts.clone(), &FillOptions::default()).unwrap_err();
    let mut document = FilledDocument::new(parts);

    assert_eq!(document.annotate_errors(&error, &CommentDefaults::default()).unwrap(), 1);
    assert_eq!(document.comments()[0].range.text, "{bad");
}
