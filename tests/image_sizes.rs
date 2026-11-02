use std::sync::Arc;

use docx_fill::{FillHooks, FillOptions, Image, ImageLength, ImageSize, PackageTemplate};
use serde_json::{Value, json};

const SECTION: &str = "<w:sectPr><w:pgSz w:w=\"4320\" w:h=\"4320\"/><w:pgMar w:top=\"0\" w:right=\"0\" w:bottom=\"0\" w:left=\"0\"/></w:sectPr>";

struct SizedImage(Option<ImageSize>);

impl FillHooks for SizedImage {
    fn image(&mut self, _part: &str, _value: &Value) -> Option<Image> {
        Some(Image { key: "same".into(), bytes: Arc::from(&[1, 2, 3][..]), extension: "png".into(), width: 2.0, height: 1.0 })
    }

    fn image_size(&mut self, _part: &str, value: &Value, _image: &Image) -> Option<ImageSize> {
        value.as_f64().and_then(|width| ImageSize::pixels(width, 20.0)).or(self.0)
    }
}

fn render(body: &str, size: Option<ImageSize>, data: Value) -> Vec<(String, Vec<u8>)> {
    let document = format!("<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\"><w:body>{body}{SECTION}</w:body></w:document>");
    let template = PackageTemplate::parse([("word/document.xml".to_string(), document.into_bytes())], &FillOptions::default()).unwrap();
    template.render_with(&data, &mut SizedImage(size)).into_iter().map(|(name, bytes)| (name, bytes.into_owned())).collect()
}

fn paragraph(tag: &str) -> String {
    format!("<w:p><w:r><w:t>{tag}</w:t></w:r></w:p>")
}

fn document(parts: &[(String, Vec<u8>)]) -> &str {
    std::str::from_utf8(&parts.iter().find(|(name, _)| name == "word/document.xml").unwrap().1).unwrap()
}

#[test]
fn uses_exact_display_dimensions() {
    let filled = render(&paragraph("{%a}"), ImageSize::pixels(100.0, 50.0), json!({"a": true}));

    assert!(document(&filled).contains("<wp:extent cx=\"952500\" cy=\"476250\"/>"));
}

#[test]
fn converts_physical_units() {
    let widths = [ImageLength::Pixels(96.0), ImageLength::Inches(1.0), ImageLength::Centimeters(2.54), ImageLength::Millimeters(25.4), ImageLength::Points(72.0), ImageLength::Picas(6.0), ImageLength::Emus(914400.0)];
    for width in widths {
        let size = ImageSize::new(width, ImageLength::Points(36.0));
        let filled = render(&paragraph("{%a}"), size, json!({"a": true}));
        assert!(document(&filled).contains("<wp:extent cx=\"914400\" cy=\"457200\"/>"));
    }
}

#[test]
fn keeps_default_fitting() {
    let filled = render(&paragraph("{%a}"), None, json!({"a": true}));

    assert!(document(&filled).contains("<wp:extent cx=\"2606040\" cy=\"1303020\"/>"));
}

#[test]
fn does_not_refit_explicit_size() {
    let body = format!("<w:tbl><w:tr><w:tc><w:tcPr><w:tcW w:w=\"1440\" w:type=\"dxa\"/></w:tcPr>{}</w:tc></w:tr></w:tbl>", paragraph("{%%a}"));
    let filled = render(&body, ImageSize::pixels(400.0, 600.0), json!({"a": true}));

    assert!(document(&filled).contains("<wp:extent cx=\"3810000\" cy=\"5715000\"/>"));
    assert!(document(&filled).contains("<w:jc w:val=\"center\"/>"));
}

#[test]
fn shares_bytes_across_sizes() {
    let filled = render(&(paragraph("{%a}") + &paragraph("{%b}")), None, json!({"a": 80, "b": 120}));

    assert!(document(&filled).contains("<wp:extent cx=\"762000\" cy=\"190500\"/>"));
    assert!(document(&filled).contains("<wp:extent cx=\"1143000\" cy=\"190500\"/>"));
    assert_eq!(filled.iter().filter(|(name, _)| name.starts_with("word/media/")).count(), 1);
}

#[test]
fn rejects_invalid_dimensions() {
    for bad in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::MAX] {
        assert!(ImageSize::pixels(bad, 1.0).is_none());
        assert!(ImageSize::pixels(1.0, bad).is_none());
    }
}
