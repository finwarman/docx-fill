use super::ImageLength;
use crate::properties::{find_starts, set_attribute};

const EMU_PER_PIXEL: f64 = 9525.0;
const ANGLE_UNITS_PER_DEGREE: f64 = 60000.0;
const WRAP_COORDINATE: i32 = 21600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageRelative { Page, Margin, Column, Character, Paragraph, Line }

impl ImageRelative {
    fn value(self) -> &'static str {
        match self { Self::Page => "page", Self::Margin => "margin", Self::Column => "column", Self::Character => "character", Self::Paragraph => "paragraph", Self::Line => "line" }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ImageWrap { None, #[default] Square, Tight, Through, TopBottom }

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ImageLayer { Behind, #[default] InFront }

/// Floating image position and wrap distances. Offsets may be negative.
#[derive(Debug, Clone)]
pub struct ImageFloat {
    x: i32, y: i32, horizontal: ImageRelative, vertical: ImageRelative,
    wrap: ImageWrap, layer: ImageLayer, margins: [u32; 4],
}

impl ImageFloat {
    pub fn new(x: ImageLength, y: ImageLength) -> Option<Self> {
        Some(Self { x: coordinate(x)?, y: coordinate(y)?, horizontal: ImageRelative::Page, vertical: ImageRelative::Page, wrap: ImageWrap::Square, layer: ImageLayer::InFront, margins: [0; 4] })
    }
    pub fn relative(mut self, horizontal: ImageRelative, vertical: ImageRelative) -> Option<Self> {
        if matches!(horizontal, ImageRelative::Paragraph | ImageRelative::Line) || matches!(vertical, ImageRelative::Column | ImageRelative::Character) { return None; }
        self.horizontal = horizontal; self.vertical = vertical; Some(self)
    }
    pub fn wrap(mut self, wrap: ImageWrap) -> Self { self.wrap = wrap; self }
    pub fn layer(mut self, layer: ImageLayer) -> Self { self.layer = layer; self }
    pub fn margins(mut self, top: ImageLength, right: ImageLength, bottom: ImageLength, left: ImageLength) -> Option<Self> {
        let values = [coordinate(top)?, coordinate(right)?, coordinate(bottom)?, coordinate(left)?];
        if values.iter().any(|value| *value < 0) { return None; }
        self.margins = values.map(|value| value as u32); Some(self)
    }

    pub(super) fn apply(&self, mut xml: String) -> String {
        let Some(start) = find_starts(&xml, "wp:inline").first().copied() else { return xml };
        let Some(end) = xml[start..].find('>').map(|at| start + at + 1) else { return xml };
        let [top, right, bottom, left] = self.margins;
        let behind = u8::from(self.layer == ImageLayer::Behind);
        let head = format!("<wp:anchor distT=\"{top}\" distR=\"{right}\" distB=\"{bottom}\" distL=\"{left}\" simplePos=\"0\" relativeHeight=\"0\" behindDoc=\"{behind}\" locked=\"0\" layoutInCell=\"1\" allowOverlap=\"1\"><wp:simplePos x=\"0\" y=\"0\"/><wp:positionH relativeFrom=\"{}\"><wp:posOffset>{}</wp:posOffset></wp:positionH><wp:positionV relativeFrom=\"{}\"><wp:posOffset>{}</wp:posOffset></wp:positionV>", self.horizontal.value(), self.x, self.vertical.value(), self.y);
        xml.replace_range(start..end, &head);
        xml = xml.replacen("</wp:inline>", "</wp:anchor>", 1);
        let wrap = match self.wrap {
            ImageWrap::None => "<wp:wrapNone/>".to_string(),
            ImageWrap::Square => "<wp:wrapSquare wrapText=\"bothSides\"/>".to_string(),
            ImageWrap::TopBottom => "<wp:wrapTopAndBottom/>".to_string(),
            ImageWrap::Tight | ImageWrap::Through => {
                let kind = if self.wrap == ImageWrap::Tight { "wrapTight" } else { "wrapThrough" };
                format!("<wp:{kind} wrapText=\"bothSides\"><wp:wrapPolygon edited=\"0\"><wp:start x=\"0\" y=\"0\"/><wp:lineTo x=\"0\" y=\"{WRAP_COORDINATE}\"/><wp:lineTo x=\"{WRAP_COORDINATE}\" y=\"{WRAP_COORDINATE}\"/><wp:lineTo x=\"{WRAP_COORDINATE}\" y=\"0\"/><wp:lineTo x=\"0\" y=\"0\"/></wp:wrapPolygon></wp:{kind}>")
            }
        };
        xml.replacen("<wp:docPr", &format!("{wrap}<wp:docPr"), 1)
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct ImageTransform { rotation: Option<i32>, horizontal: bool, vertical: bool }

impl ImageTransform {
    pub(super) fn rotate(&mut self, degrees: f64) -> Option<()> {
        if !degrees.is_finite() { return None; }
        self.rotation = Some((degrees.rem_euclid(360.0) * ANGLE_UNITS_PER_DEGREE).round() as i32); Some(())
    }
    pub(super) fn flip_horizontal(&mut self) { self.horizontal = true; }
    pub(super) fn flip_vertical(&mut self) { self.vertical = true; }
    pub(super) fn apply(&self, mut xml: String) -> String {
        let Some(start) = find_starts(&xml, "a:xfrm").first().copied() else { return xml };
        let Some(end) = xml[start..].find('>').map(|at| start + at + 1) else { return xml };
        let mut raw = xml[start..end].to_string();
        if let Some(rotation) = self.rotation { raw = set_attribute(&raw, "rot", &rotation.to_string()); }
        if self.horizontal { raw = set_attribute(&raw, "flipH", "1"); }
        if self.vertical { raw = set_attribute(&raw, "flipV", "1"); }
        xml.replace_range(start..end, &raw); xml
    }
}

fn coordinate(length: ImageLength) -> Option<i32> {
    let value = length.pixels() * EMU_PER_PIXEL;
    (value.is_finite() && value.round() >= i32::MIN as f64 && value.round() <= i32::MAX as f64).then(|| value.round() as i32)
}
