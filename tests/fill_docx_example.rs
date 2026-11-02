//! The `fill_docx` example, on zips built here.

mod common;

#[allow(dead_code)]
#[path = "../examples/fill_docx.rs"]
mod example;

use std::io::{Cursor, Read, Write};

use common::{document, paragraph, text};
use serde_json::json;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

const CONTENT_TYPES: &[u8] = br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;

fn zipped(parts: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in parts {
        if let Some(folder) = name.strip_suffix('/') {
            zip.add_directory(folder, SimpleFileOptions::default()).unwrap();
        } else {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(bytes).unwrap();
        }
    }
    zip.finish().unwrap().into_inner()
}

fn unzipped(docx: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut zip = ZipArchive::new(Cursor::new(docx)).unwrap();
    (0..zip.len()).map(|index| {
        let mut part = zip.by_index(index).unwrap();
        let mut bytes = Vec::new();
        part.read_to_end(&mut bytes).unwrap();
        (part.name().unwrap().into_owned(), bytes)
    }).collect()
}

#[test]
fn fills_the_document_and_keeps_other_parts_in_order() {
    let body = document(&paragraph(&["Hello {na", "me}"]));
    let logo: &[u8] = &[0x89, b'P', b'N', b'G', 0, 0xff];
    let docx = zipped(&[("[Content_Types].xml", CONTENT_TYPES), ("word/document.xml", body.as_bytes()), ("word/media/logo.png", logo), ("word/styles.xml", b"<w:styles>{name}</w:styles>")]);

    let filled = unzipped(&example::fill_docx(&docx, &json!({ "name": "Ann" })).unwrap());

    let names: Vec<&str> = filled.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["[Content_Types].xml", "word/document.xml", "word/media/logo.png", "word/styles.xml"]);
    assert_eq!(filled[0].1, CONTENT_TYPES);
    assert_eq!(text(std::str::from_utf8(&filled[1].1).unwrap()), "Hello Ann");
    assert_eq!(filled[2].1, logo);
    assert_eq!(filled[3].1, b"<w:styles>{name}</w:styles>");
}

#[test]
fn leaves_out_folder_entries() {
    let body = document(&paragraph(&["{v}"]));
    let docx = zipped(&[("word/", b""), ("word/document.xml", body.as_bytes())]);

    let filled = unzipped(&example::fill_docx(&docx, &json!({ "v": "V" })).unwrap());

    assert_eq!(filled.len(), 1);
    assert_eq!(filled[0].0, "word/document.xml");
}

#[test]
fn names_the_part_a_template_error_is_in() {
    let body = document(&paragraph(&["{#open}"]));
    let docx = zipped(&[("word/document.xml", body.as_bytes())]);

    let error = example::fill_docx(&docx, &json!({})).unwrap_err();

    assert_eq!(error.to_string(), r#"word/document.xml: The loop with tag "open" is unclosed"#);
}

#[test]
fn refuses_a_file_that_is_not_a_zip() {
    assert!(example::fill_docx(b"not a zip", &json!({})).is_err());
}
