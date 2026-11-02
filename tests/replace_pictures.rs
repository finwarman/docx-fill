mod common;

use std::collections::BTreeMap;
use std::sync::Arc;
use common::{document, paragraph};
use docx_fill::{FillHooks, FillOptions, Image, ImageSize, PackageTemplate};
use serde_json::{Value, json};

struct Pictures;
impl FillHooks for Pictures {
    fn image(&mut self, _part: &str, value: &Value) -> Option<Image> {
        let key = value.as_str()?;
        Some(Image { key:key.into(), extension:"png".into(), bytes:Arc::from(key.as_bytes()), width:4.0, height:1.0 })
    }
    fn image_size(&mut self, _part: &str, value: &Value, _image: &Image) -> Option<ImageSize> {
        (value == "resize").then(|| ImageSize::pixels(15.0, 20.0).unwrap())
    }
}

fn picture(tag: &str) -> String {
    format!("<w:p><w:r><w:drawing xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><wp:inline distT=\"7\"><wp:extent cx=\"19050\" cy=\"28575\"/><wp:docPr id=\"7\" name=\"Picture\" descr=\"Before {tag} after\"/><a:graphic><a:graphicData><a:blip r:embed=\"rIdOld\"/><a:srcRect l=\"1000\"/><a:xfrm rot=\"60000\"><a:ext cx=\"19050\" cy=\"28575\"/></a:xfrm></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>")
}

fn render(body: &str, options: Value, data: Value) -> BTreeMap<String, Vec<u8>> {
    let parts = [
        ("word/document.xml".into(), document(body).into_bytes()),
        ("word/media/old.png".into(), b"old".to_vec()),
        ("word/_rels/document.xml.rels".into(), b"<Relationships><Relationship Id=\"rIdOld\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"media/old.png\"/></Relationships>".to_vec()),
    ];
    let options = FillOptions::default().read_json(&options).unwrap();
    PackageTemplate::parse(parts, &options).unwrap().render_with(&data, &mut Pictures).into_iter().map(|(name, bytes)| (name, bytes.into_owned())).collect()
}

#[test]
fn replaces_bytes_and_keeps_layout() {
    let parts = render(&picture("{%photo}"), json!({}), json!({"photo":"new"}));
    let xml = String::from_utf8_lossy(&parts["word/document.xml"]);

    assert!(xml.contains("rIdDocxFillReplacement_8") && !xml.contains("r:embed=\"rIdOld\""));
    assert!(xml.contains("<wp:extent cx=\"19050\" cy=\"28575\"/>") && xml.contains("distT=\"7\""));
    assert!(xml.contains("<a:srcRect l=\"1000\"/>") && xml.contains("rot=\"60000\""));
    assert!(xml.contains("descr=\"Before  after\"") && !xml.contains("{%photo}"));
    assert_eq!(parts["word/media/replacements/8.png"], b"new");
}

#[test]
fn resolves_tags_in_each_loop_item() {
    let body = paragraph(&["{#rows}"]) + &picture("{%photo}") + &paragraph(&["{/rows}"]);
    let parts = render(&body, json!({}), json!({"photo":"wrong","rows":[{"photo":"A"},{"photo":"B"}]}));
    let replacements: Vec<_> = parts.iter().filter(|(name, _)| name.contains("/replacements/")).map(|(_, bytes)| bytes.as_slice()).collect();

    assert_eq!(replacements, [b"A", b"B"]);
}

#[test]
fn accepts_prefixes_and_expressions() {
    let parts = render(&picture("[[picture:photo.path]]"), json!({"delimiters":{"start":"[[","end":"]]"},"tagPrefixes":{"image":"picture:"},"expressions":true}), json!({"photo":{"path":"nested"}}));

    assert_eq!(parts["word/media/replacements/8.png"], b"nested");
}

#[test]
fn respects_explicit_image_size() {
    let parts = render(&picture("{%photo}"), json!({}), json!({"photo":"resize"}));
    let xml = String::from_utf8_lossy(&parts["word/document.xml"]);

    assert!(xml.contains("<wp:extent cx=\"142875\" cy=\"190500\"/>"));
    assert!(xml.contains("<a:ext cx=\"142875\" cy=\"190500\"/>"));
}

#[test]
fn keeps_picture_without_a_value() {
    let parts = render(&picture("{%photo}"), json!({}), json!({}));
    let xml = String::from_utf8_lossy(&parts["word/document.xml"]);

    assert!(xml.contains("r:embed=\"rIdOld\""));
    assert!(!parts.keys().any(|name| name.contains("/replacements/")));
}
