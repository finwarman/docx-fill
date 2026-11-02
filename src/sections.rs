//! `{:subsection doc}`, `{:replacesection doc}` and `{:page props}`: a section of the body takes
//! a subtemplate's headers and footers, or new page settings. Built from the WordprocessingML
//! spec: a section's `<w:sectPr>` closes it, in its last paragraph or at the end of the body.

use std::borrow::Cow;
use std::collections::BTreeMap;

use serde_json::Value;

use crate::includes::{Subtemplate, import_section_headers};
use crate::properties::{find_element_end, find_starts, set_attribute, set_child};
use crate::render::FillHooks;
use crate::DOCUMENT_PART;

/// Marks a section tag in the filled body: the start, its index, then the end.
pub(crate) const SECTION_START: char = '\u{E007}';
pub(crate) const SECTION_END: char = '\u{E008}';

/// `<w:sectPr>` children in schema order.
const SECTION_ORDER: &[&str] = &[
    "w:headerReference", "w:footerReference", "w:footnotePr", "w:endnotePr", "w:type", "w:pgSz", "w:pgMar", "w:paperSrc", "w:pgBorders",
    "w:lnNumType", "w:pgNumType", "w:cols", "w:formProt", "w:vAlign", "w:noEndnote", "w:titlePg", "w:textDirection", "w:bidi", "w:rtlGutter",
    "w:docGrid", "w:printerSettings", "w:sectPrChange",
];
const REFERENCES: [&str; 2] = ["w:headerReference", "w:footerReference"];
const REFERENCE_TYPES: [&str; 3] = ["default", "first", "even"];
const MARGINS: [&str; 7] = ["header", "footer", "top", "left", "right", "bottom", "gutter"];
// Paper sizes in twips, portrait.
const SIZES: [(&str, [u32; 2]); 10] = [
    ("A3", [16838, 23811]),
    ("A4", [11906, 16838]),
    ("A5", [8391, 11906]),
    ("A6", [5953, 8391]),
    ("B4", [14173, 20012]),
    ("B5", [9979, 14173]),
    ("Letter", [12240, 15840]),
    ("Legal", [12240, 20160]),
    ("Executive", [10440, 15120]),
    ("Tabloid", [15840, 24480]),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SectionTag {
    /// Ends the section before the tag, so the rest of it takes the subtemplate's headers.
    Subsection,
    /// The whole section takes the subtemplate's headers.
    Replace,
    /// The section takes the value's orientation, size and margins.
    Page,
}

/// A section tag filled: its kind, value and scope.
#[derive(Debug, Clone)]
pub(crate) struct SectionUse {
    pub tag: SectionTag,
    pub value: Value,
    pub scope: Value,
}

/// Applies the body's section tags at their marks, last first so earlier marks keep their place.
pub(crate) fn apply_sections(parts: &mut Vec<(String, Cow<[u8]>)>, uses: &[SectionUse], includes: &BTreeMap<String, Subtemplate>, hooks: &mut dyn FillHooks, mut count: usize, errors: &mut Vec<crate::FillError>) {
    if uses.is_empty() {
        return;
    }
    let Some(index) = parts.iter().position(|(name, _)| name == DOCUMENT_PART) else { return };
    let mut xml = String::from_utf8_lossy(&parts[index].1).into_owned();
    for (index, used) in uses.iter().enumerate().rev() {
        let mark = format!("{SECTION_START}{index}{SECTION_END}");
        let Some(at) = xml.find(&mark) else { continue };
        xml.replace_range(at..at + mark.len(), "");
        let Some(start) = find_starts(&xml, "w:sectPr").into_iter().find(|start| *start >= at) else { continue };
        let Some(end) = find_element_end(&xml, start, "w:sectPr") else { continue };
        let section = xml[start..end].to_string();

        if used.tag == SectionTag::Page {
            xml.replace_range(start..end, &set_page(&section, &used.value));
            continue;
        }
        let Some((name, include)) = used.value.as_str().and_then(|name| includes.get(name).map(|include| (name, include))) else { continue };
        count += 1;
        let (references, is_title_page) = import_section_headers(parts, include, name, &used.scope, hooks, count, errors);

        // The next section inherits what this one had, not the subtemplate's.
        if let Some(next) = find_starts(&xml, "w:sectPr").into_iter().find(|next| *next >= end)
            && let Some(next_end) = find_element_end(&xml, next, "w:sectPr")
        {
            let carried = carry_references(&section, &xml[next..next_end]);
            xml.replace_range(next..next_end, &carried);
        }
        xml.replace_range(start..end, &set_references(&section, &references, is_title_page));
        if used.tag == SectionTag::Subsection {
            xml.insert_str(at, &format!("<w:p><w:pPr>{section}</w:pPr></w:p>"));
        }
    }
    parts[index].1 = Cow::Owned(xml.into_bytes());
}

/// `section`'s content rewritten by `edit`, a self-closing one opened.
fn edit_section(section: &str, edit: impl FnOnce(&str) -> String) -> String {
    let open_end = section.find('>').map_or(section.len(), |at| at + 1);
    let (open, inner) = match section[..open_end].strip_suffix("/>") {
        Some(open) => (format!("{open}>"), ""),
        None => (section[..open_end].to_string(), &section[open_end..section.len() - "</w:sectPr>".len()]),
    };
    format!("{open}{}</w:sectPr>", edit(inner))
}

/// The header and footer references of a section, by element and type.
fn read_references(section: &str) -> Vec<(&'static str, String, String)> {
    let mut references = Vec::new();
    for element in REFERENCES {
        for start in find_starts(section, element) {
            let Some(end) = find_element_end(section, start, element) else { continue };
            let raw = &section[start..end];
            let kind = crate::xml::read_attribute(raw, "w:type").unwrap_or("default").to_string();
            references.push((element, kind, raw.to_string()));
        }
    }
    references
}

fn set_references(section: &str, references: &str, is_title_page: bool) -> String {
    edit_section(section, |inner| {
        let mut inner = inner.to_string();
        for element in REFERENCES {
            inner = crate::properties::edit_each(&inner, element, |_| String::new());
        }
        let title_page = if is_title_page { "<w:titlePg/>" } else { "" };
        inner = set_child(&inner, "w:titlePg", title_page, SECTION_ORDER);
        format!("{references}{inner}")
    })
}

/// `next` with each of `section`'s references it lacks a reference of that element and type for.
fn carry_references(section: &str, next: &str) -> String {
    let own = read_references(next);
    let carried: String = read_references(section)
        .into_iter()
        .filter(|(element, kind, _)| REFERENCE_TYPES.contains(&kind.as_str()) && !own.iter().any(|(each, each_kind, _)| each == element && each_kind == kind))
        .map(|(_, _, raw)| raw)
        .collect();
    if carried.is_empty() {
        return next.to_string();
    }
    edit_section(next, |inner| format!("{carried}{inner}"))
}

/// `section` with the page value's `orientation`, `size` and `margins` (twips).
fn set_page(section: &str, page: &Value) -> String {
    let Some(page) = page.as_object() else { return section.to_string() };
    edit_section(section, |inner| {
        let mut inner = inner.to_string();
        let size_raw = read_raw(&inner, "w:pgSz").unwrap_or_else(|| "<w:pgSz/>".to_string());
        let attribute = |name: &str| crate::xml::read_attribute(&size_raw, name).and_then(|value| value.parse::<u32>().ok());
        let mut size = [attribute("w:w").unwrap_or(SIZES[1].1[0]), attribute("w:h").unwrap_or(SIZES[1].1[1])];
        match page.get("size") {
            Some(Value::String(name)) => {
                if let Some((_, named)) = SIZES.iter().find(|(each, _)| each.eq_ignore_ascii_case(name)) {
                    size = *named;
                }
            }
            Some(Value::Array(pair)) => {
                if let [Some(width), Some(height)] = [pair.first(), pair.get(1)].map(|each| each.and_then(Value::as_u64)) {
                    size = [width as u32, height as u32];
                }
            }
            _ => {}
        }
        let orientation = page.get("orientation").and_then(Value::as_str).or_else(|| crate::xml::read_attribute(&size_raw, "w:orient"));
        let is_landscape = orientation == Some("landscape");
        if is_landscape == (size[0] < size[1]) && page.contains_key("orientation") {
            size.swap(0, 1);
        }
        let mut raw = set_attribute(&size_raw, "w:w", &size[0].to_string());
        raw = set_attribute(&raw, "w:h", &size[1].to_string());
        if orientation.is_some() {
            raw = set_attribute(&raw, "w:orient", if is_landscape { "landscape" } else { "portrait" });
        }
        inner = set_child(&inner, "w:pgSz", &raw, SECTION_ORDER);

        if let Some(margins) = page.get("margins").and_then(Value::as_object) {
            let mut raw = read_raw(&inner, "w:pgMar").unwrap_or_else(|| "<w:pgMar/>".to_string());
            for name in MARGINS {
                if let Some(value) = margins.get(name).and_then(Value::as_i64) {
                    raw = set_attribute(&raw, &format!("w:{name}"), &value.to_string());
                }
            }
            inner = set_child(&inner, "w:pgMar", &raw, SECTION_ORDER);
        }
        inner
    })
}

fn read_raw(inner: &str, name: &str) -> Option<String> {
    let start = find_starts(inner, name).into_iter().next()?;
    Some(inner[start..find_element_end(inner, start, name)?].to_string())
}
