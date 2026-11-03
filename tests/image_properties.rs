mod common;

use std::collections::BTreeMap;
use std::sync::Arc;
use common::document;
use docx_fill::{FillHooks, FillOptions, Image, ImageAlignment, ImageBorder, ImageCaption, ImageProperties, PackageTemplate};
use serde_json::{Value, json};

struct Pictures;

impl FillHooks for Pictures {
    fn image(&mut self, _part: &str, _value: &Value) -> Option<Image> {
        Some(Image { key:"photo".into(), extension:"png".into(), bytes:Arc::from([0_u8]), width:2.0, height:1.0 })
    }
    fn image_properties(&mut self, _part: &str, value: &Value, _image: &Image) -> ImageProperties {
        if value != "styled" { return ImageProperties::default(); }
        ImageProperties::default().alt("A & B").name("Photo \"one\"").align(ImageAlignment::Right)
            .border(ImageBorder::new("#713ffe", 2.0).unwrap()).link("https://example.com/?a=1&b=2")
            .caption(ImageCaption::new("A caption").prefix("Figure: ").align(ImageAlignment::Center))
    }
}

fn render(value: &str) -> BTreeMap<String, Vec<u8>> {
    let parts = [("word/document.xml".into(), document("<w:p><w:r><w:t>{%%photo}</w:t></w:r></w:p>").into_bytes())];
    PackageTemplate::parse(parts, &FillOptions::default()).unwrap().render_with(&json!({"photo":value}), &mut Pictures)
        .into_iter().map(|(name, bytes)| (name, bytes.into_owned())).collect()
}

#[test]
fn writes_metadata_border_and_link() {
    let parts = render("styled");
    let xml = String::from_utf8_lossy(&parts["word/document.xml"]);

    assert!(xml.contains("name=\"Photo &quot;one&quot;\"") && xml.contains("descr=\"A &amp; B\""));
    assert!(xml.contains("<w:jc w:val=\"right\"/>") && xml.contains("<a:ln w=\"25400\">"));
    assert!(xml.contains("<a:srgbClr val=\"713FFE\"/>") && xml.contains("<a:hlinkClick "));
    let rels = String::from_utf8_lossy(&parts["word/_rels/document.xml.rels"]);
    assert!(rels.contains("Target=\"https://example.com/?a=1&amp;b=2\""));
}

#[test]
fn adds_a_caption_paragraph() {
    let parts = render("styled");
    let xml = String::from_utf8_lossy(&parts["word/document.xml"]);

    assert!(xml.contains("<w:pStyle w:val=\"Caption\"/>") && xml.contains("<w:jc w:val=\"center\"/>"));
    assert!(xml.contains("Figure: ") && xml.contains("A caption"));
    assert_eq!(xml.matches("<w:p>").count(), 2);
}

#[test]
fn leaves_unset_properties_alone() {
    let parts = render("plain");
    let xml = String::from_utf8_lossy(&parts["word/document.xml"]);

    assert!(xml.contains("name=\"Photo 1\""));
    assert!(!xml.contains("descr=") && !xml.contains("<a:ln ") && !xml.contains("hlinkClick"));
    assert_eq!(xml.matches("<w:p>").count(), 1);
    assert!(ImageBorder::new("invalid", 2.0).is_none());
    assert!(ImageBorder::new("000000", f64::NAN).is_none());
}

#[test]
fn rejects_borders_wider_than_drawingml_allows() {
    // ST_LineWidth ends at 20116800 EMU, 1584 points.
    assert!(ImageBorder::new("000000", 1584.0).is_some());
    assert!(ImageBorder::new("000000", 1585.0).is_none());
}
