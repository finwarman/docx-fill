//! The footnotes `{:footnote name}` tags add. Built from the WordprocessingML spec: footnotes live
//! in `word/footnotes.xml`, which the body references by relationship.

use std::borrow::Cow;

use crate::xml::{escape, read_attribute};
use crate::{CONTENT_TYPES_PART, insert_before_last};

pub(crate) const FOOTNOTES_PART: &str = "word/footnotes.xml";
const DOCUMENT_RELATIONSHIPS: &str = "word/_rels/document.xml.rels";
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
    FOOTNOTES.append(parts, written);
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
        let close = format!("</{}>", self.root);
        if let Some(part) = parts.iter_mut().find(|(name, _)| name == self.name) {
            let xml = String::from_utf8_lossy(&part.1).into_owned();
            part.1 = Cow::Owned(insert_before_last(&xml, &close, written).into_bytes());
            return;
        }

        let xml = format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<{} xmlns:w=\"{MAIN_NAMESPACE}\">{}{written}{close}", self.root, self.start);
        parts.push((self.name.to_string(), Cow::Owned(xml.into_bytes())));

        let target = self.name.strip_prefix("word/").unwrap_or(self.name);
        if let Some(rels) = parts.iter_mut().find(|(name, _)| name == DOCUMENT_RELATIONSHIPS) {
            let xml = String::from_utf8_lossy(&rels.1).into_owned();
            if !xml.contains(self.relationship) {
                let relationship = format!("<Relationship Id=\"{}\" Type=\"{}\" Target=\"{target}\"/>", self.relationship_id, self.relationship);
                rels.1 = Cow::Owned(insert_before_last(&xml, "</Relationships>", &relationship).into_bytes());
            }
        }
        if let Some(types) = parts.iter_mut().find(|(name, _)| name == CONTENT_TYPES_PART) {
            let xml = String::from_utf8_lossy(&types.1).into_owned();
            if !xml.contains(&format!("PartName=\"/{}\"", self.name)) {
                let override_ = format!("<Override PartName=\"/{}\" ContentType=\"{}\"/>", self.name, self.content_type);
                types.1 = Cow::Owned(insert_before_last(&xml, "</Types>", &override_).into_bytes());
            }
        }
    }
}
