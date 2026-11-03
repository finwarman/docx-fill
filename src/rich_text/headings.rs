//! Package paragraph styles for the documented HTML heading mapping.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use crate::xml::{ElementKind, Token, escape, read_attribute, tokenize, unescape};
use crate::{CONTENT_TYPES_PART, add_relationship};

const RELATIONSHIP: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles";
const CONTENT_TYPE: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml";
const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const IDS: [&str; 6] = ["Title", "Heading1", "Heading2", "Heading3", "Heading4", "Heading5"];
// Concrete fallback sizes in half-points; existing template styles are never restyled.
const SIZES: [u32; 6] = [56, 40, 32, 28, 24, 22];
const TITLE_TRACKING: i32 = -10;
const APPEARANCE: &[&str] = &["w:rStyle", "w:rFonts", "w:b", "w:bCs", "w:i", "w:iCs", "w:caps", "w:smallCaps", "w:strike", "w:dstrike", "w:outline", "w:shadow", "w:emboss", "w:imprint", "w:color", "w:spacing", "w:w", "w:kern", "w:position", "w:sz", "w:szCs", "w:highlight", "w:u", "w:effect", "w:bdr", "w:shd", "w:vertAlign", "w:em"];

pub(crate) struct Headings {
    pub(super) ids: [String; 6],
    /// Each style to define, with its unique name.
    missing: BTreeMap<String, String>,
    path: String,
    relationship: Option<String>,
    relationship_ids: BTreeSet<String>,
    base: Option<String>,
}

impl Headings {
    pub(crate) fn read<'a>(main: &str, parts: impl Iterator<Item = (&'a str, &'a [u8])>) -> Self {
        let parts: BTreeMap<_, _> = parts.collect();
        let (folder, filename) = main.rsplit_once('/').unwrap_or(("", main));
        let rels = if folder.is_empty() { format!("_rels/{filename}.rels") } else { format!("{folder}/_rels/{filename}.rels") };
        let mut path = if folder.is_empty() { "styles.xml".into() } else { format!("{folder}/styles.xml") };
        let mut relationship = None;
        let mut relationship_ids = BTreeSet::new();
        if let Some(bytes) = parts.get(rels.as_str()) {
            for token in tokenize(&String::from_utf8_lossy(bytes)).unwrap_or_default() {
                let Token::Element { kind: ElementKind::Open | ElementKind::SelfClosing, name: "Relationship", raw } = token else { continue };
                let Some(id) = read_attribute(raw, "Id") else { continue };
                relationship_ids.insert(id.to_string());
                if read_attribute(raw, "Type") == Some(RELATIONSHIP) && read_attribute(raw, "TargetMode") != Some("External")
                    && let Some(target) = read_attribute(raw, "Target") {
                    path = resolve(folder, &unescape(target));
                    relationship = Some(id.to_string());
                }
            }
        }
        let styles = parts.get(path.as_str()).map(|bytes| String::from_utf8_lossy(bytes)).unwrap_or_default();
        let mut used = BTreeSet::new();
        let mut names = BTreeSet::new();
        let mut paragraphs = Vec::new();
        let mut current = None;
        let mut base = None;
        for token in tokenize(&styles).unwrap_or_default() {
            match token {
                Token::Element { kind: ElementKind::Open | ElementKind::SelfClosing, name: "w:style", raw } => {
                    current = read_attribute(raw, "w:styleId").map(unescape);
                    if let Some(id) = &current {
                        used.insert(id.clone());
                        if matches!(read_attribute(raw, "w:type"), None | Some("paragraph")) {
                            paragraphs.push((id.clone(), id.clone()));
                            if matches!(read_attribute(raw, "w:default"), Some("1" | "true" | "on")) { base = Some(id.clone()); }
                        }
                        else { current = None; }
                    }
                }
                Token::Element { kind: ElementKind::Open | ElementKind::SelfClosing, name: "w:name", raw } => {
                    if let Some(value) = read_attribute(raw, "w:val") { names.insert(normalize(&unescape(value))); }
                    if current.is_some() && let Some((_, name)) = paragraphs.last_mut()
                        && let Some(value) = read_attribute(raw, "w:val") { *name = unescape(value); }
                }
                Token::Element { kind: ElementKind::Close, name: "w:style", .. } => current = None,
                _ => {}
            }
        }
        let mut missing = BTreeMap::new();
        let ids = std::array::from_fn(|level| {
            let wanted = IDS[level];
            if let Some((id, _)) = paragraphs.iter().find(|(id, _)| id.eq_ignore_ascii_case(wanted))
                .or_else(|| paragraphs.iter().find(|(_, name)| normalize(name) == normalize(wanted))) { return id.clone(); }
            let mut id = wanted.to_string();
            if used.contains(&id) { id = format!("DocxFill{wanted}"); }
            let base = id.clone();
            let mut suffix = 1;
            while used.contains(&id) { id = format!("{base}{suffix}"); suffix += 1; }
            used.insert(id.clone());
            missing.insert(id.clone(), unique_name(level, &mut names));
            id
        });
        let base = base.or_else(|| paragraphs.iter().find(|(id, _)| id == "Normal").map(|(id, _)| id.clone()));
        Self { ids, missing, path, relationship, relationship_ids, base }
    }

    pub(crate) fn write(&self, parts: &mut Vec<(String, Cow<[u8]>)>, main: &str) {
        let mut used = BTreeSet::new();
        for (_, bytes) in parts.iter() {
            let Ok(xml) = std::str::from_utf8(bytes) else { continue };
            if !xml.contains("<w:pStyle") { continue; }
            for token in tokenize(xml).unwrap_or_default() {
                if let Token::Element { name: "w:pStyle", raw, .. } = token
                    && let Some(id) = read_attribute(raw, "w:val") { used.insert(unescape(id)); }
            }
        }
        if !self.ids.iter().any(|id| used.contains(id)) { return; }
        let mut added = String::new();
        for (level, id) in self.ids.iter().enumerate() {
            let Some(name) = self.missing.get(id) else { continue };
            if used.contains(id) { added.push_str(&definition(id, name, level, self.base.as_deref())); }
        }
        if !added.is_empty() {
            if let Some((_, bytes)) = parts.iter_mut().find(|(name, _)| name == &self.path) {
                *bytes = Cow::Owned(append_root(&String::from_utf8_lossy(bytes), "w:styles", &added).into_bytes());
            } else {
                parts.push((self.path.clone(), Cow::Owned(format!("<w:styles xmlns:w=\"{W}\">{added}</w:styles>").into_bytes())));
            }
        }
        if self.relationship.is_none() {
            let mut id = "rIdDocxFillStyles".to_string();
            let mut suffix = 1;
            while self.relationship_ids.contains(&id) { id = format!("rIdDocxFillStyles{suffix}"); suffix += 1; }
            let folder = main.rfind('/').map_or("", |at| &main[..at + 1]);
            let target = self.path.strip_prefix(folder).map_or_else(|| format!("/{}", self.path), str::to_string);
            add_relationship(parts, main, &id, &format!("<Relationship Id=\"{id}\" Type=\"{RELATIONSHIP}\" Target=\"{}\"/>", escaped(&target)));
        }
        let override_xml = format!("<Override PartName=\"/{}\" ContentType=\"{CONTENT_TYPE}\"/>", escaped(&self.path));
        if let Some((_, bytes)) = parts.iter_mut().find(|(name, _)| name == CONTENT_TYPES_PART) {
            let xml = String::from_utf8_lossy(bytes);
            let present = tokenize(&xml).unwrap_or_default().iter().any(|token| matches!(token, Token::Element { name: "Override", raw, .. } if read_attribute(raw, "PartName").map(unescape) == Some(format!("/{}", self.path))));
            if !present { *bytes = Cow::Owned(append_root(&xml, "Types", &override_xml).into_bytes()); }
        } else {
            parts.push((CONTENT_TYPES_PART.into(), Cow::Owned(format!("<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">{override_xml}</Types>").into_bytes())));
        }
    }
}

/// Keep language, proofing and other metadata while letting the paragraph style set appearance.
pub(super) fn inherited_props(props: &str) -> String {
    let Ok(tokens) = tokenize(props) else { return props.to_string() };
    let mut out = String::with_capacity(props.len());
    let mut depth = 0usize;
    let mut skipped = 0usize;
    for token in tokens {
        if skipped > 0 {
            match token {
                Token::Element { kind: ElementKind::Open, .. } => skipped += 1,
                Token::Element { kind: ElementKind::Close, .. } => skipped -= 1,
                _ => {}
            }
            continue;
        }
        if let Token::Element { kind: ElementKind::Open | ElementKind::SelfClosing, name, .. } = token
            && depth == 1 && APPEARANCE.contains(&name) {
            if matches!(token, Token::Element { kind: ElementKind::Open, .. }) { skipped = 1; }
            continue;
        }
        match token {
            Token::Element { kind: ElementKind::Open, .. } => depth += 1,
            Token::Element { kind: ElementKind::Close, .. } => depth = depth.saturating_sub(1),
            _ => {}
        }
        match token { Token::Element { raw, .. } | Token::Text(raw) => out.push_str(raw) }
    }
    out
}

pub(super) fn paragraph_props(inner: &str) -> String {
    let mut depth = 0usize;
    let mut at = 0;
    for token in tokenize(inner).unwrap_or_default() {
        if depth == 0 && matches!(token, Token::Element { name: "w:rPr", kind: ElementKind::Open | ElementKind::SelfClosing, .. })
            && let Some(end) = crate::properties::find_element_end(inner, at, "w:rPr") {
            return format!("{}{}{}", &inner[..at], inherited_props(&inner[at..end]), &inner[end..]);
        }
        match token {
            Token::Element { kind: ElementKind::Open, .. } => depth += 1,
            Token::Element { kind: ElementKind::Close, .. } => depth = depth.saturating_sub(1),
            _ => {}
        }
        at += match token { Token::Element { raw, .. } | Token::Text(raw) => raw.len() };
    }
    inner.to_string()
}

fn append_root(xml: &str, root: &str, added: &str) -> String {
    let mut depth = 0usize;
    for token in tokenize(xml).unwrap_or_default() {
        let Token::Element { kind, name, raw } = token else { continue };
        if name != root { continue; }
        let at = crate::xml::offset_in(xml, raw);
        match kind {
            ElementKind::Open => depth += 1,
            ElementKind::SelfClosing if depth == 0 => {
                let open = &raw[..raw.len() - 2];
                return format!("{}{open}>{added}</{root}>{}", &xml[..at], &xml[at + raw.len()..]);
            }
            ElementKind::Close if depth > 0 => {
                depth -= 1;
                if depth == 0 { return format!("{}{added}{}", &xml[..at], &xml[at..]); }
            }
            _ => {}
        }
    }
    xml.to_string()
}

fn normalize(name: &str) -> String { name.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect() }
fn escaped(value: &str) -> String { let mut xml = String::new(); escape(value, &mut xml); xml }

fn resolve(folder: &str, target: &str) -> String {
    let mut parts: Vec<_> = if target.starts_with('/') { Vec::new() } else { folder.split('/').filter(|part| !part.is_empty()).collect() };
    for part in target.split('/') {
        match part { "" | "." => {}, ".." => { parts.pop(); }, _ => parts.push(part) }
    }
    parts.join("/")
}

/// The built-in name for heading `level`, or a variant no other style has.
fn unique_name(level: usize, names: &mut BTreeSet<String>) -> String {
    let wanted = if level == 0 { "Title".to_string() } else { format!("heading {level}") };
    let mut name = wanted.clone();
    let mut suffix = 1;
    while names.contains(&normalize(&name)) { name = format!("{wanted} (DocxFill {suffix})"); suffix += 1; }
    names.insert(normalize(&name));
    name
}

fn definition(id: &str, name: &str, level: usize, base: Option<&str>) -> String {
    let outline = if level == 0 { String::new() } else { format!("<w:outlineLvl w:val=\"{}\"/>", level - 1) };
    let bold = if level == 0 { "<w:b w:val=\"0\"/><w:bCs w:val=\"0\"/>" } else { "<w:b/><w:bCs/>" };
    let size = SIZES[level];
    let tracking = if level == 0 { format!("<w:spacing w:val=\"{TITLE_TRACKING}\"/>") } else { String::new() };
    let base = base.map(|id| format!("<w:basedOn w:val=\"{}\"/><w:next w:val=\"{}\"/>", escaped(id), escaped(id))).unwrap_or_default();
    format!("<w:style w:type=\"paragraph\" w:styleId=\"{}\"><w:name w:val=\"{}\"/>{base}<w:qFormat/><w:pPr><w:keepNext/><w:keepLines/>{outline}</w:pPr><w:rPr><w:rFonts w:ascii=\"Arial\" w:hAnsi=\"Arial\" w:cs=\"Arial\" w:asciiTheme=\"majorHAnsi\" w:hAnsiTheme=\"majorHAnsi\" w:eastAsiaTheme=\"majorEastAsia\" w:cstheme=\"majorBidi\"/>{bold}{tracking}<w:sz w:val=\"{size}\"/><w:szCs w:val=\"{size}\"/></w:rPr></w:style>", escaped(id), escaped(name))
}
