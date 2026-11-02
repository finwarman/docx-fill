use std::sync::{Arc, Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, Ordering};

use resvg::{tiny_skia, usvg};
use super::Image;

const MAX_SVG_BYTES: usize = 8 * 1024 * 1024;
const MAX_RESOURCE_BYTES: usize = 16 * 1024 * 1024;
const MAX_FONT_BYTES: usize = 32 * 1024 * 1024;
const MAX_SIDE: u32 = 4096;
const MAX_IMAGE_PIXELS: usize = 16 * 1024 * 1024;
const MAX_FONT_TABLES: usize = 256;
const MAX_SVG_NODES: u32 = 100_000;
const MAX_RENDER_PIXELS: u64 = 32 * 1024 * 1024;
const MAX_RENDER_DEPTH: usize = 64;
const UNSUPPORTED_ELEMENTS: &[&str] = &["foreignObject", "pattern", "filter", "mask", "clipPath"];
const RASTER_FONT_TABLES: &[&[u8; 4]] = &[b"SVG ", b"sbix", b"CBDT", b"CBLC", b"EBDT", b"EBLC", b"EBSC", b"bdat", b"bloc", b"COLR", b"CPAL"];
const DEFAULT_FONT_FAMILY: &str = "Noto Sans";
const DEFAULT_FONT: &[u8] = include_bytes!("fonts/NotoSans.ttf");

pub(crate) fn rasterize(image: &Image, fonts: &[Arc<[u8]>]) -> Option<Arc<[u8]>> {
    if !image.extension.eq_ignore_ascii_case("svg") || image.bytes.len() > MAX_SVG_BYTES { return None; }
    if !valid_svg(&image.bytes) || fonts.iter().any(|font| !outline_font(font)) { return None; }
    let font_bytes = fonts.iter().try_fold(0usize, |total, font| total.checked_add(font.len()))?;
    if font_bytes > MAX_FONT_BYTES { return None; }
    let blocked = AtomicBool::new(false);
    let used = Mutex::new(0usize);
    let data_resolver = usvg::ImageHrefResolver::default_data_resolver();
    let fallback = usvg::FontResolver::default_fallback_selector();
    let mut options = usvg::Options { font_family: DEFAULT_FONT_FAMILY.into(), fontdb: default_fonts(), ..usvg::Options::default() };
    for font in fonts { options.fontdb_mut().load_font_data(font.to_vec()); }
    options.image_href_resolver.resolve_data = Box::new(|mime, data, options| {
        let within_budget = used.lock().is_ok_and(|mut total| {
            let Some(next) = total.checked_add(data.len()).filter(|next| *next <= MAX_RESOURCE_BYTES) else { return false };
            *total = next;
            true
        });
        if !within_budget { blocked.store(true, Ordering::Relaxed); return None; }
        let is_svg = mime == "image/svg+xml" || (mime == "text/plain" && imagesize::blob_size(&data).is_err());
        if is_svg && !valid_svg(&data) { blocked.store(true, Ordering::Relaxed); return None; }
        let Some(kind) = data_resolver(mime, data, options) else { blocked.store(true, Ordering::Relaxed); return None };
        let bytes = match &kind {
            usvg::ImageKind::JPEG(bytes) | usvg::ImageKind::PNG(bytes) | usvg::ImageKind::GIF(bytes) | usvg::ImageKind::WEBP(bytes) => Some(bytes),
            usvg::ImageKind::SVG(_) => None,
        };
        if let Some(bytes) = bytes {
            let valid = imagesize::blob_size(bytes).ok().is_some_and(|size| size.width.checked_mul(size.height).is_some_and(|pixels| pixels <= MAX_IMAGE_PIXELS));
            if !valid { blocked.store(true, Ordering::Relaxed); return None; }
        }
        Some(kind)
    });
    options.image_href_resolver.resolve_string = Box::new(|_, _| { blocked.store(true, Ordering::Relaxed); None });
    options.font_resolver.select_fallback = Box::new(|character, used_fonts, database| {
        let font = fallback(character, used_fonts, database);
        if font.is_none() { blocked.store(true, Ordering::Relaxed); }
        font
    });
    let tree = usvg::Tree::from_data(&image.bytes, &options).ok()?;
    if blocked.load(Ordering::Relaxed) { return None; }
    let size = tree.size();
    let scale = (MAX_SIDE as f32 / size.width()).min(MAX_SIDE as f32 / size.height()).min(1.0);
    let width = (size.width() * scale).ceil().max(1.0) as u32;
    let height = (size.height() * scale).ceil().max(1.0) as u32;
    let transform = tiny_skia::Transform::from_scale(scale, scale);
    let mut budget = u64::from(width) * u64::from(height);
    render_budget(tree.root(), transform, (width, height), 0, &mut budget)?;
    let mut pixels = tiny_skia::Pixmap::new(width, height)?;
    resvg::render(&tree, transform, &mut pixels.as_mut());
    Some(Arc::from(pixels.encode_png().ok()?))
}

fn default_fonts() -> Arc<usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut fonts = usvg::fontdb::Database::new();
        fonts.load_font_data(DEFAULT_FONT.to_vec());
        fonts.set_serif_family(DEFAULT_FONT_FAMILY);
        fonts.set_sans_serif_family(DEFAULT_FONT_FAMILY);
        fonts.set_monospace_family(DEFAULT_FONT_FAMILY);
        Arc::new(fonts)
    }).clone()
}

fn valid_svg(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else { return false };
    if text.contains("<!DOCTYPE") || text.contains("<!ENTITY") { return false; }
    let options = usvg::roxmltree::ParsingOptions { nodes_limit: MAX_SVG_NODES, ..Default::default() };
    let Ok(document) = usvg::roxmltree::Document::parse_with_options(text, options) else { return false };
    document.descendants().filter(|node| node.is_element()).all(|node| {
        let name = node.tag_name().name();
        if UNSUPPORTED_ELEMENTS.contains(&name) { return false; }
        if !["image", "feImage", "use"].contains(&name) { return true; }
        node.attributes().filter(|attribute| attribute.name() == "href").all(|attribute| {
            let href = attribute.value();
            if name == "use" { return href.starts_with('#'); }
            href.get(..5).is_some_and(|scheme| scheme.eq_ignore_ascii_case("data:"))
        })
    })
}

// Embedded SVG and bitmap font glyphs use separate decoder paths with their own resource rules.
fn outline_font(bytes: &[u8]) -> bool {
    let Some(header) = bytes.get(..12) else { return false };
    if &header[..4] != b"\0\x01\0\0" && &header[..4] != b"OTTO" { return false; }
    let count = u16::from_be_bytes([header[4], header[5]]) as usize;
    if count == 0 || count > MAX_FONT_TABLES { return false; }
    let Some(tables) = bytes.get(12..12 + count * 16) else { return false };
    let mut has_outline = false;
    for table in tables.as_chunks::<16>().0 {
        let name: &[u8; 4] = table[..4].try_into().expect("four-byte table name");
        if RASTER_FONT_TABLES.contains(&name) { return false; }
        let offset = u32::from_be_bytes(table[8..12].try_into().expect("four-byte offset")) as usize;
        let length = u32::from_be_bytes(table[12..16].try_into().expect("four-byte length")) as usize;
        if offset.checked_add(length).is_none_or(|end| end > bytes.len()) { return false; }
        has_outline |= [b"glyf", b"CFF ", b"CFF2"].contains(&name) && length > 0;
    }
    has_outline
}

// Count surfaces before rendering; nested SVGs and isolated groups allocate extra pixmaps.
fn render_budget(group: &usvg::Group, transform: tiny_skia::Transform, canvas: (u32, u32), depth: usize, budget: &mut u64) -> Option<()> {
    if depth > MAX_RENDER_DEPTH { return None; }
    let transform = transform.pre_concat(group.transform());
    let mut canvas = canvas;
    if group.should_isolate() {
        if group.clip_path().is_some() || group.mask().is_some() || !group.filters().is_empty() { return None; }
        let bounds = group.layer_bounding_box().transform(transform)?;
        let width = (bounds.width().ceil() as u64).checked_add(4)?;
        let height = (bounds.height().ceil() as u64).checked_add(4)?;
        charge_pixels(width, height, budget)?;
        canvas = (u32::try_from(width).ok()?, u32::try_from(height).ok()?);
    }
    for node in group.children() {
        match node {
            usvg::Node::Group(group) => render_budget(group, transform, canvas, depth + 1, budget)?,
            usvg::Node::Text(text) => render_budget(text.flattened(), transform, canvas, depth + 1, budget)?,
            usvg::Node::Image(image) => if let usvg::ImageKind::SVG(tree) = image.kind() {
                charge_pixels(u64::from(canvas.0), u64::from(canvas.1), budget)?;
                render_budget(tree.root(), transform, canvas, depth + 1, budget)?;
            },
            usvg::Node::Path(_) => {}
        }
    }
    Some(())
}

fn charge_pixels(width: u64, height: u64, budget: &mut u64) -> Option<()> {
    *budget = budget.checked_add(width.checked_mul(height)?)?;
    (*budget <= MAX_RENDER_PIXELS).then_some(())
}
