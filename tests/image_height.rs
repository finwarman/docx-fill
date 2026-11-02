mod common;

use std::sync::Arc;
use common::document;
use docx_fill::{FillHooks, FillOptions, Image, PackageTemplate};
use serde_json::{Value, json};

struct TallImage;

impl FillHooks for TallImage {
    fn image(&mut self, _part: &str, _value: &Value) -> Option<Image> {
        Some(Image { key: "tall".into(), bytes: Arc::from([0_u8]), extension: "png".into(), width: 1.0, height: 8.0 })
    }
}

#[test]
fn insets_height_limited_images() {
    let xml = document("<w:p><w:r><w:t>{%photo}</w:t></w:r></w:p><w:sectPr><w:pgSz w:w=\"12240\" w:h=\"15840\"/><w:pgMar w:top=\"1440\" w:bottom=\"1440\" w:left=\"1440\" w:right=\"1440\"/></w:sectPr>");
    let template = PackageTemplate::parse([("word/document.xml".into(), xml.into_bytes())], &FillOptions::default()).unwrap();
    let parts = template.render_with(&json!({ "photo": "tall" }), &mut TallImage);
    let (_, xml) = parts.iter().find(|(name, _)| name == "word/document.xml").unwrap();
    let xml = String::from_utf8_lossy(xml);

    // Nine inches of page height, inset by 5%, with the 1:8 ratio unchanged.
    assert!(xml.contains("<wp:extent cx=\"977265\" cy=\"7818120\"/>"), "{xml}");
}
