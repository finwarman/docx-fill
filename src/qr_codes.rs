use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;

use crate::properties::{find_element_end, find_starts, set_attribute};
use crate::template::read_image_tag;
use crate::xml::{Token, escape, read_attribute, tokenize, unescape};
use crate::FillOptions;

const IMAGE_RELATIONSHIP: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";
const MAX_IMAGE_SIDE: u32 = 8192;
const MAX_DECODE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_QR_SIDE: u32 = 2048;

/// Decode each referenced raster once, then let normal picture tags handle scope and layout.
pub(crate) fn prepare(parts: &mut [(String, Vec<u8>)], options: &FillOptions) {
    if options.delimiters.start.is_empty() || options.delimiters.end.is_empty() { return; }
    let sources: BTreeMap<_, _> = parts.iter().map(|(name, bytes)| (name.as_str(), bytes.as_slice())).collect();
    let mut decoded = BTreeMap::<String, Option<String>>::new();
    let mut changes = Vec::new();
    for (index, (name, bytes)) in parts.iter().enumerate() {
        let Ok(xml) = std::str::from_utf8(bytes) else { continue };
        if !xml.contains("<w:drawing") { continue; }
        let relations = image_relations(name, &sources);
        let mut edits = Vec::new();
        for start in find_starts(xml, "w:drawing") {
            let Some(end) = find_element_end(xml, start, "w:drawing") else { continue };
            let drawing = &xml[start..end];
            let Some(property_start) = find_starts(drawing, "wp:docPr").first().copied() else { continue };
            let Some(property_end) = drawing[property_start..].find('>').map(|at| property_start + at + 1) else { continue };
            let properties = &drawing[property_start..property_end];
            if ["descr", "name"].iter().any(|name| read_attribute(properties, name).is_some_and(|text| read_image_tag(&unescape(text), options).is_some())) { continue; }
            let Some(blip_start) = find_starts(drawing, "a:blip").first().copied() else { continue };
            let Some(blip_end) = drawing[blip_start..].find('>').map(|at| blip_start + at + 1) else { continue };
            let Some(id) = read_attribute(&drawing[blip_start..blip_end], "r:embed") else { continue };
            let Some(path) = relations.get(id) else { continue };
            let payload = decoded.entry(path.clone()).or_insert_with(|| sources.get(path.as_str()).and_then(|bytes| decode(bytes, options)));
            let Some(payload) = payload else { continue };
            let mut escaped = String::new(); escape(payload, &mut escaped);
            edits.push((start + property_start, start + property_end, set_attribute(properties, "descr", &escaped)));
        }
        if edits.is_empty() { continue; }
        let mut changed = xml.to_string();
        for (start, end, value) in edits.into_iter().rev() { changed.replace_range(start..end, &value); }
        changes.push((index, changed.into_bytes()));
    }
    for (index, bytes) in changes { parts[index].1 = bytes; }
}

fn decode(bytes: &[u8], options: &FillOptions) -> Option<String> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_SIDE);
    limits.max_image_height = Some(MAX_IMAGE_SIDE);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let mut gray = reader.decode().ok()?.into_luma8();
    let largest = gray.width().max(gray.height());
    if largest > MAX_QR_SIDE {
        let width = (u64::from(gray.width()) * u64::from(MAX_QR_SIDE) / u64::from(largest)).max(1) as u32;
        let height = (u64::from(gray.height()) * u64::from(MAX_QR_SIDE) / u64::from(largest)).max(1) as u32;
        gray = image::imageops::resize(&gray, width, height, image::imageops::FilterType::Nearest);
    }
    let mut decoder = quircs::Quirc::default();
    let mut tags = BTreeSet::new();
    for code in decoder.identify(gray.width() as usize, gray.height() as usize, &gray) {
        let Some(payload) = code.ok().and_then(|code| code.decode().ok()).and_then(|data| String::from_utf8(data.payload).ok()) else { continue };
        if read_image_tag(&payload, options).is_some() { tags.insert(payload); }
    }
    if tags.len() == 1 { tags.pop_first() } else { None }
}

fn image_relations(part: &str, sources: &BTreeMap<&str, &[u8]>) -> BTreeMap<String, String> {
    let (folder, file) = part.rsplit_once('/').unwrap_or(("", part));
    let path = if folder.is_empty() { format!("_rels/{file}.rels") } else { format!("{folder}/_rels/{file}.rels") };
    let Some(bytes) = sources.get(path.as_str()) else { return BTreeMap::new() };
    let Ok(xml) = std::str::from_utf8(bytes) else { return BTreeMap::new() };
    tokenize(xml).unwrap_or_default().into_iter().filter_map(|token| {
        let Token::Element { name: "Relationship", raw, .. } = token else { return None };
        if read_attribute(raw, "Type") != Some(IMAGE_RELATIONSHIP) || read_attribute(raw, "TargetMode") == Some("External") { return None; }
        Some((read_attribute(raw, "Id")?.to_string(), resolve(folder, &unescape(read_attribute(raw, "Target")?))?))
    }).collect()
}

fn resolve(folder: &str, target: &str) -> Option<String> {
    let mut components = if target.starts_with('/') { Vec::new() } else { folder.split('/').filter(|part| !part.is_empty()).collect::<Vec<_>>() };
    for part in target.split('/') {
        match part { "" | "." => {}, ".." => { components.pop()?; }, _ => components.push(part) }
    }
    Some(components.join("/"))
}
