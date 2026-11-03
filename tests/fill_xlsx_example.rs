#[path = "common/spreadsheet.rs"] mod common;
#[allow(dead_code)]
#[path = "../examples/fill_xlsx.rs"] mod example;
use std::io::{Cursor, Read, Write};
use serde_json::json;
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

#[test]
fn fills_an_xlsx_archive_and_keeps_binary_members() {
    let parts=common::package(&format!(r#"<row r="1">{}</row>"#,common::inline("A1","{name}")));
    let mut zip=ZipWriter::new(Cursor::new(Vec::new()));
    for (name,bytes) in &parts { zip.start_file(name,SimpleFileOptions::default()).unwrap(); zip.write_all(bytes).unwrap(); }
    let input=zip.finish().unwrap().into_inner();
    let output=example::fill_xlsx(&input,&json!({"name":"Excel"})).unwrap();
    let mut zip=ZipArchive::new(Cursor::new(output)).unwrap();
    assert_eq!(zip.len(),parts.len());
    let mut sheet=String::new();zip.by_name("xl/worksheets/sheet1.xml").unwrap().read_to_string(&mut sheet).unwrap();
    assert!(sheet.contains("Excel"));
    let mut bytes=Vec::new();zip.by_name("xl/media/unchanged.bin").unwrap().read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes,[0,255,42,1]);
}

#[test]
fn refuses_non_zip_data_and_oversized_archives() {
    assert!(example::fill_xlsx(b"not a ZIP",&json!({})).is_err());
    assert!(example::fill_xlsx(&vec![0;64*1024*1024+1],&json!({})).is_err());
}
