//! HTML image sources and dimensions. External sources are resolved by the caller's hook.

use std::sync::Arc;
use crate::{Image, ImageLength, ImageProperties, ImageSize};

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;
const BASE64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) struct HtmlImage {
    pub slot: usize,
    pub source: String,
    pub attributes: Vec<(String, String)>,
}

impl HtmlImage {
    pub fn embedded(&self) -> Result<Option<Image>, String> {
        let (extension, bytes) = if self.source.trim_start().get(..4).is_some_and(|start| start.eq_ignore_ascii_case("<svg")) {
            ("svg", self.source.as_bytes().to_vec())
        } else if let Some(data) = self.source.strip_prefix("data:") {
            let (header, body) = data.split_once(',').ok_or("Invalid image data URL")?;
            let mime = header.split(';').next().unwrap_or_default();
            let extension = match mime.to_ascii_lowercase().as_str() {
                "image/png" => "png", "image/jpeg" | "image/jpg" => "jpeg", "image/gif" => "gif", "image/svg+xml" => "svg",
                _ => return Err(format!("Unsupported image MIME type {mime}")),
            };
            let bytes = if header.split(';').any(|part| part.eq_ignore_ascii_case("base64")) { decode_base64(body)? } else { decode_uri(body)? };
            (extension, bytes)
        } else { return Ok(None); };
        if extension == "svg" {
            let xml = std::str::from_utf8(&bytes).map_err(|_| "Invalid SVG encoding")?;
            crate::xml::tokenize(xml).and_then(|tokens| crate::xml::check_balanced(&tokens))?;
        }
        let explicit = || Some((length(self.attribute("width")?, 0.0)?, length(self.attribute("height")?, 0.0)?));
        let (width, height) = source_size(extension, &bytes).or_else(|| (extension == "svg").then(explicit).flatten()).ok_or("Invalid or unsized image source")?;
        let hash = bytes.iter().fold(FNV_OFFSET, |hash, byte| (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME));
        Ok(Some(Image { key: format!("html{hash:016x}"), bytes: Arc::from(bytes), extension: extension.into(), width, height }))
    }

    pub fn size(&self, image: &Image, container: f64) -> Option<ImageSize> {
        let width = self.attribute("width").and_then(|value| length(value, container));
        let height = self.attribute("height").and_then(|value| length(value, container));
        let ratio = image.width / image.height;
        let (width, height) = match (width, height) {
            (Some(width), Some(height)) => (width, height),
            (Some(width), None) => (width, width / ratio),
            (None, Some(height)) => (height * ratio, height),
            (None, None) => (image.width, image.height),
        };
        ImageSize::pixels(width, height)
    }

    pub fn properties(&self) -> ImageProperties {
        let mut properties = ImageProperties::default();
        if let Some(alt) = self.attribute("alt") { properties = properties.alt(alt); }
        if let Some(title) = self.attribute("title") { properties = properties.name(title); }
        properties
    }

    fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
    }
}

fn length(value: &str, container: f64) -> Option<f64> {
    let value = value.trim();
    if let Some(percent) = value.strip_suffix('%') { return percent.trim().parse::<f64>().ok().map(|percent| container * percent / 100.0); }
    let unit = [("px", ImageLength::Pixels as fn(f64) -> ImageLength), ("pt", ImageLength::Points), ("in", ImageLength::Inches), ("cm", ImageLength::Centimeters), ("mm", ImageLength::Millimeters), ("pc", ImageLength::Picas)];
    let length = unit.into_iter().find_map(|(unit, constructor)| value.strip_suffix(unit).and_then(|value| value.trim().parse().ok()).map(constructor))
        .or_else(|| value.parse().ok().map(ImageLength::Pixels))?;
    ImageSize::new(length, ImageLength::Pixels(1.0)).map(|size| size.dimensions().0)
}

fn source_size(extension: &str, bytes: &[u8]) -> Option<(f64, f64)> {
    match extension {
        "png" if bytes.starts_with(PNG_SIGNATURE) && bytes.get(12..16)? == b"IHDR" => Some((u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?) as f64, u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?) as f64)),
        "gif" if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") => Some((u16::from_le_bytes(bytes.get(6..8)?.try_into().ok()?) as f64, u16::from_le_bytes(bytes.get(8..10)?.try_into().ok()?) as f64)),
        "jpeg" => jpeg_size(bytes),
        "svg" => svg_size(std::str::from_utf8(bytes).ok()?),
        _ => None,
    }.filter(|(width, height)| *width > 0.0 && *height > 0.0 && width.is_finite() && height.is_finite())
}

fn jpeg_size(bytes: &[u8]) -> Option<(f64, f64)> {
    if !bytes.starts_with(&[0xff, 0xd8]) { return None; }
    let mut at = 2;
    while bytes.get(at) == Some(&0xff) {
        while bytes.get(at) == Some(&0xff) { at += 1; }
        let marker = *bytes.get(at)?;
        at += 1;
        if marker == 0xd9 || marker == 0xda { return None; }
        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) { continue; }
        let length = u16::from_be_bytes(bytes.get(at..at + 2)?.try_into().ok()?) as usize;
        if length < 2 { return None; }
        let segment = bytes.get(at..at + length)?;
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            return Some((u16::from_be_bytes(segment.get(5..7)?.try_into().ok()?) as f64, u16::from_be_bytes(segment.get(3..5)?.try_into().ok()?) as f64));
        }
        at += length;
    }
    None
}

fn svg_size(svg: &str) -> Option<(f64, f64)> {
    let start = svg.to_ascii_lowercase().find("<svg")?;
    let root = &svg[start..start + svg[start..].find('>')? + 1];
    let attribute = |name| crate::xml::read_attribute(root, name);
    let view = attribute("viewBox").map(|value| value.split(|c: char| c.is_whitespace() || c == ',').filter(|s| !s.is_empty()).map(str::parse::<f64>).collect::<Result<Vec<_>, _>>()).transpose().ok().flatten();
    let fallback = view.as_ref().filter(|view| view.len() == 4).map(|view| (view[2], view[3]));
    let width = attribute("width").and_then(|value| length(value, fallback.map_or(0.0, |size| size.0)));
    let height = attribute("height").and_then(|value| length(value, fallback.map_or(0.0, |size| size.1)));
    match (width, height, fallback) {
        (Some(width), Some(height), _) => Some((width, height)),
        (Some(width), None, Some((vw, vh))) => Some((width, width * vh / vw)),
        (None, Some(height), Some((vw, vh))) => Some((height * vw / vh, height)),
        (None, None, fallback) => fallback,
        _ => None,
    }
}

fn decode_base64(text: &str) -> Result<Vec<u8>, String> {
    let bytes: Vec<_> = text.bytes().filter(|byte| !byte.is_ascii_whitespace()).collect();
    if bytes.len() % 4 != 0 { return Err("Invalid base64 image".into()); }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for (index, chunk) in bytes.as_chunks::<4>().0.iter().enumerate() {
        let padding = chunk.iter().rev().take_while(|byte| **byte == b'=').count();
        if padding > 2 || (padding > 0 && index + 1 != bytes.len() / 4) { return Err("Invalid base64 image padding".into()); }
        let mut value = 0u32;
        for (index, byte) in chunk.iter().enumerate() {
            let digit = if *byte == b'=' && index >= 4 - padding { 0 } else { BASE64.iter().position(|candidate| candidate == byte).ok_or("Invalid base64 image character")? as u32 };
            value = value << 6 | digit;
        }
        if (padding == 1 && value & 0xff != 0) || (padding == 2 && value & 0xffff != 0) { return Err("Invalid base64 image padding bits".into()); }
        out.extend_from_slice(&value.to_be_bytes()[1..4 - padding]);
    }
    Ok(out)
}

fn decode_uri(text: &str) -> Result<Vec<u8>, String> {
    let mut bytes = text.bytes();
    let mut out = Vec::new();
    while let Some(byte) = bytes.next() {
        if byte != b'%' { out.push(byte); continue; }
        let hex = [bytes.next().ok_or("Invalid image URI escape")?, bytes.next().ok_or("Invalid image URI escape")?];
        let hex = std::str::from_utf8(&hex).map_err(|_| "Invalid image URI escape")?;
        out.push(u8::from_str_radix(hex, 16).map_err(|_| "Invalid image URI escape")?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_jpeg_frame_dimensions() {
        let bytes = [0xff, 0xd8, 0xff, 0xe0, 0, 4, 0, 0, 0xff, 0xc2, 0, 8, 8, 0, 12, 0, 24, 1];
        assert_eq!(jpeg_size(&bytes), Some((24.0, 12.0)));
        assert_eq!(jpeg_size(&bytes[..14]), None);
    }

    #[test]
    fn validates_base64_padding() {
        assert_eq!(decode_base64("Zm9v\nYg==").unwrap(), b"foob");
        for value in ["A===", "AA=A", "AA==AA==", "AB==", "AAA"] { assert!(decode_base64(value).is_err(), "{value}"); }
    }
}
