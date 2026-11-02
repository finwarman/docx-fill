mod common;

use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::Arc;
use common::{document, paragraph};
use docx_fill::{FillHooks, FillOptions, Image, PackageTemplate};
use image::{DynamicImage, GrayImage, ImageFormat, Luma};
use qrcode::{Color, QrCode};
use serde_json::{Value, json};

struct Pictures;
impl FillHooks for Pictures {
    fn image(&mut self, _part: &str, value: &Value) -> Option<Image> {
        let key = value.as_str()?;
        Some(Image { key:key.into(), extension:"png".into(), bytes:Arc::from(key.as_bytes()), width:1.0, height:1.0 })
    }
}

fn qr_image(payload: &str) -> Vec<u8> {
    let code = QrCode::new(payload).unwrap();
    let width = code.width();
    let mut image = GrayImage::from_pixel(((width + 8) * 6) as u32, ((width + 8) * 6) as u32, Luma([255]));
    for y in 0..width {
        for x in 0..width {
            if code[(x, y)] != Color::Dark { continue; }
            for dy in 0..6 {
                for dx in 0..6 { image.put_pixel(((x + 4) * 6 + dx) as u32, ((y + 4) * 6 + dy) as u32, Luma([0])); }
            }
        }
    }
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageLuma8(image).write_to(&mut bytes, ImageFormat::Png).unwrap();
    bytes.into_inner()
}

fn picture(description: &str) -> String {
    format!("<w:p><w:r><w:drawing xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><wp:inline><wp:extent cx=\"19050\" cy=\"28575\"/><wp:docPr id=\"7\" name=\"Picture\" descr=\"{description}\"/><a:graphic><a:graphicData><a:blip r:embed=\"rIdOld\"/><a:srcRect l=\"1000\"/><a:xfrm rot=\"60000\"><a:ext cx=\"19050\" cy=\"28575\"/></a:xfrm></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>")
}

fn render(body: &str, raster: Vec<u8>, options: Value, data: Value) -> BTreeMap<String, Vec<u8>> {
    let parts = [
        ("word/document.xml".into(), document(body).into_bytes()),
        ("word/media/qr.png".into(), raster),
        ("word/_rels/document.xml.rels".into(), b"<Relationships><Relationship Id=\"rIdOld\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"media/qr.png\"/></Relationships>".to_vec()),
    ];
    let options = FillOptions::default().read_json(&options).unwrap();
    PackageTemplate::parse(parts, &options).unwrap().render_with(&data, &mut Pictures).into_iter().map(|(name, bytes)| (name, bytes.into_owned())).collect()
}

#[test]
fn replaces_qr_and_keeps_size() {
    let parts = render(&picture("Original"), qr_image("{%photo}"), json!({}), json!({"photo":"replacement"}));
    let xml = String::from_utf8_lossy(&parts["word/document.xml"]);

    assert_eq!(parts["word/media/replacements/8.png"], b"replacement");
    assert!(xml.contains("<wp:extent cx=\"19050\" cy=\"28575\"/>") && xml.contains("rot=\"60000\""));
    assert!(xml.contains("<a:srcRect l=\"1000\"/>") && !xml.contains("{%photo}"));
}

#[test]
fn resolves_each_loop_scope() {
    let body = paragraph(&["{#rows}"]) + &picture("") + &paragraph(&["{/rows}"]);
    let parts = render(&body, qr_image("{%photo}"), json!({}), json!({"rows":[{"photo":"one"},{"photo":"two"}]}));
    let images: Vec<_> = parts.iter().filter(|(path, _)| path.contains("/replacements/")).map(|(_, bytes)| bytes.as_slice()).collect();

    assert_eq!(images, [b"one", b"two"]);
}

#[test]
fn accepts_configured_tags() {
    let parts = render(&picture(""), qr_image("[[picture:photo]]"), json!({"delimiters":{"start":"[[","end":"]]"},"tagPrefixes":{"image":"picture:"}}), json!({"photo":"custom"}));
    assert_eq!(parts["word/media/replacements/8.png"], b"custom");
}

#[test]
fn respects_existing_alt_tag() {
    let parts = render(&picture("{%other}"), qr_image("{%photo}"), json!({}), json!({"photo":"wrong","other":"manual"}));
    assert_eq!(parts["word/media/replacements/8.png"], b"manual");
}

#[test]
fn ignores_disabled_or_non_tags() {
    for (raster, options) in [(qr_image("{%photo}"), json!({"extensions":{"qrCodes":false}})), (qr_image("https://example.com"), json!({})), (b"not an image".to_vec(), json!({}))] {
        let parts = render(&picture(""), raster, options, json!({"photo":"unused"}));
        assert!(!parts.keys().any(|path| path.contains("/replacements/")));
        assert!(String::from_utf8_lossy(&parts["word/document.xml"]).contains("r:embed=\"rIdOld\""));
    }
}
