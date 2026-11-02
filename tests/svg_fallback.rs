mod common;

use std::collections::BTreeMap;
use std::sync::Arc;
use common::document;
use docx_fill::{FillHooks, FillOptions, Image, ImageSize, PackageTemplate};
use serde_json::{Value, json};

struct Svg;

impl FillHooks for Svg {
    fn image(&mut self, _part: &str, _value: &Value) -> Option<Image> {
        Some(Image { key:"icon".into(), extension:"svg".into(), bytes:Arc::from(b"<svg/>".as_slice()), width:2.0, height:1.0 })
    }

    fn image_size(&mut self, _part: &str, _value: &Value, _image: &Image) -> Option<ImageSize> {
        ImageSize::pixels(32.0, 16.0)
    }

    fn svg_fallback(&mut self, _part: &str, value: &Value, _image: &Image) -> Option<Arc<[u8]>> {
        (value == "preview").then(|| Arc::from(b"PNG preview".as_slice()))
    }
}

fn render(value: &str) -> BTreeMap<String, Vec<u8>> {
    let parts = [
        ("word/document.xml".into(), document("<w:p><w:r><w:t>{%icon}{%icon}</w:t></w:r></w:p>").into_bytes()),
        ("[Content_Types].xml".into(), b"<Types></Types>".to_vec()),
    ];
    let template = PackageTemplate::parse(parts, &FillOptions::default()).unwrap();
    template.render_with(&json!({"icon":value}), &mut Svg).into_iter().map(|(name, bytes)| (name, bytes.into_owned())).collect()
}

#[test]
fn embeds_svg_with_png_preview() {
    let parts = render("preview");
    let xml = String::from_utf8_lossy(&parts["word/document.xml"]);

    assert!(xml.contains("<a:blip r:embed=\"rIdDocxFillSvgFallback_icon\">"));
    assert!(xml.contains("<asvg:svgBlip xmlns:asvg=\"http://schemas.microsoft.com/office/drawing/2016/SVG/main\" r:embed=\"rIdDocxFill_icon\"/>"));
    assert!(xml.contains("<wp:extent cx=\"304800\" cy=\"152400\"/>"));
    assert_eq!(parts["word/media/docx_fill_icon.svg"], b"<svg/>");
    assert_eq!(parts["word/media/svg_fallback/docx_fill_icon.png"], b"PNG preview");
    let rels = String::from_utf8_lossy(&parts["word/_rels/document.xml.rels"]);
    assert_eq!(rels.matches("Id=\"rIdDocxFillSvgFallback_icon\"").count(), 1);
    assert!(String::from_utf8_lossy(&parts["[Content_Types].xml"]).contains("image/svg+xml"));
}

#[test]
fn keeps_svg_only_without_preview() {
    let parts = render("none");
    let xml = String::from_utf8_lossy(&parts["word/document.xml"]);

    assert!(xml.contains("<a:blip r:embed=\"rIdDocxFill_icon\"/>"));
    assert!(!xml.contains("svgBlip"));
    assert!(!parts.keys().any(|name| name.ends_with(".png")));
}
