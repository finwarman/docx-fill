#![cfg(feature = "svg-preview")]

mod common;

use std::sync::Arc;
use common::document;
use docx_fill::{FillHooks, FillOptions, Image, PackageTemplate};
use serde_json::{Value, json};

struct Svg(String);
impl FillHooks for Svg {
    fn image(&mut self, _: &str, _: &Value) -> Option<Image> {
        Some(Image { key:"preview".into(), extension:"svg".into(), bytes:Arc::from(self.0.as_bytes()), width:20.0, height:10.0 })
    }
}

fn previews(svg: String) -> bool {
    let parts = [("word/document.xml".into(), document("<w:p><w:r><w:t>{%icon}</w:t></w:r></w:p>").into_bytes()), ("[Content_Types].xml".into(), b"<Types></Types>".to_vec())];
    let template = PackageTemplate::parse(parts, &FillOptions::default()).unwrap();
    let out = template.render_with(&json!({"icon":"x"}), &mut Svg(svg));
    out.iter().any(|(name, _)| name.ends_with(".png"))
}

// Tiny DAG of `use` that expands to `fanout^levels` copies of `leaf`.
fn fan(leaf: &str, levels: usize, fanout: usize) -> String {
    let mut defs = format!(r#"<g id="n0">{leaf}</g>"#);
    for level in 1..=levels {
        defs.push_str(&format!(r#"<g id="n{level}">"#));
        for _ in 0..fanout { defs.push_str(&format!(r##"<use href="#n{}"/>"##, level - 1)); }
        defs.push_str("</g>");
    }
    format!(r##"<defs>{defs}</defs><use href="#n{levels}"/>"##)
}

#[test]
fn bounds_path_segments_across_uses() {
    let mut d = String::from("M0 0");
    for _ in 0..2000 { d.push_str(" L2000 1000 L0 1000 L2000 0"); }
    let content = fan(&format!(r#"<path d="{d}" fill="red"/>"#), 3, 10);
    let svg = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="2000" height="1000">{content}</svg>"#);
    assert!(!previews(svg));
}

#[test]
fn bounds_nested_svg_node_expansion() {
    let inner = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">{}</svg>"#, fan(r#"<rect width="1" height="1"/>"#, 4, 10));
    let encoded = inner.bytes().map(|byte| format!("%{byte:02X}")).collect::<String>();
    let image = format!(r#"<image href="data:image/svg+xml,{encoded}" width="10" height="10"/>"#);
    let content = fan(&image, 3, 10);
    let svg = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10">{content}</svg>"#);
    assert!(!previews(svg));
}

#[test]
fn bounds_text_length() {
    let text = "W ".repeat(100_000);
    let svg = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100"><text y="50">{text}</text></svg>"#);
    assert!(!previews(svg));
}

fn nested_plain(inner: &str) -> String {
    let encoded = inner.bytes().map(|byte| format!("%{byte:02X}")).collect::<String>();
    format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><image href="data:text/plain,{encoded}" width="20" height="10"/></svg>"#)
}

// A comment whose bytes 4..6 are E0 A5 makes imagesize read the SVG as Aseprite.
const ASEPRITE_COMMENT: &str = "<!--\u{0940}abcd-->";

#[test]
fn validates_svg_that_sniffs_as_another_image() {
    let unsafe_inner = r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><foreignObject/><rect width="20" height="10" fill="red"/></svg>"#;
    assert!(!previews(nested_plain(unsafe_inner)));
    assert!(!previews(nested_plain(&format!("{ASEPRITE_COMMENT}{unsafe_inner}"))));
}

#[test]
fn sizes_preview_to_inserted_extent() {
    struct Icon;
    impl FillHooks for Icon {
        fn image(&mut self, _: &str, _: &Value) -> Option<Image> {
            let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 2 1"><rect width="2" height="1" fill="red"/></svg>"#;
            Some(Image { key:"icon".into(), extension:"svg".into(), bytes:Arc::from(svg.as_bytes()), width:200.0, height:100.0 })
        }
    }
    let parts = [("word/document.xml".into(), document("<w:p><w:r><w:t>{%icon}</w:t></w:r></w:p>").into_bytes()), ("[Content_Types].xml".into(), b"<Types></Types>".to_vec())];
    let out = PackageTemplate::parse(parts, &FillOptions::default()).unwrap().render_with(&json!({"icon":"x"}), &mut Icon)
        .into_iter().map(|(name, bytes)| (name, bytes.into_owned())).collect::<std::collections::BTreeMap<_, _>>();
    let png = out.iter().find(|(name, _)| name.ends_with(".png")).unwrap().1;
    let pixels = resvg::tiny_skia::Pixmap::decode_png(png).unwrap();
    assert!(pixels.width() >= 200 && pixels.height() >= 100, "{}x{}", pixels.width(), pixels.height());
}

#[test]
fn rejects_paint_cycles_in_svg_that_sniffs_as_another_image() {
    let inner = format!(r#"{ASEPRITE_COMMENT}<svg xmlns="http://www.w3.org/2000/svg" width="40" height="30"><g fill="url(#p)"><defs><pattern id="p" width="4" height="4" patternUnits="userSpaceOnUse"><rect width="4" height="4"/></pattern></defs><rect width="40" height="30"/></g></svg>"#);
    assert!(!previews(nested_plain(&inner)));
}
