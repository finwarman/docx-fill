//! The footnotes `{:footnote name}` tags add. Built from the WordprocessingML spec: footnotes live
//! in `word/footnotes.xml`, which the body references by relationship.

use std::borrow::Cow;
use std::collections::BTreeSet;

use crate::part_kinds::{PartKind, PartKinds};
use crate::xml::{ElementKind, Token, escape, read_attribute, tokenize, unescape};
use crate::{CONTENT_TYPES_PART, insert_before_last};

pub(crate) const FOOTNOTES_PART: &str = "word/footnotes.xml";
const FOOTNOTES_RELATIONSHIP: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes";
const FOOTNOTES_CONTENT_TYPE: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml";
pub(crate) const MAIN_NAMESPACE: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
// Word's separator footnotes, which a new footnotes part needs before any note.
const SEPARATORS: &str = "<w:footnote w:type=\"separator\" w:id=\"-1\"><w:p><w:r><w:separator/></w:r></w:p></w:footnote>\
<w:footnote w:type=\"continuationSeparator\" w:id=\"0\"><w:p><w:r><w:continuationSeparator/></w:r></w:p></w:footnote>";

/// Whether a footnote value is text or complete Word runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FootnoteContent { #[default] Text, Raw }

/// Formatting fragments for newly inserted footnotes.
#[derive(Debug, Clone, Default)]
pub struct FootnoteOptions {
    pub content: FootnoteContent,
    pub ref_properties: Option<String>,
    pub ref_run: Option<String>,
    pub paragraph_properties: Option<String>,
    pub run_properties: Option<String>,
}

pub(crate) fn write_note(id: u32, text: &str, options: &FootnoteOptions) -> String {
    let paragraph = options.paragraph_properties.as_deref().unwrap_or("<w:pStyle w:val=\"FootnoteText\"/>");
    let reference = options.ref_properties.as_deref().unwrap_or("<w:vertAlign w:val=\"superscript\"/>");
    let mut out = format!("<w:footnote w:id=\"{id}\"><w:p><w:pPr>{paragraph}</w:pPr><w:r><w:rPr>{reference}</w:rPr><w:footnoteRef/></w:r>");
    if let Some(run) = &options.ref_run { out.push_str(run); }
    match options.content {
        FootnoteContent::Raw => {
            if options.ref_run.is_none() { out.push_str("<w:r><w:t xml:space=\"preserve\"> </w:t></w:r>"); }
            out.push_str(text);
        }
        FootnoteContent::Text => {
            out.push_str("<w:r>");
            if let Some(properties) = &options.run_properties { out.push_str(&format!("<w:rPr>{properties}</w:rPr>")); }
            out.push_str("<w:t xml:space=\"preserve\">");
            if options.ref_run.is_none() { out.push(' '); }
            escape(text, &mut out);
            out.push_str("</w:t></w:r>");
        }
    }
    out.push_str("</w:p></w:footnote>");
    out
}

/// The largest footnote id in a footnotes part, at least 0.
pub(crate) fn read_max_id(xml: &str) -> u32 {
    read_max_element_id(xml, "w:footnote")
}

/// The largest `w:id` of the `name` elements in `xml`, at least 0.
pub(crate) fn read_max_element_id(xml: &str, name: &str) -> u32 {
    let mut max = 0;
    let mut rest = xml;
    let start = format!("<{name} ");
    while let Some(at) = rest.find(&start) {
        rest = &rest[at + 1..];
        let raw = &rest[..rest.find('>').unwrap_or(rest.len())];
        if let Some(id) = read_attribute(raw, "w:id").and_then(|id| id.parse::<u32>().ok()) {
            max = max.max(id);
        }
    }
    max
}

/// Adds each footnote to the footnotes part, creating the part, its relationship and content
/// type when the package has none.
pub(crate) fn add_footnotes(parts: &mut Vec<(String, Cow<[u8]>)>, notes: &[(u32, String)]) {
    if notes.is_empty() {
        return;
    }
    let mut written = String::new();
    for (_, note) in notes {
        written.push_str(note);
    }
    append_notes(parts, &written);
}

/// Adds `written`, whole `<w:footnote>` elements, to the footnotes part, creating the part, its
/// relationship and content type when the package has none.
pub(crate) fn append_notes(parts: &mut Vec<(String, Cow<[u8]>)>, written: &str) {
    let (main, target) = location(parts.iter().map(|(name, bytes)| (name.as_str(), bytes.as_ref())));
    FOOTNOTES.append_at(parts, written, &main, &target, Metadata::Ensure);
}

#[derive(PartialEq, Eq)]
enum Metadata { CreateOnly, Ensure }

pub(crate) fn target<'a>(parts: impl Iterator<Item = (&'a str, &'a [u8])>) -> String {
    location(parts).1
}

fn location<'a>(parts: impl Iterator<Item = (&'a str, &'a [u8])>) -> (String, String) {
    let parts = parts.collect::<Vec<_>>();
    let types = parts.iter().find(|(name, _)| *name == CONTENT_TYPES_PART).and_then(|(_, bytes)| std::str::from_utf8(bytes).ok());
    let kinds = PartKinds::read(types);
    let main = parts.iter().find(|(name, _)| kinds.kind(name) == PartKind::Main).map_or(crate::DOCUMENT_PART, |(name, _)| *name);
    let rels_path = crate::includes::relationships_of(main);
    let rels = parts.iter().find(|(name, _)| *name == rels_path).and_then(|(_, bytes)| std::str::from_utf8(bytes).ok());
    let target = rels.and_then(|xml| tokenize(xml).ok()).unwrap_or_default().into_iter().find_map(|token| {
        let Token::Element { kind: ElementKind::Open | ElementKind::SelfClosing, name: "Relationship", raw } = token else { return None };
        if read_attribute(raw, "Type") != Some(FOOTNOTES_RELATIONSHIP) || read_attribute(raw, "TargetMode") == Some("External") { return None; }
        let target = unescape(read_attribute(raw, "Target")?);
        (!target.is_empty()).then(|| crate::includes::resolve(main, &target))
    }).unwrap_or_else(|| crate::includes::resolve(main, "footnotes.xml"));
    (main.to_string(), target)
}

/// A part of the body's notes, such as footnotes or comments.
pub(crate) struct NotesPart {
    pub name: &'static str,
    pub root: &'static str,
    pub relationship: &'static str,
    pub relationship_id: &'static str,
    pub content_type: &'static str,
    /// What a new part holds before any note.
    pub start: &'static str,
}

const FOOTNOTES: NotesPart = NotesPart {
    name: FOOTNOTES_PART,
    root: "w:footnotes",
    relationship: FOOTNOTES_RELATIONSHIP,
    relationship_id: "rIdDocxFillFootnotes",
    content_type: FOOTNOTES_CONTENT_TYPE,
    start: SEPARATORS,
};

impl NotesPart {
    /// Adds `written`, whole note elements, creating the part, its relationship and content type
    /// when the package has none.
    pub(crate) fn append(&self, parts: &mut Vec<(String, Cow<[u8]>)>, written: &str) {
        self.append_at(parts, written, crate::DOCUMENT_PART, self.name, Metadata::CreateOnly);
    }

    fn append_at(&self, parts: &mut Vec<(String, Cow<[u8]>)>, written: &str, main: &str, target: &str, metadata: Metadata) {
        let close = format!("</{}>", self.root);
        if let Some(part) = parts.iter_mut().find(|(name, _)| name == target) {
            let xml = String::from_utf8_lossy(&part.1).into_owned();
            part.1 = Cow::Owned(insert_before_last(&xml, &close, written).into_bytes());
            if metadata == Metadata::CreateOnly { return; }
        } else {
            let xml = format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<{} xmlns:w=\"{MAIN_NAMESPACE}\">{}{written}{close}", self.root, self.start);
            parts.push((target.to_string(), Cow::Owned(xml.into_bytes())));
        }

        let rels_path = crate::includes::relationships_of(main);
        let mut relative = String::new();
        escape(&crate::includes::relative_to(main, target), &mut relative);
        let xml = parts.iter().find(|(name, _)| *name == rels_path).map(|(_, bytes)| String::from_utf8_lossy(bytes).into_owned()).unwrap_or_default();
        let relationships = tokenize(&xml).unwrap_or_default().into_iter().filter_map(|token| match token {
            Token::Element { kind: ElementKind::Open | ElementKind::SelfClosing, name: "Relationship", raw } => Some(raw),
            _ => None,
        }).collect::<Vec<_>>();
        let related = |raw: &&str| read_attribute(raw, "Type") == Some(self.relationship);
        let linked = relationships.iter().any(|raw| related(raw) && (metadata == Metadata::CreateOnly || read_attribute(raw, "TargetMode") != Some("External")));
        let external = relationships.iter().find(|raw| related(raw) && read_attribute(raw, "TargetMode") == Some("External"));
        if !linked && let Some(raw) = external {
            let replacement = crate::properties::set_attribute(&crate::properties::set_attribute(raw, "Target", &relative), "TargetMode", "Internal");
            let start = crate::xml::offset_in(&xml, raw);
            let mut updated = xml.clone();
            updated.replace_range(start..start + raw.len(), &replacement);
            if let Some((_, bytes)) = parts.iter_mut().find(|(name, _)| *name == rels_path) { *bytes = Cow::Owned(updated.into_bytes()); }
        } else if !linked {
            let ids = relationships.iter().filter_map(|raw| read_attribute(raw, "Id")).collect::<BTreeSet<_>>();
            let id = (0..).map(|suffix| if suffix == 0 { self.relationship_id.to_string() } else { format!("{}{suffix}", self.relationship_id) }).find(|id| !ids.contains(id.as_str())).expect("finite relationships");
            let relationship = format!("<Relationship Id=\"{id}\" Type=\"{}\" Target=\"{relative}\"/>", self.relationship);
            crate::add_relationship(parts, main, &id, &relationship);
        }
        if let Some(types) = parts.iter_mut().find(|(name, _)| name == CONTENT_TYPES_PART) {
            let xml = String::from_utf8_lossy(&types.1).into_owned();
            let mut path = String::new();
            escape(target, &mut path);
            let exists = tokenize(&xml).unwrap_or_default().into_iter().any(|token| matches!(token,
                Token::Element { kind: ElementKind::Open | ElementKind::SelfClosing, name: "Override", raw }
                    if read_attribute(raw, "PartName").is_some_and(|name| unescape(name) == format!("/{target}"))));
            if !exists {
                let override_ = format!("<Override PartName=\"/{path}\" ContentType=\"{}\"/>", self.content_type);
                types.1 = Cow::Owned(insert_before_last(&xml, "</Types>", &override_).into_bytes());
            }
        }
    }
}
