mod common;

use common::{document, elements, text};
use docx_fill::{FillOptions, PackageTemplate};
use serde_json::json;

const TAG: &str = "<w:p><w:pPr><w:pStyle w:val=\"Body\"/></w:pPr><w:r><w:rPr><w:rFonts w:ascii=\"Body Font\"/><w:b/><w:sz w:val=\"18\"/></w:rPr><w:t>{~~html}</w:t></w:r></w:p>";
const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

fn render(html: &str, styles: Option<&str>) -> Vec<(String, Vec<u8>)> {
    let mut parts = vec![("word/document.xml".into(), document(TAG).into_bytes())];
    if let Some(styles) = styles { parts.push(("word/styles.xml".into(), styles.as_bytes().to_vec())); }
    let template = PackageTemplate::parse(parts, &FillOptions::default()).unwrap();
    template.render(&json!({"html":html})).into_iter().map(|(name, bytes)| (name, bytes.into_owned())).collect()
}

fn part<'a>(parts: &'a [(String, Vec<u8>)], name: &str) -> &'a str {
    std::str::from_utf8(&parts.iter().find(|(path, _)| path == name).unwrap().1).unwrap()
}

#[test]
fn inherits_template_headings() {
    let styles = format!("<w:styles xmlns:w=\"{W}\"><w:style w:type=\"paragraph\" w:styleId=\"Title\"><w:name w:val=\"Title\"/><w:rPr><w:rFonts w:asciiTheme=\"majorHAnsi\"/><w:b w:val=\"0\"/><w:sz w:val=\"56\"/></w:rPr></w:style><w:style w:type=\"paragraph\" w:styleId=\"Heading1\"><w:name w:val=\"heading 1\"/><w:rPr><w:i/></w:rPr></w:style></w:styles>");
    let output = render("<h1>Title</h1><h2>Heading</h2>", Some(&styles));
    let body = part(&output, "word/document.xml");

    assert!(body.contains("<w:pStyle w:val=\"Title\"/>") && body.contains("<w:pStyle w:val=\"Heading1\"/>"));
    assert!(!body.contains("Body Font") && !body.contains("<w:b/>") && !body.contains("<w:sz"));
    assert_eq!(part(&output, "word/styles.xml"), styles);
}

#[test]
fn creates_missing_styles() {
    let output = render("<h1>A</h1><h2>B</h2><h3>C</h3><h4>D</h4><h5>E</h5><h6>F</h6>", None);
    let body = part(&output, "word/document.xml");
    let styles = part(&output, "word/styles.xml");
    for id in ["Title", "Heading1", "Heading2", "Heading3", "Heading4", "Heading5"] {
        assert!(body.contains(&format!("<w:pStyle w:val=\"{id}\"/>")));
        assert!(styles.contains(&format!("w:styleId=\"{id}\"")));
    }
    assert!(styles.contains("w:asciiTheme=\"majorHAnsi\"") && styles.contains("w:ascii=\"Arial\"") && styles.contains("w:sz w:val=\"56\""));
    assert!(part(&output, "word/_rels/document.xml.rels").contains("/styles\" Target=\"styles.xml\""));
    assert!(part(&output, "[Content_Types].xml").contains("/word/styles.xml"));
    assert_eq!(text(body), "ABCDEF");
}

#[test]
fn tracks_fallback_title_only() {
    let output = render("<h1>A</h1><h2>B</h2>", None);
    let styles = part(&output, "word/styles.xml");
    let title = styles.split("w:styleId=\"Title\"").nth(1).unwrap().split("</w:style>").next().unwrap();
    let heading = styles.split("w:styleId=\"Heading1\"").nth(1).unwrap().split("</w:style>").next().unwrap();
    assert!(title.contains("<w:spacing w:val=\"-10\"/>"));
    assert!(!heading.contains("<w:spacing"));

    let existing = format!("<w:styles xmlns:w=\"{W}\"><w:style w:type=\"paragraph\" w:styleId=\"Title\"><w:name w:val=\"Title\"/><w:rPr><w:spacing w:val=\"8\"/></w:rPr></w:style></w:styles>");
    let output = render("<h1>A</h1>", Some(&existing));
    assert_eq!(part(&output, "word/styles.xml"), existing);
}

#[test]
fn keeps_explicit_heading_css() {
    let output = render("<h1 style='font-family:Explicit;font-size:9pt;font-weight:normal;color:red'>A <i>B</i></h1>", None);
    let body = part(&output, "word/document.xml");
    assert!(body.contains("<w:pStyle w:val=\"Title\"/>"));
    for run in elements(body, "w:r") {
        assert!(run.contains("w:ascii=\"Explicit\"") && run.contains("w:sz w:val=\"18\""));
        assert!(run.contains("<w:b w:val=\"0\"/>") && run.contains("w:color w:val=\"FF0000\""));
    }
    assert!(body.contains("<w:i/>") && !body.contains("Body Font"));
}

#[test]
fn resolves_heading_style_names() {
    let styles = format!("<w:styles xmlns:w=\"{W}\"><w:style w:type=\"paragraph\" w:styleId=\"CustomTitle\"><w:name w:val=\"Title\"/></w:style><w:style w:type=\"paragraph\" w:styleId=\"FirstHeading\"><w:name w:val=\"heading 1\"/></w:style></w:styles>");
    let output = render("<h1>A</h1><h2>B</h2>", Some(&styles));
    let body = part(&output, "word/document.xml");
    assert!(body.contains("w:val=\"CustomTitle\"") && body.contains("w:val=\"FirstHeading\""));
    assert_eq!(part(&output, "word/styles.xml"), styles);
}

#[test]
fn avoids_character_style_ids() {
    let styles = format!("<w:styles xmlns:w=\"{W}\"><w:style w:type=\"character\" w:styleId=\"Title\"><w:name w:val=\"Title\"/></w:style></w:styles>");
    let output = render("<h1>A</h1>", Some(&styles));
    assert!(part(&output, "word/document.xml").contains("w:pStyle w:val=\"DocxFillTitle\""));
    assert!(part(&output, "word/styles.xml").contains("w:type=\"paragraph\" w:styleId=\"DocxFillTitle\""));
}

#[test]
fn skips_unused_heading_styles() {
    let output = render("<p>A</p>", None);
    assert!(!output.iter().any(|(name, _)| name == "word/styles.xml"));
    let output = render("<h1>A</h1>", None);
    assert!(!part(&output, "word/styles.xml").contains("w:styleId=\"Heading1\""));
}

#[test]
fn reads_related_styles_part() {
    let styles = format!("<w:styles xmlns:w=\"{W}\"><w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Body\"><w:name w:val=\"Normal\"/></w:style></w:styles>");
    let header = format!("<w:hdr xmlns:w=\"{W}\">{TAG}</w:hdr>");
    let relations = "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"styles\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"../format/styles.xml\"/></Relationships>";
    let template = PackageTemplate::parse([
        ("word/document.xml".into(), document(TAG).into_bytes()), ("word/header1.xml".into(), header.into_bytes()),
        ("format/styles.xml".into(), styles.into_bytes()), ("word/_rels/document.xml.rels".into(), relations.as_bytes().to_vec()),
    ], &FillOptions::default()).unwrap();
    let output: Vec<_> = template.render(&json!({"html":"<h1>A</h1><h2>B</h2>"})).into_iter().map(|(name, bytes)| (name, bytes.into_owned())).collect();
    let styles = part(&output, "format/styles.xml");

    assert_eq!(styles.matches("w:styleId=\"Title\"").count(), 1);
    assert!(styles.contains("<w:basedOn w:val=\"Body\"/>"));
    assert!(part(&output, "word/header1.xml").contains("w:pStyle w:val=\"Title\""));
    assert!(!output.iter().any(|(name, _)| name == "word/styles.xml"));
}

#[test]
fn keeps_custom_heading_style() {
    struct Custom;
    impl docx_fill::FillHooks for Custom {
        fn html_style_transform(&mut self, element: &docx_fill::HtmlElement<'_>) -> Option<String> {
            (element.name() == "h1").then(|| "CustomHeading".into())
        }
    }
    let template = PackageTemplate::parse([("word/document.xml".into(), document(TAG).into_bytes())], &FillOptions::default()).unwrap();
    let output: Vec<_> = template.render_with(&json!({"html":"<h1>A</h1>"}), &mut Custom).into_iter().map(|(name, bytes)| (name, bytes.into_owned())).collect();
    assert!(part(&output, "word/document.xml").contains("w:pStyle w:val=\"CustomHeading\""));
    assert!(!output.iter().any(|(name, _)| name == "word/styles.xml"));
}

#[test]
fn expands_empty_roots() {
    let template = PackageTemplate::parse([
        ("word/document.xml".into(), document(TAG).into_bytes()),
        ("word/styles.xml".into(), format!("<w:styles xmlns:w=\"{W}\"/>").into_bytes()),
        ("[Content_Types].xml".into(), b"<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"/>".to_vec()),
    ], &FillOptions::default()).unwrap();
    let output: Vec<_> = template.render(&json!({"html":"<h1>A</h1>"})).into_iter().map(|(name, bytes)| (name, bytes.into_owned())).collect();
    assert!(part(&output, "word/styles.xml").contains("w:styleId=\"Title\""));
    assert!(part(&output, "[Content_Types].xml").contains("PartName=\"/word/styles.xml\""));
}

#[test]
fn accepts_heading_tag_forms() {
    for html in ["<H1>A</H1>", "<h1\tclass='x'>A</h1>", "<h1\nclass='x'>A</h1>", "<h1/>"] {
        let output = render(html, None);
        assert!(part(&output, "word/document.xml").contains("w:pStyle w:val=\"Title\""));
        assert!(part(&output, "word/styles.xml").contains("w:styleId=\"Title\""));
    }
}

#[test]
fn accepts_default_style_types() {
    let styles = format!("<w:styles xmlns:w=\"{W}\"><w:style w:styleId=\"Title\"><w:name w:val=\"Title\"/><w:rPr><w:sz w:val=\"72\"/></w:rPr></w:style><w:style w:default=\"1\" w:styleId=\"Body\"><w:name w:val=\"Normal\"/></w:style></w:styles>");
    let output = render("<h1>A</h1><h2>B</h2>", Some(&styles));
    assert!(part(&output, "word/document.xml").contains("w:pStyle w:val=\"Title\""));
    let styles = part(&output, "word/styles.xml");
    assert_eq!(styles.matches("w:styleId=\"Title\"").count(), 1);
    assert!(styles.contains("w:sz w:val=\"72\"") && styles.contains("w:basedOn w:val=\"Body\""));
}

#[test]
fn keeps_heading_run_metadata() {
    let tag = TAG.replace("<w:sz", "<w:noProof/><w:sz").replace("</w:rPr>", "<w:lang w:val=\"fr-FR\"/></w:rPr>");
    let template = PackageTemplate::parse([("word/document.xml".into(), document(&tag).into_bytes())], &FillOptions::default()).unwrap();
    let output: Vec<_> = template.render(&json!({"html":"<h1>A</h1>"})).into_iter().map(|(name, bytes)| (name, bytes.into_owned())).collect();
    let body = part(&output, "word/document.xml");
    assert!(body.contains("<w:lang w:val=\"fr-FR\"/>") && body.contains("<w:noProof/>"));
    assert!(!body.contains("Body Font") && !body.contains("<w:sz"));
}

#[test]
fn expands_roots_with_attributes() {
    let styles = format!("<w:styles xmlns:w=\"{W}\" xmlns:meta=\"urn:metadata\" meta:note=\"a>b\"/>");
    let types = "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\" xmlns:meta=\"urn:metadata\" meta:note=\"a>b\"/>";
    let template = PackageTemplate::parse([
        ("word/document.xml".into(), document(TAG).into_bytes()),
        ("word/styles.xml".into(), styles.into_bytes()), ("[Content_Types].xml".into(), types.as_bytes().to_vec()),
    ], &FillOptions::default()).unwrap();
    let output: Vec<_> = template.render(&json!({"html":"<h1>A</h1>"})).into_iter().map(|(name, bytes)| (name, bytes.into_owned())).collect();
    assert!(part(&output, "word/styles.xml").contains("w:styleId=\"Title\""));
    assert!(part(&output, "[Content_Types].xml").contains("PartName=\"/word/styles.xml\""));
    assert!(part(&output, "word/styles.xml").contains("meta:note=\"a>b\""));
}

#[test]
fn keeps_historical_run_styles() {
    let props = "<w:rPr><w:sz w:val=\"18\"></w:sz><w:lang w:val=\"fr-FR\"/><w:rPrChange w:id=\"7\" w:author=\"Editor\"><w:rPr><w:sz w:val=\"12\"/></w:rPr></w:rPrChange></w:rPr>";
    let tag = format!("<w:p><w:r>{props}<w:t>{{~~html}}</w:t></w:r></w:p>");
    let template = PackageTemplate::parse([("word/document.xml".into(), document(&tag).into_bytes())], &FillOptions::default()).unwrap();
    let output: Vec<_> = template.render(&json!({"html":"<h1>A</h1>"})).into_iter().map(|(name, bytes)| (name, bytes.into_owned())).collect();
    let body = part(&output, "word/document.xml");
    assert!(!body.contains("w:val=\"18\""));
    assert!(body.contains("<w:rPrChange w:id=\"7\" w:author=\"Editor\"><w:rPr><w:sz w:val=\"12\"/></w:rPr></w:rPrChange>"));
    assert!(body.contains("<w:lang w:val=\"fr-FR\"/>"));
}

#[test]
fn keeps_paragraph_history() {
    for live in ["", "<w:rPr><w:sz w:val=\"18\"/></w:rPr>"] {
        let history = "<w:pPrChange w:id=\"7\" w:author=\"Editor\"><w:pPr><w:rPr><w:sz w:val=\"12\"/></w:rPr></w:pPr></w:pPrChange>";
        let tag = format!("<w:p><w:pPr>{live}{history}</w:pPr><w:r><w:t>{{~~html}}</w:t></w:r></w:p>");
        let template = PackageTemplate::parse([("word/document.xml".into(), document(&tag).into_bytes())], &FillOptions::default()).unwrap();
        let output: Vec<_> = template.render(&json!({"html":"<h1>A</h1>"})).into_iter().map(|(name, bytes)| (name, bytes.into_owned())).collect();
        let body = part(&output, "word/document.xml");
        assert!(body.contains(history));
        assert!(!body.contains("w:val=\"18\""));
    }
}
