mod common;

use std::collections::BTreeMap;
use std::sync::Arc;
use common::{document, text};
use docx_fill::{FillHooks, FillOptions, Image, ImageProperties, ImageSize, NoHooks, PackageTemplate};
use serde_json::{Value, json};

const PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";

fn render(html: &str, options: &FillOptions, hooks: &mut dyn FillHooks) -> BTreeMap<String, Vec<u8>> {
    let template = PackageTemplate::parse([
        ("word/document.xml".into(), document("<w:p><w:r><w:t>{~~html}</w:t></w:r></w:p>").into_bytes()),
        ("[Content_Types].xml".into(), b"<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"/>".to_vec()),
    ], options).unwrap();
    template.try_render_with(&json!({"html":html}), hooks).unwrap().into_iter().map(|(name, bytes)| (name, bytes.into_owned())).collect()
}

#[test]
fn embeds_data_with_dimensions() {
    let parts = render(&format!("<p>before<img src='{PNG}' width='20' height='10' alt='A &amp; B' title='Icon'>after</p>"), &FillOptions::default(), &mut NoHooks);
    let xml = String::from_utf8_lossy(&parts["word/document.xml"]);
    let media = parts.iter().find(|(name, _)| name.starts_with("word/media/")).unwrap();
    let rels = String::from_utf8_lossy(&parts["word/_rels/document.xml.rels"]);

    assert_eq!(text(&xml), "beforeafter");
    assert!(xml.contains("cx=\"190500\" cy=\"95250\""));
    assert!(xml.contains("descr=\"A &amp; B\"") && xml.contains("name=\"Icon\""));
    assert!(xml.contains("xmlns:wp=") && xml.contains("xmlns:r="));
    assert!(media.1.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert!(rels.contains(media.0.strip_prefix("word/").unwrap()));
    assert!(String::from_utf8_lossy(&parts["[Content_Types].xml"]).contains("image/png"));
}

#[test]
fn sizes_and_reuses_sources() {
    let options = FillOptions::default().read_json(&json!({"htmlStyleSheet":"img {width:30px!important;height:15px} img + span {font-weight:bold}"})).unwrap();
    let parts = render(&format!("<p><img src='{PNG}' width='5' style='width:10px'><img src='{PNG}'><span>after</span></p>"), &options, &mut NoHooks);
    let xml = String::from_utf8_lossy(&parts["word/document.xml"]);

    assert_eq!(xml.matches("cx=\"285750\" cy=\"142875\"").count(), 4);
    assert_eq!(parts.keys().filter(|name| name.starts_with("word/media/")).count(), 1);
    assert!(xml.contains("<w:b/>"));
    assert!(xml.contains("wp:docPr id=\"1\"") && xml.contains("wp:docPr id=\"2\""));
}

struct Pictures;
impl FillHooks for Pictures {
    fn image(&mut self, _: &str, value: &Value) -> Option<Image> {
        (value == "local:photo").then(|| Image { key: "external".into(), bytes: Arc::from([1_u8]), extension: "png".into(), width: 4.0, height: 2.0 })
    }
    fn image_size(&mut self, _: &str, value: &Value, _: &Image) -> Option<ImageSize> {
        (value == "local:photo").then(|| ImageSize::pixels(12.0, 6.0).unwrap())
    }
    fn image_properties(&mut self, _: &str, value: &Value, _: &Image) -> ImageProperties {
        if value == "local:photo" { ImageProperties::default().alt("Hook") } else { ImageProperties::default() }
    }
    fn svg_fallback(&mut self, _: &str, _: &Value, _: &Image) -> Option<Arc<[u8]>> { Some(Arc::from([2_u8])) }
}

#[test]
fn resolves_image_hooks() {
    let parts = render("<p><img src='local:photo' width='99' alt='HTML'></p>", &FillOptions::default(), &mut Pictures);
    let xml = String::from_utf8_lossy(&parts["word/document.xml"]);

    assert!(xml.contains("cx=\"114300\" cy=\"57150\"") && xml.contains("descr=\"Hook\""));
    assert!(parts.contains_key("word/media/docx_fill_external.png"));
}

#[test]
fn embeds_svg_preview() {
    let html = "<p>before<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 8 4' width='16'><path d='M0 0h8v4z'/></svg>after</p>";
    let parts = render(html, &FillOptions::default(), &mut Pictures);
    let xml = String::from_utf8_lossy(&parts["word/document.xml"]);

    assert_eq!(text(&xml), "beforeafter");
    assert!(xml.contains("cx=\"152400\" cy=\"76200\""));
    assert!(xml.contains("asvg:svgBlip"));
    assert!(parts.keys().any(|name| name.ends_with(".svg")) && parts.keys().any(|name| name.contains("svg_fallback/") && name.ends_with(".png")));
}

#[test]
fn image_links_and_intrinsic_size() {
    let parts = render(&format!("<a href='https://example.com'><img src='{PNG}'></a>"), &FillOptions::default(), &mut NoHooks);
    let xml = String::from_utf8_lossy(&parts["word/document.xml"]);

    assert!(xml.contains("cx=\"9525\" cy=\"9525\""));
    assert!(xml.contains("<w:hyperlink") && xml.contains("</w:drawing></w:r></w:hyperlink>"));
    assert!(String::from_utf8_lossy(&parts["word/_rels/document.xml.rels"]).contains("https://example.com"));
}

#[test]
fn ignores_disabled_images() {
    let mut options = FillOptions::default();
    options.extensions.images = false;
    let parts = render("<p>A<img src='unresolved'><svg><unknown/></svg>B</p>", &options, &mut NoHooks);
    assert_eq!(text(&String::from_utf8_lossy(&parts["word/document.xml"])), "AB");
    assert!(!parts.keys().any(|name| name.starts_with("word/media/")));
}

#[test]
fn rejects_bad_image_data() {
    let template = PackageTemplate::parse([("word/document.xml".into(), document("<w:p><w:r><w:t>{~~html}</w:t></w:r></w:p>").into_bytes())], &FillOptions::default()).unwrap();
    for source in ["data:image/png;base64,?", "data:image/png;base64,AA==", "data:image/svg+xml,%GG", "https://example.com/a.png"] {
        assert!(template.try_render(&json!({"html":format!("<img src='{source}'>")})).is_err(), "{source}");
    }
}

#[test]
fn gif_and_encoded_svg_sources() {
    let gif = "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";
    let svg = "data:image/svg+xml,%3Csvg%20xmlns='http://www.w3.org/2000/svg'%20viewBox='0%200%208%204'%3E%3Cpath%20d='M0%200h8v4z'/%3E%3C/svg%3E";
    let parts = render(&format!("<p><img src=\"{gif}\"><img width='20' src=\"{svg}\"></p>"), &FillOptions::default(), &mut NoHooks);
    let xml = String::from_utf8_lossy(&parts["word/document.xml"]);

    assert!(xml.contains("cx=\"9525\" cy=\"9525\"") && xml.contains("cx=\"190500\" cy=\"95250\""));
    assert!(parts.keys().any(|name| name.ends_with(".gif")) && parts.keys().any(|name| name.ends_with(".svg")));
}

#[test]
fn reads_sources_as_browsers_do() {
    let unpadded = PNG.trim_end_matches('=').replacen("data:", "DATA:", 1);
    let parts = render(&format!("<p><img src='{unpadded}'><svg width='8' height='4'><path d='M0 0h8v4z'/></svg></p>"), &FillOptions::default(), &mut NoHooks);
    let svg = parts.iter().find(|(name, _)| name.ends_with(".svg")).unwrap().1;

    assert_eq!(parts.keys().filter(|name| name.starts_with("word/media/") && !name.contains("/svg_fallback/")).count(), 2);
    assert!(String::from_utf8_lossy(svg).starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
}

#[test]
fn fits_unsized_images_to_the_page() {
    let parts = render("<svg viewBox='0 0 4000 2000'><path d='M0 0h8v4z'/></svg>", &FillOptions::default(), &mut NoHooks);
    let xml = String::from_utf8_lossy(&parts["word/document.xml"]);
    let width: i64 = xml.split("<wp:extent cx=\"").nth(1).unwrap().split('"').next().unwrap().parse().unwrap();

    assert!(width < 4000 * 9525, "{width}");
}

#[test]
fn embeds_images_without_extras() {
    let mut options = FillOptions::default();
    options.extensions.html_extras = false;
    let parts = render(&format!("<p>A<img src='{PNG}'>B</p>"), &options, &mut NoHooks);
    let xml = String::from_utf8_lossy(&parts["word/document.xml"]);

    assert_eq!(text(&xml), "AB");
    assert!(xml.contains("<w:drawing"));
}
