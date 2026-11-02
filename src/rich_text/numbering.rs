//! Package numbering definitions for HTML lists.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use super::{LIST_INDENT_TWIPS, MAX_LIST_LEVEL, Numbering};
use crate::cell_styling::find_element_start;
use crate::xml::{ElementKind, Token, escape, read_attribute, tokenize};
use crate::{CONTENT_TYPES_PART, add_relationship, insert_before_last};

const RELATIONSHIP: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering";
const CONTENT_TYPE: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml";
const MAIN_NAMESPACE: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const FORMAT_BULLET: &str = "bullet";
const FORMAT_DECIMAL: &str = "decimal";
const FORMAT_UPPER_LETTER: &str = "upperLetter";
const FORMAT_LOWER_LETTER: &str = "lowerLetter";
const FORMAT_UPPER_ROMAN: &str = "upperRoman";
const FORMAT_LOWER_ROMAN: &str = "lowerRoman";

struct Definition {
    id: u32,
    abstract_id: u32,
    format: Numbering,
    level: usize,
    start: i64,
}

pub(crate) struct Numberings {
    path: String,
    relationship_id: Option<String>,
    relationship_ids: BTreeSet<String>,
    used_numbers: BTreeSet<u32>,
    used_abstracts: BTreeSet<u32>,
    definitions: Vec<Definition>,
    next_number: u32,
    next_abstract: u32,
}

impl Numberings {
    pub(crate) fn read<'a>(main: &str, parts: impl Iterator<Item = (&'a str, &'a [u8])>) -> Self {
        let parts: BTreeMap<_, _> = parts.collect();
        let folder = main.rsplit_once('/').map_or("", |(folder, _)| folder);
        let filename = main.rsplit('/').next().unwrap_or(main);
        let rels_path = if folder.is_empty() { format!("_rels/{filename}.rels") } else { format!("{folder}/_rels/{filename}.rels") };
        let mut path = if folder.is_empty() { "numbering.xml".to_string() } else { format!("{folder}/numbering.xml") };
        let mut relationship_id = None;
        let mut relationship_ids = BTreeSet::new();
        if let Some(bytes) = parts.get(rels_path.as_str()) {
            let xml = String::from_utf8_lossy(bytes);
            for token in tokenize(&xml).unwrap_or_default() {
                let Token::Element { kind: ElementKind::Open | ElementKind::SelfClosing, name: "Relationship", raw } = token else { continue };
                let Some(id) = read_attribute(raw, "Id") else { continue };
                relationship_ids.insert(id.to_string());
                if read_attribute(raw, "Type") == Some(RELATIONSHIP) && read_attribute(raw, "TargetMode") != Some("External")
                    && let Some(target) = read_attribute(raw, "Target") {
                    path = resolve_part(folder, target);
                    relationship_id = Some(id.to_string());
                }
            }
        }
        let mut used_numbers = BTreeSet::new();
        let mut used_abstracts = BTreeSet::new();
        if let Some(bytes) = parts.get(path.as_str()) {
            let xml = String::from_utf8_lossy(bytes);
            for token in tokenize(&xml).unwrap_or_default() {
                let Token::Element { kind: ElementKind::Open | ElementKind::SelfClosing, name, raw } = token else { continue };
                let (attribute, ids) = match name {
                    "w:num" => ("w:numId", &mut used_numbers),
                    "w:abstractNum" => ("w:abstractNumId", &mut used_abstracts),
                    _ => continue,
                };
                if let Some(id) = read_attribute(raw, attribute).and_then(|value| value.parse().ok()) {
                    ids.insert(id);
                }
            }
        }
        Self { path, relationship_id, relationship_ids, used_numbers, used_abstracts, definitions: Vec::new(), next_number: 1, next_abstract: 0 }
    }

    pub(super) fn add(&mut self, format: Numbering, level: usize, start: i64) -> Option<u32> {
        let id = free_id(&mut self.used_numbers, &mut self.next_number)?;
        let abstract_id = free_id(&mut self.used_abstracts, &mut self.next_abstract)?;
        self.definitions.push(Definition { id, abstract_id, format, level: level.min(MAX_LIST_LEVEL), start });
        Some(id)
    }

    pub(crate) fn write(self, parts: &mut Vec<(String, Cow<[u8]>)>, main: &str) {
        if self.definitions.is_empty() {
            return;
        }
        let mut abstracts = String::new();
        let mut numbers = String::new();
        for definition in &self.definitions {
            abstracts.push_str(&definition.abstract_xml());
            numbers.push_str(&format!("<w:num w:numId=\"{}\"><w:abstractNumId w:val=\"{}\"/></w:num>", definition.id, definition.abstract_id));
        }
        if let Some((_, bytes)) = parts.iter_mut().find(|(name, _)| *name == self.path) {
            let xml = open_root(&String::from_utf8_lossy(bytes), "w:numbering");
            let at = find_element_start(&xml, "<w:num").unwrap_or_else(|| xml.rfind("</w:numbering>").unwrap_or(xml.len()));
            let xml = format!("{}{abstracts}{}", &xml[..at], &xml[at..]);
            let at = find_element_start(&xml, "<w:numIdMacAtCleanup").unwrap_or_else(|| xml.rfind("</w:numbering>").unwrap_or(xml.len()));
            *bytes = Cow::Owned(format!("{}{numbers}{}", &xml[..at], &xml[at..]).into_bytes());
        } else {
            let xml = format!("<w:numbering xmlns:w=\"{MAIN_NAMESPACE}\">{abstracts}{numbers}</w:numbering>");
            parts.push((self.path.clone(), Cow::Owned(xml.into_bytes())));
        }
        if self.relationship_id.is_none() {
            let mut suffix = 0;
            let mut id = "rIdDocxFillNumbering".to_string();
            while self.relationship_ids.contains(&id) {
                suffix += 1;
                id = format!("rIdDocxFillNumbering{suffix}");
            }
            let folder = main.rfind('/').map_or("", |at| &main[..at + 1]);
            let target = self.path.strip_prefix(folder).map_or_else(|| format!("/{}", self.path), str::to_string);
            let mut escaped = String::new();
            escape(&target, &mut escaped);
            add_relationship(parts, main, &id, &format!("<Relationship Id=\"{id}\" Type=\"{RELATIONSHIP}\" Target=\"{escaped}\"/>"));
        }
        add_content_type(parts, &self.path);
    }
}

impl Definition {
    fn abstract_xml(&self) -> String {
        let format = match self.format {
            Numbering::Bullet => FORMAT_BULLET,
            Numbering::Decimal => FORMAT_DECIMAL,
            Numbering::UpperAlpha => FORMAT_UPPER_LETTER,
            Numbering::LowerAlpha => FORMAT_LOWER_LETTER,
            Numbering::UpperRoman => FORMAT_UPPER_ROMAN,
            Numbering::LowerRoman => FORMAT_LOWER_ROMAN,
        };
        let mut xml = format!("<w:abstractNum w:abstractNumId=\"{}\"><w:multiLevelType w:val=\"multilevel\"/>", self.abstract_id);
        for level in 0..=self.level {
            let start = if level == self.level { self.start } else { 1 };
            let marker = if matches!(self.format, Numbering::Bullet) { "•".to_string() } else { format!("%{}.", level + 1) };
            let indent = LIST_INDENT_TWIPS as usize * (level + 1);
            xml.push_str(&format!("<w:lvl w:ilvl=\"{level}\"><w:start w:val=\"{start}\"/><w:numFmt w:val=\"{format}\"/><w:lvlText w:val=\"{marker}\"/><w:lvlJc w:val=\"left\"/><w:pPr><w:tabs><w:tab w:val=\"num\" w:pos=\"{indent}\"/></w:tabs><w:ind w:left=\"{indent}\" w:hanging=\"{LIST_INDENT_TWIPS}\"/></w:pPr></w:lvl>"));
        }
        xml.push_str("</w:abstractNum>");
        xml
    }
}

fn free_id(ids: &mut BTreeSet<u32>, next: &mut u32) -> Option<u32> {
    let mut id = *next;
    while ids.contains(&id) {
        id = id.checked_add(1)?;
    }
    ids.insert(id);
    *next = id.saturating_add(1);
    Some(id)
}

fn resolve_part(folder: &str, target: &str) -> String {
    let mut pieces: Vec<&str> = if target.starts_with('/') { Vec::new() } else { folder.split('/').filter(|piece| !piece.is_empty()).collect() };
    for piece in target.split('/') {
        match piece {
            "" | "." => {}
            ".." => { pieces.pop(); }
            _ => pieces.push(piece),
        }
    }
    pieces.join("/")
}

fn open_root(xml: &str, name: &str) -> String {
    let Some(start) = find_element_start(xml, &format!("<{name}")) else { return xml.to_string() };
    let Some(end) = xml[start..].find('>').map(|at| start + at) else { return xml.to_string() };
    let opening = xml[start..end].trim_end();
    let Some(opening) = opening.strip_suffix('/') else { return xml.to_string() };
    format!("{}{opening}></{name}>{}", &xml[..start], &xml[end + 1..])
}

fn add_content_type(parts: &mut Vec<(String, Cow<[u8]>)>, path: &str) {
    let name = format!("/{path}");
    let mut escaped = String::new();
    escape(&name, &mut escaped);
    let added = format!("<Override PartName=\"{escaped}\" ContentType=\"{CONTENT_TYPE}\"/>");
    if let Some((_, bytes)) = parts.iter_mut().find(|(name, _)| name == CONTENT_TYPES_PART) {
        let xml = open_root(&String::from_utf8_lossy(bytes), "Types");
        let present = tokenize(&xml).unwrap_or_default().iter().any(|token| matches!(token, Token::Element { name: "Override", raw, .. } if read_attribute(raw, "PartName") == Some(name.as_str())));
        if !present {
            *bytes = Cow::Owned(insert_before_last(&xml, "</Types>", &added).into_bytes());
        }
        return;
    }
    let xml = format!("<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">{added}</Types>");
    parts.push((CONTENT_TYPES_PART.to_string(), Cow::Owned(xml.into_bytes())));
}
