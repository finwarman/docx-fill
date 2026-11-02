use crate::properties::{find_element_end, find_starts};
use crate::xml::{escape, read_attribute, unescape};
use super::{EMU_PER_PX, Image, ImageSize};

pub(crate) struct PictureReplacement {
    pub image: Image,
    pub size: Option<ImageSize>,
    pub tag: String,
}

pub(crate) struct PictureFrame {
    pub start: usize,
    pub replacement: Option<PictureReplacement>,
}

/// Change the picture source while preserving its existing anchor, crop and geometry.
pub(crate) fn replace_picture(xml: &str, relationship: &str, size: Option<ImageSize>, tag: &str) -> Option<String> {
    let start = *find_starts(xml, "a:blip").first()?;
    let end = find_element_end(xml, start, "a:blip")?;
    let mut out = format!("{}<a:blip r:embed=\"{relationship}\"/>{}", &xml[..start], &xml[end..]);
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
