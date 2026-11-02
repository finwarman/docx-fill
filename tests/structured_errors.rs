mod common;

use common::{cell, document, paragraph, row, table};
use docx_fill::{FillErrorId, FillOptions, PackageTemplate, Template};

#[test]
fn collects_errors_across_parts() {
    let parts = [
        ("word/document.xml".into(), document(&paragraph(&["{a"])).into_bytes()),
        ("word/header1.xml".into(), b"<w:hdr xmlns:w=\"w\"><w:p><w:r><w:t>{b</w:t></w:r></w:p></w:hdr>".to_vec()),
    ];
    let error = PackageTemplate::parse(parts, &FillOptions::default()).err().unwrap();
    let errors = error.diagnostics();

    assert_eq!(error.id, FillErrorId::MultiError);
    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0].part.as_deref(), Some("word/document.xml"));
    assert_eq!(errors[1].part.as_deref(), Some("word/header1.xml"));
    assert!(errors.iter().all(|error| error.id == FillErrorId::UnclosedTag));
    assert_eq!(errors[0].tag.as_deref(), Some("a"));
}

#[test]
fn reports_both_duplicate_delimiters() {
    let error = Template::parse(&document(&paragraph(&["{{name}}"])), &FillOptions::default()).err().unwrap();
    let ids: Vec<_> = error.diagnostics().iter().map(|error| error.id).collect();

    assert_eq!(ids, [FillErrorId::DuplicateOpenTag, FillErrorId::DuplicateCloseTag]);
}

#[test]
fn collects_loop_and_expression_errors() {
    let xml = document(&(paragraph(&["{#a}{/b}"]) + &paragraph(&["{#c}"]) + &paragraph(&["{x+}", "{y+}"])));
    let options = FillOptions { expressions: true, ..FillOptions::default() };
    let error = Template::parse(&xml, &options).err().unwrap();
    let ids: Vec<_> = error.diagnostics().iter().map(|error| error.id).collect();

    assert_eq!(ids.iter().filter(|&&id| id == FillErrorId::ClosingTagMismatch).count(), 1);
    assert_eq!(ids.iter().filter(|&&id| id == FillErrorId::UnclosedLoop).count(), 1);
    assert_eq!(ids.iter().filter(|&&id| id == FillErrorId::ScopeParserCompilation).count(), 2);
}

#[test]
fn identifies_paragraph_and_offset() {
    let xml = document(&(paragraph(&["valid"]) + &paragraph(&["é {a"])));
    let error = Template::parse(&xml, &FillOptions::default()).err().unwrap();

    assert_eq!(error.id.as_str(), "unclosed_tag");
    assert_eq!(error.paragraph, Some(1));
    assert_eq!(error.offset, Some(3));
    assert_eq!(error.tag.as_deref(), Some("a"));
}

#[test]
fn identifies_malformed_xml() {
    let error = Template::parse("<w:p><w:r></w:p>", &FillOptions::default()).err().unwrap();

    assert_eq!(error.id, FillErrorId::MalformedXml);
}

#[test]
fn identifies_unbalanced_table_loops() {
    let xml = document(&table(&[row(&[cell(&["{#a}1{/a}{#b}"]), cell(&["2{/b}"])])]));
    let error = Template::parse(&xml, &FillOptions::default()).err().unwrap();

    assert_eq!(error.id.as_str(), "unbalanced_loop_tags");
    assert_eq!(error.tag.as_deref(), Some("/b"));
}

#[test]
fn collects_picture_attribute_errors() {
    let picture = r#"<w:p><w:r><w:drawing xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"><wp:docPr id="1" name="{%x+}" descr="{%y+}"/></w:drawing></w:r></w:p>"#;
    let xml = document(&(paragraph(&["{z+}"]) + picture));
    let options = FillOptions { expressions: true, ..FillOptions::default() };
    let error = Template::parse(&xml, &options).err().unwrap();
    let errors = error.diagnostics();

    assert_eq!(errors.len(), 3);
    assert!(errors.iter().all(|error| error.id == FillErrorId::ScopeParserCompilation));
    assert!(errors.iter().all(|error| error.tag.is_some()));
}

#[test]
fn counts_empty_paragraphs() {
    let xml = document(&("<w:p/><w:p><w:r><w:t>before</w:t></w:r><w:drawing><w:p/></w:drawing></w:p>".to_string() + &paragraph(&["{bad"])));
    let error = Template::parse(&xml, &FillOptions::default()).err().unwrap();

    assert_eq!(error.paragraph, Some(3));
    assert_eq!(error.offset, Some(0));
}

#[test]
fn reports_bad_prefixes_once() {
    let parts = [
        ("word/document.xml".into(), document(&paragraph(&["{a}"])).into_bytes()),
        ("word/header1.xml".into(), b"<w:hdr xmlns:w=\"w\"><w:p><w:r><w:t>{b}</w:t></w:r></w:p></w:hdr>".to_vec()),
    ];
    let mut options = FillOptions::default();
    options.tag_prefixes.raw = "#".into();
    let error = PackageTemplate::parse(parts, &options).err().unwrap();

    assert_eq!(error.diagnostics().len(), 1);
    assert_eq!(error.part, None);
}

#[test]
fn names_duplicate_close_tag() {
    let error = Template::parse(&document(&paragraph(&["{x}}"])), &FillOptions::default()).err().unwrap();
    let duplicate = error.diagnostics().into_iter().find(|error| error.id == FillErrorId::DuplicateCloseTag).unwrap();

    assert_eq!(duplicate.tag.as_deref(), Some("x"));
}
