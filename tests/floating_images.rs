mod common;

use std::sync::Arc;
use common::document;
use docx_fill::{FillHooks, FillOptions, Image, ImageFloat, ImageLayer, ImageLength, ImageProperties, ImageRelative, ImageWrap, PackageTemplate};
use serde_json::{Value, json};

struct Picture(ImageWrap);
impl FillHooks for Picture {
    fn image(&mut self, _part: &str, _value: &Value) -> Option<Image> {
        Some(Image { key:"picture".into(), extension:"png".into(), bytes:Arc::from([0_u8]), width:1.0, height:1.0 })
    }
    fn image_properties(&mut self, _part: &str, _value: &Value, _image: &Image) -> ImageProperties {
        let floating = ImageFloat::new(ImageLength::Inches(1.0), ImageLength::Pixels(-20.0)).unwrap()
            .relative(ImageRelative::Margin, ImageRelative::Paragraph).unwrap().layer(ImageLayer::Behind).wrap(self.0)
            .margins(ImageLength::Points(2.0), ImageLength::Points(2.0), ImageLength::Points(2.0), ImageLength::Points(2.0)).unwrap();
        ImageProperties::default().floating(floating).rotate(90.0).unwrap().flip_horizontal().flip_vertical()
    }
}

fn render(wrap: ImageWrap) -> String {
    let parts = [("word/document.xml".into(), document("<w:p><w:r><w:t>{%picture}</w:t></w:r></w:p>").into_bytes())];
    let template = PackageTemplate::parse(parts, &FillOptions::default()).unwrap();
    let output = template.render_with(&json!({"picture":"one"}), &mut Picture(wrap));
    String::from_utf8_lossy(&output.iter().find(|(name, _)| name == "word/document.xml").unwrap().1).into_owned()
}

#[test]
fn positions_and_transforms_images() {
    let xml = render(ImageWrap::TopBottom);

    assert!(xml.contains("<wp:anchor ") && !xml.contains("wp:inline"));
    assert!(xml.contains("behindDoc=\"1\"") && xml.contains("distT=\"25400\""));
    assert!(xml.contains("<wp:positionH relativeFrom=\"margin\"><wp:posOffset>914400</wp:posOffset>"));
    assert!(xml.contains("<wp:positionV relativeFrom=\"paragraph\"><wp:posOffset>-190500</wp:posOffset>"));
    assert!(xml.contains("<wp:wrapTopAndBottom/>") && xml.contains("rot=\"5400000\""));
    assert!(xml.contains("flipH=\"1\"") && xml.contains("flipV=\"1\""));
}

#[test]
fn writes_each_wrap_mode() {
    for (wrap, tag) in [(ImageWrap::None,"wrapNone"), (ImageWrap::Square,"wrapSquare"), (ImageWrap::Tight,"wrapTight"), (ImageWrap::Through,"wrapThrough")] {
        assert!(render(wrap).contains(&format!("<wp:{tag}")));
    }
}

#[test]
fn rejects_invalid_positions() {
    assert!(ImageFloat::new(ImageLength::Pixels(f64::NAN), ImageLength::Pixels(0.0)).is_none());
    assert!(ImageFloat::new(ImageLength::Pixels(0.0), ImageLength::Pixels(0.0)).unwrap().relative(ImageRelative::Paragraph, ImageRelative::Page).is_none());
    assert!(ImageProperties::default().rotate(f64::INFINITY).is_none());
}
