#![allow(dead_code)]

use docx_fill::{FillOptions, fill_part};
use serde_json::Value;

/// Export complete chart packages for manual rendering when explicitly requested.
pub fn export_chart_fixture(name: &str, parts: &std::collections::BTreeMap<String, Vec<u8>>) {
    use std::io::Write;
    let Ok(folder) = std::env::var("DOCX_FILL_CHART_FIXTURES") else { return };
    let mut parts = parts.clone();
    parts.entry("_rels/.rels".to_string()).or_insert_with(|| br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#.to_vec());
    for (path, bytes) in &mut parts {
        if path.ends_with(".rels") { *bytes = String::from_utf8(bytes.clone()).unwrap().replace("<Relationships>", "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">").into_bytes(); }
    }
    if let Some(bytes) = parts.get_mut("[Content_Types].xml") {
        let mut xml = String::from_utf8(bytes.clone()).unwrap().replace("<Types>", "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">");
        if !xml.contains("wordprocessingml.document.main+xml") {
            xml = xml.replace("</Types>", "<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/></Types>");
        }
        *bytes = xml.into_bytes();
    }
    if let Some(bytes) = parts.get_mut("word/document.xml") {
        let mut xml = String::from_utf8(bytes.clone()).unwrap().replace("cx=\"914400\" cy=\"914400\"", "cx=\"5486400\" cy=\"3200400\"");
        if !xml.contains("<w:sectPr") { xml = xml.replace("</w:body>", "<w:sectPr><w:pgSz w:w=\"12240\" w:h=\"15840\"/><w:pgMar w:top=\"1440\" w:right=\"1440\" w:bottom=\"1440\" w:left=\"1440\"/></w:sectPr></w:body>"); }
        *bytes = xml.into_bytes();
    }
    std::fs::create_dir_all(&folder).unwrap();
    let file = std::fs::File::create(std::path::Path::new(&folder).join(format!("{name}.docx"))).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    for (path, bytes) in parts { zip.start_file(path, zip::write::SimpleFileOptions::default()).unwrap(); zip.write_all(&bytes).unwrap(); }
    zip.finish().unwrap();
}

pub const NAMESPACES: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

pub fn document(body: &str) -> String {
    format!("<w:document {NAMESPACES}><w:body>{body}</w:body></w:document>")
}

/// The filled body of a document holding `body`, with the preview's options.
pub fn fill(body: &str, data: Value) -> String {
    fill_with(body, data, &FillOptions::default())
}

/// The filled body of a document holding `body`.
pub fn fill_with(body: &str, data: Value, options: &FillOptions) -> String {
    let xml = fill_part(&document(body), &data, options).expect("template parses");
    let start = xml.find("<w:body>").unwrap() + "<w:body>".len();
    xml[start..xml.rfind("</w:body>").unwrap()].to_string()
}

pub fn run(text: &str) -> String {
    format!("<w:r><w:t>{text}</w:t></w:r>")
}

/// A paragraph with one run per text.
pub fn paragraph(texts: &[&str]) -> String {
    format!("<w:p>{}</w:p>", texts.iter().map(|text| run(text)).collect::<String>())
}

/// A table cell holding one paragraph with one run per text.
pub fn cell(texts: &[&str]) -> String {
    format!("<w:tc>{}</w:tc>", paragraph(texts))
}

pub fn row(cells: &[String]) -> String {
    format!("<w:tr>{}</w:tr>", cells.concat())
}

pub fn table(rows: &[String]) -> String {
    format!("<w:tbl>{}</w:tbl>", rows.concat())
}

/// Each `<name>…</name>` in `xml`, not nested, in order.
pub fn elements<'a>(xml: &'a str, name: &str) -> Vec<&'a str> {
    let (open, close) = (format!("<{name}>"), format!("</{name}>"));
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let Some(length) = rest[start..].find(&close) else { break };
        let end = start + length + close.len();
        found.push(&rest[start..end]);
        rest = &rest[end..];
    }
    found
}

/// The joined `<w:t>` text of `xml`, as written.
pub fn text(xml: &str) -> String {
    let mut out = String::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<w:t") {
        rest = &rest[start + 4..];
        if !(rest.starts_with('>') || rest.starts_with(' ')) {
            continue;
        }
        let open_end = rest.find('>').unwrap();
        if rest[..open_end].ends_with('/') {
            continue;
        }
        rest = &rest[open_end + 1..];
        let end = rest.find("</w:t>").unwrap();
        out.push_str(&rest[..end]);
        rest = &rest[end..];
    }
    out
}

/// Each paragraph's text.
pub fn paragraph_texts(xml: &str) -> Vec<String> {
    elements(xml, "w:p").into_iter().map(text).collect()
}

/// Each row's cell texts.
pub fn read_rows(xml: &str) -> Vec<Vec<String>> {
    elements(xml, "w:tr").into_iter().map(|each| elements(each, "w:tc").into_iter().map(text).collect()).collect()
}
