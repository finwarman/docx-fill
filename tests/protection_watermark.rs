mod common;

use common::document;
use docx_fill::{DocumentEdits, FillOptions, HashAlgorithm, PackageTemplate, Protection, ProtectionKind, Watermark, WatermarkLayout};
use serde_json::json;

const SETTINGS: &str = r#"<w:settings xmlns:w="w"><w:zoom w:percent="100"/></w:settings>"#;

fn edit(edits: DocumentEdits, parts: &[(&str, &str)]) -> Vec<(String, String)> {
    let parts = parts.iter().map(|(name, xml)| (name.to_string(), xml.as_bytes().to_vec()));
    let template = PackageTemplate::parse(parts, &FillOptions { document: edits, ..FillOptions::default() }).unwrap();
    template.render(&json!({})).into_iter().map(|(name, bytes)| (name, String::from_utf8(bytes.into_owned()).unwrap())).collect()
}

fn part(parts: &[(String, String)], name: &str) -> String {
    parts.iter().find(|(each, _)| each == name).unwrap_or_else(|| panic!("no {name}")).1.clone()
}

#[test]
fn protects_with_a_password_hash_word_checks() {
    let protection = Protection { kind: ProtectionKind::Comments, algorithm: HashAlgorithm::Sha256, salt: Some("AAECAwQFBgcICQoLDA0ODw==".to_string()), ..Protection::new("Scope2026!") };
    let parts = edit(DocumentEdits { protection: Some(protection), read_only: true, ..DocumentEdits::default() }, &[("word/settings.xml", SETTINGS)]);

    assert_eq!(
        part(&parts, "word/settings.xml"),
        r#"<w:settings xmlns:w="w"><w:zoom w:percent="100"/><w:documentProtection w:edit="comments" w:enforcement="1" w:cryptProviderType="rsaAES" w:cryptAlgorithmClass="hash" w:cryptAlgorithmType="typeAny" w:cryptAlgorithmSid="12" w:cryptSpinCount="100000" w:hash="R5gYuCZD03npMzJp4UurhpH9Lkr8oXc+8kqwu1GGNEI=" w:salt="AAECAwQFBgcICQoLDA0ODw=="/></w:settings>"#
    );
}

#[test]
fn salts_each_fill_differently_without_a_salt() {
    let edits = DocumentEdits { protection: Some(Protection { spin_count: 10, ..Protection::new("secret1") }), ..DocumentEdits::default() };
    let first = part(&edit(edits.clone(), &[("word/settings.xml", SETTINGS)]), "word/settings.xml");
    let second = part(&edit(edits, &[("word/settings.xml", SETTINGS)]), "word/settings.xml");

    assert!(first.contains(r#"w:edit="readOnly""#) && first.contains(r#"w:cryptAlgorithmSid="14""#));
    assert_ne!(first, second);
}

#[test]
fn rejects_a_short_password_or_bad_salt() {
    for (protection, message) in [
        (Protection::new("12345"), "The password length must be of at least 6 characters"),
        (Protection { salt: Some("not base64!".to_string()), ..Protection::new("123456") }, "The salt is not base64"),
    ] {
        let options = FillOptions { document: DocumentEdits { protection: Some(protection), ..DocumentEdits::default() }, ..FillOptions::default() };
        let error = PackageTemplate::parse([("word/settings.xml".to_string(), SETTINGS.as_bytes().to_vec())], &options).unwrap_err();
        assert!(error.to_string().contains(message));
    }
}

#[test]
fn draws_the_watermark_in_every_header() {
    let header = r#"<w:hdr xmlns:w="w"><w:p><w:r><w:t>Head</w:t></w:r></w:p></w:hdr>"#;
    let watermark = Watermark { text: "DRAFT & co".to_string(), layout: WatermarkLayout::Diagonal };
    let parts = edit(DocumentEdits { watermark: Some(watermark), ..DocumentEdits::default() }, &[("word/document.xml", &document("<w:sectPr/>")), ("word/header1.xml", header), ("word/header2.xml", header)]);

    for (name, index) in [("word/header1.xml", 1), ("word/header2.xml", 2)] {
        let xml = part(&parts, name);
        assert!(xml.starts_with(r#"<w:hdr xmlns:w="w" xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:w10="urn:schemas-microsoft-com:office:word"><w:p><w:pPr><w:pStyle w:val="Header"/>"#));
        assert!(xml.contains(&format!(r#"<v:shape id="PowerPlusWaterMarkObject{index}" o:spid="_x0000_s{index}025""#)));
        assert!(xml.contains("rotation:315;") && xml.contains(r#"string="DRAFT &amp; co""#));
        assert!(xml.ends_with(r#"<w:p><w:r><w:t>Head</w:t></w:r></w:p></w:hdr>"#));
    }
    assert!(!parts.iter().any(|(name, _)| name.contains("watermark")));
}

#[test]
fn adds_a_header_for_the_watermark_when_there_is_none() {
    let body = r#"<w:p><w:pPr><w:sectPr><w:pgSz w:w="1"/></w:sectPr></w:pPr></w:p><w:sectPr><w:pgSz w:w="2"/></w:sectPr>"#;
    let parts = edit(
        DocumentEdits { watermark: Some(Watermark { text: "COPY".to_string(), layout: WatermarkLayout::Horizontal }), ..DocumentEdits::default() },
        &[("word/document.xml", &document(body)), ("[Content_Types].xml", r#"<Types xmlns="t"></Types>"#), ("word/_rels/document.xml.rels", r#"<Relationships xmlns="r"></Relationships>"#)],
    );

    let header = part(&parts, "word/header_docx_fill_watermark.xml");
    assert!(header.contains(r#"string="COPY""#) && !header.contains("rotation"));
    let xml = part(&parts, "word/document.xml");
    assert!(xml.contains(r#"<w:sectPr><w:headerReference w:type="default" r:id="rIdDocxFillWatermark"/><w:pgSz w:w="1"/></w:sectPr>"#));
    assert_eq!(xml.matches("headerReference").count(), 1);
    assert!(xml.contains(r#"xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#));
    assert!(part(&parts, "word/_rels/document.xml.rels").contains(r#"Target="header_docx_fill_watermark.xml""#));
    assert!(part(&parts, "[Content_Types].xml").contains(r#"PartName="/word/header_docx_fill_watermark.xml""#));
}

#[test]
fn keeps_index_text_in_the_watermark() {
    let header = r#"<w:hdr xmlns:w="w"><w:p/></w:hdr>"#;
    let watermark = Watermark { text: "Copy {index}".to_string(), layout: WatermarkLayout::Horizontal };
    let parts = edit(DocumentEdits { watermark: Some(watermark), ..DocumentEdits::default() }, &[("word/document.xml", &document("<w:sectPr/>")), ("word/header1.xml", header)]);

    assert!(part(&parts, "word/header1.xml").contains(r#"string="Copy {index}""#));
}
