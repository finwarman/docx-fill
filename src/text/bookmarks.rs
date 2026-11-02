use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use super::FilledDocument;
use crate::xml::{ElementKind, Token, check_balanced, read_attribute, tokenize, unescape};
use crate::FillError;

/// A named bookmark in a package part. Offsets are resolved again after each edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark {
    pub name: String,
    pub id: String,
    pub part: String,
}

struct Range { start: usize, end: usize }

impl FilledDocument {
    /// Bookmarks with both range marks, in package and document order.
    pub fn bookmarks(&self) -> Vec<Bookmark> {
        let mut found = Vec::new();
        for (part, bytes) in &self.parts {
            let Ok(xml) = std::str::from_utf8(bytes) else { continue };
            for (bookmark, _) in ranges(xml, part) { found.push(bookmark); }
        }
        found
    }

    /// The XML between a bookmark's marks, without the marks themselves.
    pub fn get_xml(&self, bookmark: &Bookmark) -> Result<String, FillError> {
        let xml = self.read_part(&bookmark.part).ok_or_else(|| FillError::new("Bookmark part no longer exists"))?;
        let range = find_range(&xml, bookmark)?;
        Ok(xml[range.start..range.end].to_string())
    }

    /// Replace the bookmark's content, preserving its boundary marks.
    /// The resulting part must remain balanced XML with paired bookmarks.
    pub fn replace_xml(&mut self, bookmark: &Bookmark, replacement: &str) -> Result<(), FillError> {
        let xml = self.read_part(&bookmark.part).ok_or_else(|| FillError::new("Bookmark part no longer exists"))?;
        let range = find_range(&xml, bookmark)?;
        let changed = format!("{}{replacement}{}", &xml[..range.start], &xml[range.end..]);
        let tokens = tokenize(&changed).map_err(FillError::new)?;
        check_balanced(&tokens).map_err(FillError::new)?;
        check_bookmarks(&tokens)?;
        let part = self.parts.iter_mut().find(|(name, _)| name == &bookmark.part).ok_or_else(|| FillError::new("Bookmark part no longer exists"))?;
        part.1 = Cow::Owned(changed.into_bytes());
        Ok(())
    }
}

fn find_range(xml: &str, bookmark: &Bookmark) -> Result<Range, FillError> {
    ranges(xml, &bookmark.part).into_iter().find(|(found, _)| found == bookmark).map(|(_, range)| range)
        .ok_or_else(|| FillError::new("Bookmark no longer exists"))
}

fn ranges(xml: &str, part: &str) -> Vec<(Bookmark, Range)> {
    let Ok(tokens) = tokenize(xml) else { return Vec::new() };
    let mut opened = BTreeMap::new();
    let mut found = Vec::new();
    let mut at = 0;
    for token in tokens {
        let length = match &token { Token::Element { raw, .. } => raw.len(), Token::Text(text) => text.len() };
        match token {
            Token::Element { kind: kind @ (ElementKind::Open | ElementKind::SelfClosing), name: "w:bookmarkStart", raw } => {
                if let (Some(id), Some(name)) = (read_attribute(raw, "w:id"), read_attribute(raw, "w:name")) {
                    let start = if kind == ElementKind::Open {
                        xml[at + length..].find("</w:bookmarkStart>").map_or(at + length, |end| at + length + end + "</w:bookmarkStart>".len())
                    } else { at + length };
                    opened.insert(id.to_string(), (unescape(name), start));
                }
            }
            Token::Element { kind: ElementKind::Open | ElementKind::SelfClosing, name: "w:bookmarkEnd", raw } => {
                if let Some(id) = read_attribute(raw, "w:id")
                    && let Some((name, start)) = opened.remove(id) {
                    found.push((Bookmark { name, id: id.to_string(), part: part.to_string() }, Range { start, end: at }));
                }
            }
            _ => {}
        }
        at += length;
    }
    found.sort_by_key(|(_, range)| range.start);
    found
}

fn check_bookmarks(tokens: &[Token<'_>]) -> Result<(), FillError> {
    let mut balance = BTreeMap::new();
    let mut starts = BTreeSet::new();
    for token in tokens {
        let Token::Element { name: name @ ("w:bookmarkStart" | "w:bookmarkEnd"), raw, kind: ElementKind::Open | ElementKind::SelfClosing } = token else { continue };
        let Some(id) = read_attribute(raw, "w:id") else { continue };
        let start = *name == "w:bookmarkStart";
        if start && !starts.insert(id) { return Err(FillError::new("Replacement duplicates a bookmark id")); }
        let count = balance.entry(id).or_insert(0i32);
        *count += if start { 1 } else { -1 };
        if *count < 0 { return Err(FillError::new("Replacement leaves an unpaired bookmark")); }
    }
    if balance.values().any(|count| *count != 0) { return Err(FillError::new("Replacement leaves an unpaired bookmark")); }
    Ok(())
}
