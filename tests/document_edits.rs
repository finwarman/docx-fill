mod common;

use common::document;
use docx_fill::{DocumentEdits, FillOptions, Margins, PackageTemplate};
use serde_json::json;

fn edit(edits: DocumentEdits, parts: &[(&str, &str)]) -> Vec<(String, String)> {
    let parts = parts.iter().map(|(name, xml)| (name.to_string(), xml.as_bytes().to_vec()));
    let options = FillOptions { document: edits, ..FillOptions::default() };
    let template = PackageTemplate::parse(parts, &options).unwrap();
    template.render(&json!({})).into_iter().map(|(name, bytes)| (name, String::from_utf8(bytes.into_owned()).unwrap())).collect()
}

fn part(parts: &[(String, String)], name: &str) -> String {
    parts.iter().find(|(each, _)| each == name).unwrap().1.clone()
}

#[test]
fn protects_the_document_read_only_in_schema_order() {
    let settings = r#"<?xml version="1.0"?><w:settings xmlns:w="x"><w:zoom w:percent="100"/><w:documentProtection w:edit="comments"/><w:defaultTabStop w:val="720"/></w:settings>"#;
    let parts = edit(DocumentEdits { read_only: true, ..DocumentEdits::default() }, &[("word/settings.xml", settings)]);

    assert_eq!(
        part(&parts, "word/settings.xml"),
        r#"<?xml version="1.0"?><w:settings xmlns:w="x"><w:zoom w:percent="100"/><w:documentProtection w:edit="readOnly" w:enforcement="1"/><w:defaultTabStop w:val="720"/></w:settings>"#
    );
}

#[test]
fn protects_empty_settings_roots() {
    let before = "<?xml version=\"1.0\"?>\n<!--before-->\n";
    let root = "<w:settings xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:meta='urn:metadata' meta:note='a>b' xml:space='preserve' ";
    let after = "\n<!--after-->\n";
    let expected = format!("{before}{root}><w:documentProtection w:edit=\"readOnly\" w:enforcement=\"1\"/></w:settings>{after}");

    for ending in ["/>", "></w:settings>"] {
        let settings = format!("{before}{root}{ending}{after}");
        let options = FillOptions::default().read_json(&json!({"document":{"readOnly":true}})).unwrap();
        let parts = edit(options.document, &[("word/settings.xml", &settings)]);
        assert_eq!(part(&parts, "word/settings.xml"), expected);
    }
}

#[test]
fn ignores_commented_settings() {
    let before = "<?xml version=\"1.0\"?><!-- sample <w:settings/> -->";
    let after = "<!-- sample </w:settings> -->";
    let root = "<w:settings xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"";
    for ending in ["/>", "></w:settings>"] {
        let input = format!("{before}{root}{ending}{after}");
        let edits = DocumentEdits { read_only: true, ..DocumentEdits::default() };
        let parts = edit(edits, &[("word/settings.xml", &input)]);

        assert_eq!(part(&parts, "word/settings.xml"), format!("{before}{root}><w:documentProtection w:edit=\"readOnly\" w:enforcement=\"1\"/></w:settings>{after}"));
    }
}

#[test]
fn sets_every_sections_margins_in_twips() {
    let body = r#"<w:p><w:pPr><w:sectPr><w:pgMar w:top="1" w:right="1" w:bottom="1" w:left="1" w:header="708"/></w:sectPr></w:pPr></w:p><w:sectPr><w:pgMar w:top="1440"/></w:sectPr>"#;
    let margins = Margins { top: 0.5, right: 1.0, bottom: 0.75, left: 1.25 };
    let parts = edit(DocumentEdits { margins: Some(margins), ..DocumentEdits::default() }, &[("word/document.xml", &document(body))]);

    let xml = part(&parts, "word/document.xml");
    assert!(xml.contains(r#"<w:pgMar w:top="720" w:right="1440" w:bottom="1080" w:left="1800" w:header="708"/>"#));
    assert!(xml.contains(r#"<w:pgMar w:top="720" w:right="1440" w:bottom="1080" w:left="1800"/>"#));
}

#[test]
fn sets_the_default_language() {
    let language = || DocumentEdits { language: Some("en-GB".to_string()), ..DocumentEdits::default() };
    let with_defaults = r#"<w:styles xmlns:w="x"><w:docDefaults><w:rPrDefault><w:rPr><w:sz w:val="22"/><w:lang w:val="en-US" w:eastAsia="ja-JP"/></w:rPr></w:rPrDefault></w:docDefaults></w:styles>"#;
    let without = r#"<w:styles xmlns:w="x"><w:style/></w:styles>"#;

    assert!(part(&edit(language(), &[("word/styles.xml", with_defaults)]), "word/styles.xml").contains(r#"<w:sz w:val="22"/><w:lang w:val="en-GB" w:eastAsia="ja-JP"/>"#));
    assert_eq!(
        part(&edit(language(), &[("word/styles.xml", without)]), "word/styles.xml"),
        r#"<w:styles xmlns:w="x"><w:docDefaults><w:rPrDefault><w:rPr><w:lang w:val="en-GB"/></w:rPr></w:rPrDefault></w:docDefaults><w:style/></w:styles>"#
    );
}

#[test]
fn sets_empty_styles_language() {
    let before = "<?xml version=\"1.0\"?>\n<!--before-->\n";
    let root = "<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xml:space='preserve' ";
    let after = "\n<!--after-->\n";
    let expected = format!("{before}{root}><w:docDefaults><w:rPrDefault><w:rPr><w:lang w:val=\"fr-FR\"/></w:rPr></w:rPrDefault></w:docDefaults></w:styles>{after}");

    for ending in ["/>", "></w:styles>"] {
        let styles = format!("{before}{root}{ending}{after}");
        let options = FillOptions::default().read_json(&json!({"document":{"language":"fr-FR"}})).unwrap();
        let parts = edit(options.document, &[("word/styles.xml", &styles)]);
        assert_eq!(part(&parts, "word/styles.xml"), expected);
    }
}

#[test]
fn ignores_commented_styles() {
    let before = "<!-- sample <w:styles/> -->";
    let after = "<!-- sample </w:styles> -->";
    let root = "<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"";
    for ending in ["/>", "></w:styles>"] {
        let input = format!("{before}{root}{ending}{after}");
        let edits = DocumentEdits { language: Some("fr-FR".into()), ..DocumentEdits::default() };
        let parts = edit(edits, &[("word/styles.xml", &input)]);

        assert_eq!(part(&parts, "word/styles.xml"), format!("{before}{root}><w:docDefaults><w:rPrDefault><w:rPr><w:lang w:val=\"fr-FR\"/></w:rPr></w:rPrDefault></w:docDefaults></w:styles>{after}"));
    }
}

#[test]
fn changes_nothing_by_default() {
    let settings = r#"<w:settings xmlns:w="x"/>"#;
    let parts = edit(DocumentEdits::default(), &[("word/settings.xml", settings)]);

    assert_eq!(part(&parts, "word/settings.xml"), settings);
}
