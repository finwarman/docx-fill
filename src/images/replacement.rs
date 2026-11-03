use crate::properties::{find_element_end, find_starts};
use crate::xml::{ElementKind, Token, escape, read_attribute, tokenize, unescape};
use super::{EMU_PER_PX, Image, ImageSize};

pub(crate) struct PictureReplacement {
    pub image: Image,
    pub size: Option<ImageSize>,
    pub tag: String,
    pub fallback: Option<std::sync::Arc<[u8]>>,
}

pub(crate) struct PictureFrame {
    pub start: usize,
    pub replacement: Option<PictureReplacement>,
    #[cfg(feature = "qr-codes")]
    pub(crate) source_properties: Option<String>,
}

/// Change the picture source while preserving its existing anchor, crop and geometry.
pub(crate) fn replace_picture(xml: &str, relationship: &str, size: Option<ImageSize>, tag: &str, svg: Option<&str>) -> Option<String> {
    let start = own_blip(xml)?;
    let end = find_element_end(xml, start, "a:blip")?;
    let mut out = format!("{}{}{}", &xml[..start], super::blip_xml(relationship, svg), &xml[end..]);
    if let Some(size) = size {
        let (width, height) = size.dimensions();
        // The picture's `a:ext` is the one in `a:xfrm`; `wp:docPr` can hold an `a:ext` of its own.
        let transform = find_starts(&out, "a:xfrm").first().copied();
        for (name, after) in [("wp:extent", Some(0)), ("a:ext", transform)] {
            let Some(after) = after else { continue };
            if let Some(start) = find_starts(&out, name).into_iter().find(|start| *start >= after)
                && let Some(end) = out[start..].find('>').map(|at| start + at + 1) {
                let raw = set_attribute(&out[start..end], "cx", &(width * EMU_PER_PX).round().to_string());
                let raw = set_attribute(&raw, "cy", &(height * EMU_PER_PX).round().to_string());
                out.replace_range(start..end, &raw);
            }
        }
    }
    for name in ["wp:docPr", "pic:cNvPr"] {
        if let Some(start) = find_starts(&out, name).first().copied()
            && let Some(end) = out[start..].find('>').map(|at| start + at + 1) {
            let mut raw = out[start..end].to_string();
            for attribute in ["descr", "name"] {
                if let Some(value) = read_attribute(&raw, attribute) {
                    let value = unescape(value).replace(tag, "");
                    let mut escaped = String::new();
                    escape(&value, &mut escaped);
                    raw = set_attribute(&raw, attribute, &escaped);
                }
            }
            out.replace_range(start..end, &raw);
        }
    }
    Some(out)
}

fn own_blip(xml: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut at = 0;
    for token in tokenize(xml).ok()? {
        let length = match token { Token::Element { raw, .. } => raw.len(), Token::Text(text) => text.len() };
        if let Token::Element { kind, name, .. } = token {
            match (kind, name) {
                (ElementKind::Open, "w:drawing") => depth += 1,
                (ElementKind::Close, "w:drawing") => depth = depth.checked_sub(1)?,
                (ElementKind::Open | ElementKind::SelfClosing, "a:blip") if depth == 1 => return Some(at),
                _ => {}
            }
        }
        at += length;
    }
    None
}

fn set_attribute(raw: &str, name: &str, value: &str) -> String {
    for quote in ['"', '\''] {
        let needle = format!(" {name}={quote}");
        if let Some(at) = raw.find(&needle)
            && let Some(end) = raw[at + needle.len()..].find(quote) {
            let end = at + needle.len() + end + 1;
            return format!("{} {name}=\"{value}\"{}", &raw[..at], &raw[end..]);
        }
    }
    crate::properties::set_attribute(raw, name, value)
}
