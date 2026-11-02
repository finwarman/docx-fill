#![cfg(feature = "svg-preview")]

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;
use common::document;
use docx_fill::{FillHooks, FillOptions, Image, PackageTemplate};
use serde_json::{Value, json};

const RED: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="20" height="10" fill="red"/></svg>"#;
struct Svg<'a>(&'a str);
impl FillHooks for Svg<'_> {
    fn image(&mut self, _: &str, _: &Value) -> Option<Image> {
        Some(Image { key:"preview".into(), extension:"svg".into(), bytes:Arc::from(self.0.as_bytes()), width:20.0, height:10.0 })
    }
}

fn render(body: &str, data: Value, hooks: &mut dyn FillHooks) -> BTreeMap<String, Vec<u8>> {
    let parts = [("word/document.xml".into(), document(body).into_bytes()), ("[Content_Types].xml".into(), b"<Types></Types>".to_vec())];
    PackageTemplate::parse(parts, &FillOptions::default()).unwrap().render_with(&data, hooks)
        .into_iter().map(|(name, bytes)| (name, bytes.into_owned())).collect()
}

fn preview(parts: &BTreeMap<String, Vec<u8>>) -> Option<resvg::tiny_skia::Pixmap> {
    parts.iter().find(|(name, _)| name.ends_with(".png"))
        .map(|(_, bytes)| resvg::tiny_skia::Pixmap::decode_png(bytes).unwrap())
}

#[test]
fn previews_image_tags() {
    let parts = render("<w:p><w:r><w:t>{%icon}</w:t></w:r></w:p>", json!({"icon":"red"}), &mut Svg(RED));
    let pixels = preview(&parts).unwrap();

    assert_eq!((pixels.width(), pixels.height()), (20, 10));
    let pixel = pixels.pixel(10, 5).unwrap();
    assert_eq!((pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()), (255, 0, 0, 255));
    assert!(String::from_utf8_lossy(&parts["word/document.xml"]).contains("asvg:svgBlip"));
    assert_eq!(parts["word/media/docx_fill_preview.svg"], RED.as_bytes());
}

#[test]
fn previews_html_and_replacements() {
    let body = "<w:p><w:r><w:t>{~~html}</w:t></w:r></w:p>";
    assert!(preview(&render(body, json!({"html":RED}), &mut Svg(RED))).is_some());
    let body = r#"<w:p><w:r><w:drawing><wp:inline><wp:extent cx="190500" cy="95250"/><wp:docPr id="7" descr="{%icon}"/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="old"/></pic:blipFill><pic:spPr><a:xfrm><a:ext cx="190500" cy="95250"/></a:xfrm></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#;
    let parts = render(body, json!({"icon":"red"}), &mut Svg(RED));
    assert!(preview(&parts).is_some());
    let xml = String::from_utf8_lossy(&parts["word/document.xml"]);
    assert!(xml.contains("asvg:svgBlip") && xml.contains("rIdDocxFillReplacementFallback_"));
    assert!(xml.contains("cx=\"190500\" cy=\"95250\""));
}

#[test]
fn previews_text_and_caps_size() {
    let text = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="30"><text x="2" y="22">Example</text></svg>"#;
    let pixels = preview(&render("<w:p><w:r><w:t>{%icon}</w:t></w:r></w:p>", json!({"icon":"text"}), &mut Svg(text))).unwrap();
    assert!(pixels.pixels().iter().any(|pixel| pixel.alpha() > 0));
    let large = RED.replace("width=\"20\"", "width=\"1000000\"");
    let pixels = preview(&render("<w:p><w:r><w:t>{%icon}</w:t></w:r></w:p>", json!({"icon":"large"}), &mut Svg(&large))).unwrap();
    assert_eq!(pixels.width(), 4096);
    assert!(pixels.height() >= 1 && pixels.height() <= 4096);
}

#[test]
fn keeps_overrides_and_bad_svg() {
    struct Native;
    impl FillHooks for Native {
        fn image(&mut self, part: &str, value: &Value) -> Option<Image> { Svg(RED).image(part, value) }
        fn svg_fallback(&mut self, _: &str, _: &Value, _: &Image) -> Option<Arc<[u8]>> { None }
    }
    let body = "<w:p><w:r><w:t>{%icon}</w:t></w:r></w:p>";
    assert!(preview(&render(body, json!({"icon":"native"}), &mut Native)).is_none());
    for svg in ["<svg", r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><image href="file:///unused.png" width="20" height="10"/></svg>"#, r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><foreignObject><div>HTML</div></foreignObject></svg>"#] {
        assert!(preview(&render(body, json!({"icon":"invalid"}), &mut Svg(svg))).is_none());
    }
}

#[test]
fn bounds_nested_svg_resources() {
    let body = "<w:p><w:r><w:t>{%icon}</w:t></w:r></w:p>";
    for (inner, accepted) in [(RED, true), (r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><image href="file:///unused.png"/></svg>"#, false), (r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><foreignObject/></svg>"#, false)] {
        let encoded = inner.bytes().map(|byte| format!("%{byte:02X}")).collect::<String>();
        let svg = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><image href="data:image/svg+xml,{encoded}" width="20" height="10"/></svg>"#);
        assert_eq!(preview(&render(body, json!({"icon":"nested"}), &mut Svg(&svg))).is_some(), accepted);
    }
}

#[test]
fn bounds_render_surfaces() {
    let body = "<w:p><w:r><w:t>{%icon}</w:t></w:r></w:p>";
    for content in [r#"<defs><pattern id="p" width="100000" height="100000" patternUnits="userSpaceOnUse"><rect width="10" height="10"/></pattern></defs><rect width="20" height="10" fill="url(#p)"/>"#, r#"<g opacity="0.5"><rect width="100000" height="100000"/></g>"#] {
        let svg = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10">{content}</svg>"#);
        assert!(preview(&render(body, json!({"icon":"large"}), &mut Svg(&svg))).is_none());
    }
    let svg = RED.replace("<rect", r#"<g opacity="0.5"><rect"#).replace("</svg>", "</g></svg>");
    let pixels = preview(&render(body, json!({"icon":"opacity"}), &mut Svg(&svg))).unwrap();
    assert!((127..=128).contains(&pixels.pixel(10, 5).unwrap().alpha()));
}

#[test]
fn accepts_only_outline_fonts() {
    struct Fonts(Vec<u8>);
    impl FillHooks for Fonts {
        fn image(&mut self, part: &str, value: &Value) -> Option<Image> { Svg(RED).image(part, value) }
        fn svg_fonts(&mut self, _: &str, _: &Value) -> Vec<Arc<[u8]>> { vec![Arc::from(self.0.clone())] }
    }
    let body = "<w:p><w:r><w:t>{%icon}</w:t></w:r></w:p>";
    let font = include_bytes!("../src/images/fonts/NotoSans.ttf");
    assert!(preview(&render(body, json!({"icon":"font"}), &mut Fonts(font.to_vec()))).is_some());
    for signature in [b"SVG ", b"CBDT", b"COLR"] {
        let mut invalid = font.to_vec();
        invalid[12..16].copy_from_slice(signature);
        assert!(preview(&render(body, json!({"icon":"font"}), &mut Fonts(invalid))).is_none());
    }
    let mut collection = font.to_vec();
    collection[..4].copy_from_slice(b"ttcf");
    assert!(preview(&render(body, json!({"icon":"font"}), &mut Fonts(collection))).is_none());
}

#[test]
fn rejects_hidden_svg_resources() {
    let body = "<w:p><w:r><w:t>{%icon}</w:t></w:r></w:p>";
    let external = r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:alt="http://www.w3.org/1999/xlink" width="20" height="10"><image alt:href="file:///unused.png" width="20" height="10"/></svg>"#;
    let entity = r#"<!DOCTYPE svg [<!ENTITY tile '<pattern id="p" width="100000" height="100000" patternUnits="userSpaceOnUse"><rect width="10" height="10"/></pattern>'>]><svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><defs>&tile;</defs><rect width="20" height="10" fill="url(#p)"/></svg>"#;
    for content in [external, entity] {
        assert!(preview(&render(body, json!({"icon":"invalid"}), &mut Svg(content))).is_none());
        let encoded = [0xef, 0xbb, 0xbf].into_iter().chain(content.bytes()).map(|byte| format!("%{byte:02X}")).collect::<String>();
        let svg = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><image href="data:text/plain,{encoded}" width="20" height="10"/></svg>"#);
        assert!(preview(&render(body, json!({"icon":"nested"}), &mut Svg(&svg))).is_none());
    }
}

fn effect(content: &str) -> Option<resvg::tiny_skia::Pixmap> {
    let svg = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="30">{content}</svg>"#);
    preview(&render("<w:p><w:r><w:t>{%icon}</w:t></w:r></w:p>", json!({"icon":"effect"}), &mut Svg(&svg)))
}

#[test]
fn previews_clip_paths_and_masks() {
    let clipped = effect(r#"<defs><clipPath id="clip"><rect width="20" height="30"/></clipPath></defs><rect width="40" height="30" fill="red" clip-path="url(#clip)"/>"#).unwrap();
    assert_eq!(clipped.pixel(5, 5).unwrap().red(), 255);
    assert_eq!(clipped.pixel(30, 5).unwrap().alpha(), 0);
    let masked = effect(r#"<defs><mask id="mask" maskUnits="userSpaceOnUse" x="0" y="0" width="40" height="30"><rect width="40" height="30" fill="black"/><rect width="20" height="30" fill="white"/></mask></defs><rect width="40" height="30" fill="red" mask="url(#mask)"/>"#).unwrap();
    assert_eq!(masked.pixel(5, 5).unwrap().red(), 255);
    assert_eq!(masked.pixel(30, 5).unwrap().alpha(), 0);
}

#[test]
fn previews_patterns_and_transforms() {
    let pixels = effect(r#"<defs><pattern id="p" patternUnits="userSpaceOnUse" width="4" height="4" patternTransform="scale(2)"><rect width="2" height="4" fill="blue"/></pattern></defs><rect width="40" height="30" fill="url(#p)"/>"#).unwrap();
    assert_eq!(pixels.pixel(1, 5).unwrap().blue(), 255);
    assert_eq!(pixels.pixel(6, 5).unwrap().alpha(), 0);
}

#[test]
fn previews_blur_and_colour_filters() {
    let blurred = effect(r#"<defs><filter id="blur" x="-100%" y="-100%" width="300%" height="300%"><feGaussianBlur stdDeviation="2"/></filter></defs><rect x="10" y="10" width="12" height="10" fill="red" style="filter:url('#blur')"/>"#).unwrap();
    assert!(blurred.pixel(8, 15).unwrap().alpha() > 0);
    assert!(blurred.pixel(8, 15).unwrap().alpha() < blurred.pixel(15, 15).unwrap().alpha());
    let coloured = effect(r#"<defs><filter id="colour"><feColorMatrix type="matrix" values="0 0 1 0 0  0 1 0 0 0  1 0 0 0 0  0 0 0 1 0"/></filter></defs><rect width="40" height="30" fill="red" filter="url(#colour)"/>"#).unwrap();
    assert_eq!(coloured.pixel(20, 15).unwrap().red(), 0);
    assert_eq!(coloured.pixel(20, 15).unwrap().blue(), 255);
}

#[test]
fn previews_filter_image_content() {
    let pixels = effect(r##"<defs><g id="source"><rect width="40" height="30" fill="blue"/></g><filter id="image" x="0" y="0" width="100%" height="100%"><feImage href="#source"/></filter></defs><rect width="40" height="30" filter="url(#image)"/>"##).unwrap();
    assert_eq!(pixels.pixel(20, 15).unwrap().blue(), 255);
}

#[test]
fn rejects_recursive_effects() {
    for content in [
        r##"<defs><pattern id="a" href="#b"/><pattern id="b" href="#c"/><pattern id="c" href="#a"/></defs><rect width="40" height="30" fill="url(#a)"/>"##,
        r#"<defs><mask id="a" mask="url(#b)"><rect width="40" height="30"/></mask><mask id="b" mask="url(#c)"><rect width="40" height="30"/></mask><mask id="c" mask="url(#a)"><rect width="40" height="30"/></mask></defs><rect width="40" height="30" mask="url(#a)"/>"#,
        r#"<defs><clipPath id="a" clip-path="url(#b)"><rect width="40" height="30"/></clipPath><clipPath id="b" clip-path="url(#c)"><rect width="40" height="30"/></clipPath><clipPath id="c" clip-path="url(#a)"><rect width="40" height="30"/></clipPath></defs><rect width="40" height="30" clip-path="url(#a)"/>"#,
        r#"<g fill="url(#p)"><defs><pattern id="p" width="4" height="4" patternUnits="userSpaceOnUse"><rect width="4" height="4"/></pattern></defs><rect width="40" height="30"/></g>"#,
    ] { assert!(effect(content).is_none()); }
    let safe = r#"<g fill="url(#p)"><defs><pattern id="p" width="4" height="4" patternUnits="userSpaceOnUse"><rect width="4" height="4" fill="red"/></pattern></defs><rect width="40" height="30"/></g>"#;
    assert!(effect(safe).is_some());
}

#[test]
fn rejects_excessive_filter_work() {
    for primitive in [r#"<feConvolveMatrix order="65536 65536" kernelMatrix="1"/>"#, r#"<feTurbulence numOctaves="4294967295"/>"#, r#"<feGaussianBlur stdDeviation="100000000000000000000000"/>"#, r#"<feMorphology radius="1000000000"/>"#] {
        assert!(effect(&format!(r#"<defs><filter id="f">{primitive}</filter></defs><rect width="40" height="30" filter="url(#f)"/>"#)).is_none());
    }
    let merge = "<feMergeNode in=\"SourceGraphic\"/>".repeat(20_000);
    assert!(effect(&format!(r#"<defs><filter id="f"><feMerge>{merge}</feMerge></filter></defs><rect width="40" height="30" filter="url(#f)"/>"#)).is_none());
}

#[test]
fn bounds_expanded_references() {
    let mut content = String::from(r#"<defs><g id="n0"><rect width="1" height="1"/></g>"#);
    for index in 1..22 { content.push_str(&format!(r##"<g id="n{index}"><use href="#n{}"/><use href="#n{}"/></g>"##, index - 1, index - 1)); }
    content.push_str(r##"</defs><use href="#n21"/>"##);
    assert!(effect(&content).is_none());
}

#[test]
fn rejects_stylesheet_cycles() {
    let styled = r#"<style>rect { fill: inherit; }</style><g fill="url(#p)"><defs><pattern id="p" width="4" height="4" patternUnits="userSpaceOnUse"><rect width="4" height="4" fill="red"/></pattern></defs><rect width="40" height="30"/></g>"#;
    assert!(effect(styled).is_none());
}

#[test]
fn bounds_inline_paint_inheritance() {
    assert!(effect(include_str!("fixtures/svg/inline-inheritance.svg")).is_none());
}

#[test]
fn bounds_namespaced_filter_values() {
    assert!(effect(r#"<defs><filter id="f"><feConvolveMatrix xmlns:xlink="http://www.w3.org/1999/xlink" xlink:order="65536 65536" kernelMatrix="1"/></filter></defs><rect width="40" height="30" filter="url(#f)"/>"#).is_none());
}

#[test]
fn previews_stylesheet_resources() {
    let pixels = effect(r#"<style>.area { fill: url('#p'); } .stripe { fill: blue; }</style><defs><pattern id="p" width="4" height="4" patternUnits="userSpaceOnUse"><rect class="stripe" width="4" height="4"/></pattern></defs><rect class="area" width="40" height="30"/>"#).unwrap();
    assert_eq!(pixels.pixel(20, 15).unwrap().blue(), 255);
    let inherited = effect(r#"<style>rect { fill: inherit; }</style><g fill="red"><rect width="40" height="30"/></g>"#).unwrap();
    assert_eq!(inherited.pixel(20, 15).unwrap().red(), 255);
}

#[test]
fn previews_cdata_stylesheets() {
    let pixels = effect(r#"<style><![CDATA[.area { filter: url('#blue'); }]]></style><defs><filter id="blue"><feColorMatrix values="0 0 1 0 0 0 1 0 0 0 1 0 0 0 0 0 0 0 1 0"/></filter></defs><rect class="area" width="40" height="30" fill="red"/>"#).unwrap();
    assert_eq!(pixels.pixel(20, 15).unwrap().blue(), 255);
}

#[test]
fn honours_overridden_css_resources() {
    let pixels = effect(r#"<style>.stripe { fill:url(#p); }</style><defs><pattern id="p" width="4" height="4" patternUnits="userSpaceOnUse"><rect class="stripe" style="fill:blue" width="4" height="4"/></pattern></defs><rect width="40" height="30" fill="url(#p)"/>"#).unwrap();
    assert_eq!(pixels.pixel(20, 15).unwrap().blue(), 255);
}

#[test]
fn preserves_mixed_cdata_rules() {
    let pixels = effect(r#"<style><![CDATA[@media print {]]>.paint { fill:url('#p'); } }</style><defs><pattern id="p" width="4" height="4" patternUnits="userSpaceOnUse"><rect width="4" height="4" fill="blue"/></pattern></defs><rect class="paint" width="40" height="30" fill="red"/>"#).unwrap();
    assert_eq!(pixels.pixel(20, 15).unwrap().red(), 255);
    assert_eq!(pixels.pixel(20, 15).unwrap().blue(), 0);
}

#[test]
fn resets_unresolved_inheritance() {
    let pixels = effect(r#"<defs><pattern id="p" width="4" height="4" patternUnits="userSpaceOnUse"><rect width="4" height="4" fill="url(#p)" style="fill:inherit"/></pattern></defs><rect width="40" height="30" fill="url(#p)"/>"#).unwrap();
    let pixel = pixels.pixel(20, 15).unwrap();
    assert_eq!((pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()), (0, 0, 0, 255));
}

#[test]
fn prefers_unprefixed_references() {
    for attributes in [r##"href="#safe" xlink:href="#p""##, r##"xlink:href="#p" href="#safe""##] {
        let body = format!(r##"<defs xmlns:xlink="http://www.w3.org/1999/xlink"><rect id="safe" width="4" height="4" fill="blue"/><pattern id="p" width="4" height="4" patternUnits="userSpaceOnUse"><use {attributes}/></pattern></defs><rect width="40" height="30" fill="url(#p)"/>"##);
        assert_eq!(effect(&body).unwrap().pixel(20, 15).unwrap().blue(), 255);
    }
}
