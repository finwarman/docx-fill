use crate::properties::{find_starts, set_attribute};
use crate::xml::escape;
use super::layout::{ImageFloat, ImageTransform};

const EMU_PER_POINT: f64 = 12700.0;
const DRAWING_NAMESPACE: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ImageAlignment { #[default] Left, Center, Right }

impl ImageAlignment {
    pub(crate) fn value(self) -> &'static str {
        match self { Self::Left => "left", Self::Center => "center", Self::Right => "right" }
    }
}

/// A solid image border, with its width in points.
#[derive(Debug, Clone)]
pub struct ImageBorder { color: String, width: u32 }

impl ImageBorder {
    pub fn new(color: &str, points: f64) -> Option<Self> {
        let color = color.strip_prefix('#').unwrap_or(color);
        if color.len() != 6 || !color.bytes().all(|c| c.is_ascii_hexdigit()) || !points.is_finite() || points <= 0.0 || points * EMU_PER_POINT > i32::MAX as f64 { return None; }
        Some(Self { color: color.to_ascii_uppercase(), width: (points * EMU_PER_POINT).round() as u32 })
    }
}

/// A caption paragraph inserted after the image's paragraph.
#[derive(Debug, Clone)]
pub struct ImageCaption { text: String, prefix: String, sequence: Option<String>, style: String, align: ImageAlignment }

impl ImageCaption {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into(), prefix: "Illustration ".into(), sequence: Some("Illustration".into()), style: "Caption".into(), align: ImageAlignment::Left }
    }
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self { self.prefix = prefix.into(); self.sequence = None; self }
    pub fn sequence(mut self, name: impl Into<String>) -> Self { self.sequence = Some(name.into()); self }
    pub fn style(mut self, style: impl Into<String>) -> Self { self.style = style.into(); self }
    pub fn align(mut self, align: ImageAlignment) -> Self { self.align = align; self }

    fn xml(&self, number: u32) -> String {
        let mut out = format!("<w:p><w:pPr><w:pStyle w:val=\"{}\"/><w:jc w:val=\"{}\"/></w:pPr>", escaped(&self.style), self.align.value());
        out.push_str(&text_run(&self.prefix));
        if let Some(sequence) = &self.sequence {
            out.push_str(&format!("<w:fldSimple w:instr=\"SEQ {} \\* ARABIC\"><w:r><w:t>{number}</w:t></w:r></w:fldSimple>", escaped(sequence)));
            out.push_str(&text_run(" "));
        }
        out.push_str(&text_run(&self.text));
        out.push_str("</w:p>");
        out
    }
}

/// Per-image metadata and presentation. Unset values preserve existing defaults.
#[derive(Debug, Clone, Default)]
pub struct ImageProperties {
    alt: Option<String>, name: Option<String>, align: Option<ImageAlignment>,
    border: Option<ImageBorder>, link: Option<String>, caption: Option<ImageCaption>,
    floating: Option<ImageFloat>, transform: ImageTransform,
}

impl ImageProperties {
    pub fn alt(mut self, text: impl Into<String>) -> Self { self.alt = Some(text.into()); self }
    pub fn name(mut self, text: impl Into<String>) -> Self { self.name = Some(text.into()); self }
    pub fn align(mut self, align: ImageAlignment) -> Self { self.align = Some(align); self }
    pub fn border(mut self, border: ImageBorder) -> Self { self.border = Some(border); self }
    pub fn link(mut self, target: impl Into<String>) -> Self { self.link = Some(target.into()); self }
    pub fn caption(mut self, caption: ImageCaption) -> Self { self.caption = Some(caption); self }
    pub fn floating(mut self, floating: ImageFloat) -> Self { self.floating = Some(floating); self }
    pub fn rotate(mut self, degrees: f64) -> Option<Self> { self.transform.rotate(degrees)?; Some(self) }
    pub fn flip_horizontal(mut self) -> Self { self.transform.flip_horizontal(); self }
    pub fn flip_vertical(mut self) -> Self { self.transform.flip_vertical(); self }

    pub(crate) fn alignment(&self) -> Option<&'static str> { self.align.map(ImageAlignment::value) }
    pub(crate) fn link_target(&self) -> Option<&str> { self.link.as_deref() }
    pub(crate) fn caption_xml(&self, number: u32) -> Option<String> { self.caption.as_ref().map(|caption| caption.xml(number)) }

    pub(crate) fn apply(&self, mut xml: String, link: Option<&str>) -> String {
        for element in ["wp:docPr", "pic:cNvPr"] {
            let Some(start) = find_starts(&xml, element).first().copied() else { continue };
            let Some(end) = xml[start..].find('>').map(|end| start + end + 1) else { continue };
            let mut raw = xml[start..end].to_string();
            if let Some(name) = &self.name { raw = set_attribute(&raw, "name", &escaped(name)); }
            if let Some(alt) = &self.alt { raw = set_attribute(&raw, "descr", &escaped(alt)); }
            if element == "wp:docPr"
                && let Some(link) = link {
                raw = format!("{}><a:hlinkClick xmlns:a=\"{DRAWING_NAMESPACE}\" r:id=\"{}\"/></wp:docPr>", raw.trim_end_matches("/>"), escaped(link));
            }
            xml.replace_range(start..end, &raw);
        }
        if let Some(border) = &self.border {
            let line = format!("<a:ln w=\"{}\"><a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill><a:prstDash val=\"solid\"/></a:ln>", border.width, border.color);
            xml = xml.replacen("</pic:spPr>", &format!("{line}</pic:spPr>"), 1);
        }
        xml = self.transform.apply(xml);
        match &self.floating { Some(floating) => floating.apply(xml), None => xml }
    }
}

fn escaped(text: &str) -> String { let mut out = String::new(); escape(text, &mut out); out }
fn text_run(text: &str) -> String { format!("<w:r><w:t xml:space=\"preserve\">{}</w:t></w:r>", escaped(text)) }
