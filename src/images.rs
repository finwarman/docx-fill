//! Image tags: a drawing sized to its table cell or the page, and the media it shows.

use std::sync::Arc;
pub(crate) mod replacement;

use crate::template::PX_PER_TWIP;
use crate::xml::{ElementKind, Token, read_attribute, tokenize};

const EMU_PER_PX: f64 = 9525.0;
const SVG_NAMESPACE: &str = "http://schemas.microsoft.com/office/drawing/2016/SVG/main";
const SVG_EXTENSION: &str = "{96DAC541-7B7A-43D3-8B79-37D633B846F1}";
const PIXELS_PER_INCH: f64 = 96.0;
const CENTIMETERS_PER_INCH: f64 = 2.54;
const MILLIMETERS_PER_INCH: f64 = 25.4;
const POINTS_PER_INCH: f64 = 72.0;
const PICAS_PER_INCH: f64 = 6.0;
// Word's ST_PositiveCoordinate limit.
const MAX_WORD_IMAGE_EMU: f64 = i32::MAX as f64;
// A4 less 1 inch margins, for a part with no page size.
const DEFAULT_BODY: (f64, f64) = (698.0, 1027.0);

/// An image a hook supplies for an image tag's value.
#[derive(Debug, Clone)]
pub struct Image {
    /// Names its media part and relationship; images with one key share them.
    pub key: String,
    pub bytes: Arc<[u8]>,
    /// The media file's extension, such as `png` or `jpeg`.
    pub extension: String,
    /// Intrinsic size in any unit; only the ratio counts.
    pub width: f64,
    pub height: f64,
}

/// One display dimension, independent of the image's intrinsic dimensions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ImageLength {
    Pixels(f64),
    Inches(f64),
    Centimeters(f64),
    Millimeters(f64),
    Points(f64),
    Picas(f64),
    Emus(f64),
}

impl ImageLength {
    fn pixels(self) -> f64 {
        match self {
            Self::Pixels(value) => value,
            Self::Inches(value) => value * PIXELS_PER_INCH,
            Self::Centimeters(value) => value * PIXELS_PER_INCH / CENTIMETERS_PER_INCH,
            Self::Millimeters(value) => value * PIXELS_PER_INCH / MILLIMETERS_PER_INCH,
            Self::Points(value) => value * PIXELS_PER_INCH / POINTS_PER_INCH,
            Self::Picas(value) => value * PIXELS_PER_INCH / PICAS_PER_INCH,
            Self::Emus(value) => value / EMU_PER_PX,
        }
    }
}

/// Validated display size. Explicit sizes are not fitted or padded.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageSize {
    width: f64,
    height: f64,
}

impl ImageSize {
    /// `None` for non-finite, zero, negative or unsupported Word dimensions.
    pub fn new(width: ImageLength, height: ImageLength) -> Option<Self> {
        let (width, height) = (width.pixels(), height.pixels());
        let valid = |value: f64| value.is_finite() && (1.0..=MAX_WORD_IMAGE_EMU).contains(&(value * EMU_PER_PX).round());
        (valid(width) && valid(height)).then_some(Self { width, height })
    }

    /// A display size in pixels at 96 DPI.
    pub fn pixels(width: f64, height: f64) -> Option<Self> {
        Self::new(ImageLength::Pixels(width), ImageLength::Pixels(height))
    }

    pub(crate) fn dimensions(self) -> (f64, f64) {
        (self.width, self.height)
    }
}

/// A media part a filled part shows, and the relationship it shows it by.
#[derive(Debug, Clone)]
pub(crate) struct MediaUse {
    pub relationship_id: String,
    /// Relative to the part's folder, such as `media/docx_fill_a.png`.
    pub target: String,
    pub image: Image,
}

impl MediaUse {
    pub(crate) fn for_image(image: Image) -> MediaUse {
        let safe: String = image.key.chars().filter(char::is_ascii_alphanumeric).collect();
        MediaUse { relationship_id: format!("rIdDocxFill_{safe}"), target: format!("media/docx_fill_{safe}.{}", image.extension), image }
    }

    pub(crate) fn svg_fallback(image: &Image, bytes: Arc<[u8]>) -> MediaUse {
        let mut media = Self::for_image(Image { key: image.key.clone(), bytes, extension: "png".into(), width: image.width, height: image.height });
        media.relationship_id = media.relationship_id.replacen("rIdDocxFill_", "rIdDocxFillSvgFallback_", 1);
        media.target = media.target.replacen("media/", "media/svg_fallback/", 1);
        media
    }
}

/// The drawing an image tag prints, in pixels: `width` wide, its height from the image's shape.
/// Outside a table cell it also fits the page's height.
pub(crate) fn drawing_size(image: &Image, width: f64, is_in_cell: bool, body_height: f64) -> (f64, f64) {
    let height = if image.width > 0.0 { width * image.height / image.width } else { width };
    let fit = if is_in_cell || height <= 0.0 { 1.0 } else { (body_height / height).min(1.0) };
    (width * fit, height * fit)
}

/// An inline drawing of relationship `relationship_id`, `width` by `height` pixels.
pub(crate) fn drawing_xml(id: u32, relationship_id: &str, width: f64, height: f64, svg: Option<&str>) -> String {
    let extent = format!("cx=\"{}\" cy=\"{}\"", (width * EMU_PER_PX).round() as i64, (height * EMU_PER_PX).round() as i64);
    let blip = blip_xml(relationship_id, svg);
    format!(
        "<w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\"><wp:extent {extent}/>\
<wp:docPr id=\"{id}\" name=\"Photo {id}\"/>\
<a:graphic xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
<pic:pic xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
<pic:nvPicPr><pic:cNvPr id=\"0\" name=\"photo\"/><pic:cNvPicPr/></pic:nvPicPr>\
<pic:blipFill>{blip}<a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
<pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext {extent}/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr>\
</pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing>"
    )
}

fn blip_xml(relationship_id: &str, svg: Option<&str>) -> String {
    match svg {
        Some(svg) => format!("<a:blip r:embed=\"{relationship_id}\"><a:extLst><a:ext uri=\"{SVG_EXTENSION}\"><asvg:svgBlip xmlns:asvg=\"{SVG_NAMESPACE}\" r:embed=\"{svg}\"/></a:ext></a:extLst></a:blip>"),
        None => format!("<a:blip r:embed=\"{relationship_id}\"/>"),
    }
}

/// The largest `<wp:docPr id>` in a part, or 0.
pub(crate) fn read_max_drawing_id(xml: &str) -> u32 {
    let Ok(tokens) = tokenize(xml) else { return 0 };
    tokens
        .iter()
        .filter_map(|token| match *token {
            Token::Element { kind: ElementKind::Open | ElementKind::SelfClosing, name: "wp:docPr", raw } => read_attribute(raw, "id")?.parse().ok(),
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

/// The page's width and height less its margins, in pixels, from the part's last section; `None`
/// when the part gives no page size.
pub(crate) fn read_body_size(xml: &str) -> Option<(f64, f64)> {
    let section = &xml[xml.rfind("<w:sectPr")?..];
    let element = |name: &str| {
        let start = section.find(&format!("<{name}"))?;
        let end = section[start..].find('>')?;
        Some(&section[start..start + end + 1])
    };
    let twips = |raw: Option<&str>, attribute: &str| raw.and_then(|raw| read_attribute(raw, attribute)).and_then(|value| value.parse::<f64>().ok());

    let size = element("w:pgSz ");
    let margins = element("w:pgMar ");
    let (width, height) = (twips(size, "w:w")?, twips(size, "w:h")?);
    if width == 0.0 || height == 0.0 {
        return None;
    }
    let margin = |side| twips(margins, side).unwrap_or(0.0);
    Some(((width - margin("w:left") - margin("w:right")) * PX_PER_TWIP, (height - margin("w:top") - margin("w:bottom")) * PX_PER_TWIP))
}

pub(crate) fn body_or_default(body: Option<(f64, f64)>) -> (f64, f64) {
    body.unwrap_or(DEFAULT_BODY)
}
