#[allow(dead_code)]
#[path = "../examples/fill_xlsx.rs"]
mod example;
use serde_json::json;
use std::io::{Cursor, Read, Write};
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

fn entries(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut zip = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut parts = Vec::new();
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).unwrap();
        if file.is_dir() {
            continue;
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        parts.push((file.name().unwrap().into_owned(), bytes));
    }
    parts
}
#[test]
fn excel_and_libreoffice_packages_round_trip_without_changing_parts() {
    for input in [
        include_bytes!("fixtures/spreadsheet/excel-simple.xlsx").as_slice(),
        include_bytes!("fixtures/spreadsheet/libreoffice-empty.xlsx").as_slice(),
    ] {
        let output = example::fill_xlsx(input, &json!({})).unwrap();
        assert_eq!(entries(input), entries(&output));
    }
}
#[test]
fn rejects_unsafe_and_duplicate_archive_members() {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("a.xml", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"<a/>").unwrap();
    zip.start_file("b.xml", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"<b/>").unwrap();
    let mut bytes = zip.finish().unwrap().into_inner();
    for at in 0..bytes.len() - 5 {
        if &bytes[at..at + 5] == b"b.xml" {
            bytes[at] = b'a';
        }
    }
    assert!(example::fill_xlsx(&bytes, &json!({})).is_err());
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("../evil.xml", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"<a/>").unwrap();
    assert!(example::fill_xlsx(&zip.finish().unwrap().into_inner(), &json!({})).is_err());
}
#[test]
fn rejects_forged_oversized_expanded_entries_before_decoding() {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("a.xml", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"<a/>").unwrap();
    let mut bytes = zip.finish().unwrap().into_inner();
    let at = bytes.windows(4).position(|w| w == b"PK\x01\x02").unwrap();
    bytes[at + 24..at + 28].copy_from_slice(&(256u32 * 1024 * 1024 + 1).to_le_bytes());
    assert!(example::fill_xlsx(&bytes, &json!({})).is_err());
}
