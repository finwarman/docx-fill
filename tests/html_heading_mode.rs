mod common;

use common::{document, elements, paragraph_texts};
use docx_fill::{FillHooks, FillOptions, HtmlCustomization, HtmlElement, NoHooks, PackageTemplate};
use serde_json::json;

const TAG: &str = "<w:p><w:pPr><w:pStyle w:val=\"Body\"/><w:spacing w:after=\"120\"/></w:pPr><w:r><w:rPr><w:rFonts w:ascii=\"Body Font\"/><w:color w:val=\"123456\"/><w:sz w:val=\"22\"/><w:lang w:val=\"en-GB\"/></w:rPr><w:t>{~~html}</w:t></w:r></w:p>";
const STYLES: &str = "<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:style w:type=\"paragraph\" w:styleId=\"Heading1\"><w:name w:val=\"heading 1\"/><w:rPr><w:sz w:val=\"60\"/></w:rPr></w:style></w:styles>";

fn direct_options() -> FillOptions {
    FillOptions::default().read_json(&json!({"html":{"headingMode":"direct"}})).unwrap()
}

fn render_with(body: &str, html: &str, options: &FillOptions, styles: Option<&str>, hooks: &mut dyn FillHooks) -> Vec<(String, Vec<u8>)> {
    let mut parts = vec![("word/document.xml".into(), document(body).into_bytes())];
    if let Some(styles) = styles { parts.push(("word/styles.xml".into(), styles.as_bytes().to_vec())); }
    let template = PackageTemplate::parse(parts, options).unwrap();
    template.try_render_with(&json!({"html":html}), hooks).unwrap().into_iter().map(|(name, bytes)| (name, bytes.into_owned())).collect()
}

fn render(body: &str, html: &str, options: &FillOptions) -> Vec<(String, Vec<u8>)> {
    render_with(body, html, options, None, &mut NoHooks)
}

fn part<'a>(parts: &'a [(String, Vec<u8>)], name: &str) -> &'a str {
    std::str::from_utf8(&parts.iter().find(|(path, _)| path == name).unwrap().1).unwrap()
}

#[test]
fn default_uses_heading_styles() {
    let parts = render(TAG, "<h2>Heading</h2>", &FillOptions::default());
    let xml = part(&parts, "word/document.xml");

    assert!(xml.contains("<w:pStyle w:val=\"Heading1\"/>"));
    assert!(!xml.contains("Body Font"));
    assert!(part(&parts, "word/styles.xml").contains("w:styleId=\"Heading1\""));
}

#[test]
fn direct_keeps_template_format() {
    let html = "<h1>A</h1><h2>B</h2><h3>C</h3><h4>D</h4><h5>E</h5><h6>F</h6>";
    let parts = render(TAG, html, &direct_options());
    let xml = part(&parts, "word/document.xml");
    let paragraphs = elements(xml, "w:p");

    assert_eq!(paragraphs.len(), 6);
    for (paragraph, size) in paragraphs.iter().zip([32, 28, 26, 24, 24, 24]) {
        assert!(paragraph.contains("<w:pStyle w:val=\"Body\"/>") && paragraph.contains("<w:spacing w:after=\"120\"/>"));
        assert!(paragraph.contains("Body Font") && paragraph.contains("w:color w:val=\"123456\""));
        assert!(paragraph.contains(&format!("<w:sz w:val=\"{size}\"/>")) && paragraph.contains("<w:b/>"));
        assert!(paragraph.contains("w:lang w:val=\"en-GB\""));
    }
    assert!(!parts.iter().any(|(name, _)| name == "word/styles.xml"));
}

#[test]
fn direct_leaves_existing_styles() {
    let parts = render_with(TAG, "<h2>Heading</h2>", &direct_options(), Some(STYLES), &mut NoHooks);

    assert_eq!(part(&parts, "word/styles.xml"), STYLES);
    assert!(!part(&parts, "word/document.xml").contains("w:val=\"Heading1\""));
}

#[test]
fn direct_does_not_add_styles() {
    let body = TAG.replace("w:val=\"Body\"", "w:val=\"Title\"");
    let parts = render(&body, "<h2>Heading</h2>", &direct_options());

    assert!(!parts.iter().any(|(name, _)| name == "word/styles.xml"));
}

#[test]
fn direct_preserves_surroundings() {
    let mut options = direct_options();
    options.html.on_invalid_block = docx_fill::HtmlBlockMode::Preserve;
    let body = TAG.replace("{~~html}", "before {~html} after");
    let parts = render(&body, "<h2>Heading</h2>", &options);
    let xml = part(&parts, "word/document.xml");

    assert_eq!(paragraph_texts(xml), ["before ", "Heading", " after"]);
    assert!(elements(xml, "w:p")[1].contains("<w:sz w:val=\"28\"/>"));
    assert!(!parts.iter().any(|(name, _)| name == "word/styles.xml"));
}

#[test]
fn direct_covers_nested_tables() {
    let html = "<table><tr><td><h2>Outer</h2><table><tr><td><h3>Inner</h3></td></tr></table></td></tr></table>";
    let parts = render(TAG, html, &direct_options());
    let xml = part(&parts, "word/document.xml");

    assert_eq!(xml.matches("<w:tbl>").count(), 2);
    assert!(xml.contains("<w:sz w:val=\"28\"/>") && xml.contains("<w:sz w:val=\"26\"/>"));
    assert!(!xml.contains("<w:pStyle"));
    assert!(!parts.iter().any(|(name, _)| name == "word/styles.xml"));
}

#[test]
fn direct_keeps_explicit_css() {
    let parts = render(TAG, "<h2 style='font-family:Explicit;font-size:9pt;font-weight:normal;color:red'>A<i>B</i></h2>", &direct_options());
    let xml = part(&parts, "word/document.xml");

    for run in elements(xml, "w:r") {
        assert!(run.contains("w:ascii=\"Explicit\"") && run.contains("<w:sz w:val=\"18\"/>"));
        assert!(run.contains("<w:b w:val=\"0\"/>") && run.contains("w:color w:val=\"FF0000\""));
    }
    assert!(xml.contains("<w:i/>"));
}

#[test]
fn direct_keeps_custom_styles() {
    struct Custom;
    impl FillHooks for Custom {
        fn html_style_transform(&mut self, element: &HtmlElement<'_>) -> Option<String> {
            matches!(element.name(), "h2" | "h3").then(|| "TransformedHeading".into())
        }
        fn html_customize(&mut self, element: &HtmlElement<'_>) -> HtmlCustomization {
            if element.name() == "h2" { HtmlCustomization::default().paragraph_style("CustomHeading").css("font-size:10pt") }
            else { HtmlCustomization::default() }
        }
    }
    let parts = render_with(TAG, "<h2>Heading</h2><h3>Transformed</h3>", &direct_options(), None, &mut Custom);
    let xml = part(&parts, "word/document.xml");

    assert!(xml.contains("<w:pStyle w:val=\"CustomHeading\"/>") && xml.contains("<w:sz w:val=\"20\"/>"));
    assert!(xml.contains("<w:pStyle w:val=\"TransformedHeading\"/>"));
    assert!(!xml.contains("Body Font") && !xml.contains("<w:sz w:val=\"28\"/>"));
    assert!(!parts.iter().any(|(name, _)| name == "word/styles.xml"));
}
