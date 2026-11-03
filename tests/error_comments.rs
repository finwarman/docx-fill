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

#[test]
fn annotates_tags_across_breaks() {
    for (element, text) in [("<w:tab/>", "{ba\td"), ("<w:br/>", "{ba\nd")] {
        let body = format!("<w:p><w:r><w:t>{{ba</w:t>{element}<w:t>d</w:t></w:r></w:p>");
        let parts = parts(&body, "");
        let error = PackageTemplate::parse(parts.clone(), &FillOptions::default()).unwrap_err();
        let mut document = FilledDocument::new(parts);

        assert_eq!(document.annotate_errors(&error, &CommentDefaults::default()).unwrap(), 1);
        assert_eq!(document.comments()[0].range.text, text);
    }
}

#[test]
fn keeps_empty_paragraph_indices() {
    let parts = parts("<w:p/><w:p><w:r><w:t>{bad</w:t></w:r></w:p>", "");
    let error = PackageTemplate::parse(parts.clone(), &FillOptions::default()).unwrap_err();
    let mut document = FilledDocument::new(parts);

    assert_eq!(document.annotate_errors(&error, &CommentDefaults::default()).unwrap(), 1);
    let comments = document.comments();
    assert_eq!(comments[0].range.start.paragraph, 1);
    assert_eq!(comments[0].range.text, "{bad");
}

#[test]
fn skips_expanded_line_locations() {
    let body = "<w:p><w:r><w:t>one</w:t><w:br/><w:t>two</w:t></w:r></w:p><w:p><w:r><w:t>{bad</w:t></w:r></w:p>";
    let parts = parts(body, "");
    let mut options = FillOptions::default();
    options.paragraph_placeholder.soft_breaks = docx_fill::SoftBreakScope::All;
    let error = PackageTemplate::parse(parts.clone(), &options).unwrap_err();
    let mut document = FilledDocument::new(parts);

    assert!(error.diagnostics().iter().all(|error| error.paragraph.is_none() && error.offset.is_none()));
    assert_eq!(document.annotate_errors(&error, &CommentDefaults::default()).unwrap(), 0);
}

#[test]
fn annotates_many_errors_in_linear_time() {
    let parts = parts(&paragraph(&["}"]).repeat(5_000), "");
    let error = PackageTemplate::parse(parts.clone(), &FillOptions::default()).unwrap_err();
    let mut document = FilledDocument::new(parts);
    let started = std::time::Instant::now();

    assert_eq!(document.annotate_errors(&error, &CommentDefaults::default()).unwrap(), 5_000);
    assert!(started.elapsed().as_secs() < 5, "{:?}", started.elapsed());
    assert_eq!(document.comments().len(), 5_000);
}

#[test]
fn annotates_many_errors_in_one_paragraph() {
    let piece = format!("{} }}", "x".repeat(64));
    for body in [paragraph(&[&piece.repeat(5_000)]), paragraph(&vec![piece.as_str(); 5_000])] {
        let parts = parts(&body, "");
        let error = PackageTemplate::parse(parts.clone(), &FillOptions::default()).unwrap_err();
        let mut document = FilledDocument::new(parts);
        let started = std::time::Instant::now();

        assert_eq!(document.annotate_errors(&error, &CommentDefaults::default()).unwrap(), 5_000);
        assert!(started.elapsed().as_secs() < 5, "{:?}", started.elapsed());
        let output: BTreeMap<_, _> = document.into_parts().into_iter().collect();
        let body = String::from_utf8_lossy(&output["word/document.xml"]);
        assert_eq!(body.matches("<w:commentRangeStart ").count(), 5_000);
        assert_eq!(body.matches("<w:commentRangeEnd ").count(), 5_000);
        assert_eq!(body.matches("<w:commentReference ").count(), 5_000);
    }
}

#[test]
fn locates_entities_across_controls() {
    let body = "<w:p><w:r><w:t>é &amp; {b</w:t><w:tab/><w:t>&#x61;d</w:t></w:r></w:p>";
    let parts = parts(body, "");
    let error = PackageTemplate::parse(parts.clone(), &FillOptions::default()).unwrap_err();
    let mut document = FilledDocument::new(parts);

    assert_eq!(document.annotate_errors(&error, &CommentDefaults::default()).unwrap(), 1);
    assert_eq!(document.comments()[0].range.text, "{b\tad");
}

#[test]
fn keeps_overlapping_error_ranges() {
    let parts = parts(&paragraph(&["{bad"]), "");
    let outer = PackageTemplate::parse(parts.clone(), &FillOptions::default()).unwrap_err();
    let mut inner = outer.clone();
    inner.offset = Some(1);
    inner.tag = Some("bad".into());
    let mut errors = outer.clone();
    errors.errors = vec![outer, inner.clone(), inner].into_boxed_slice();
    let mut document = FilledDocument::new(parts);

    assert_eq!(document.annotate_errors(&errors, &CommentDefaults::default()).unwrap(), 3);
    let comments = document.comments();
    assert_eq!(comments.iter().map(|comment| comment.range.text.as_str()).collect::<Vec<_>>(), ["{bad", "bad", "bad"]);
}

#[test]
fn marks_only_a_duplicate_close() {
    let parts = parts(&paragraph(&["{a}} then a"]), "");
    let error = PackageTemplate::parse(parts.clone(), &FillOptions::default()).unwrap_err();
    let mut document = FilledDocument::new(parts);

    assert_eq!(document.annotate_errors(&error, &CommentDefaults::default()).unwrap(), 1);
    assert_eq!(document.comments()[0].range.text, "}");
}
