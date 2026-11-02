use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use super::{FilledDocument, TextPosition, read_body_text};
use crate::properties::{find_element_end, set_child};
use crate::xml::{ElementKind, Token, check_balanced, escape, read_attribute, tokenize, unescape};
use crate::{FillError, FillOptions, TagType, Template, TemplateTag, add_relationship};

const APP_PART: &str = "docProps/app.xml";
const APP_NAMESPACE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/extended-properties";
const APP_RELATIONSHIP: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties";
const APP_CONTENT_TYPE: &str = "application/vnd.openxmlformats-officedocument.extended-properties+xml";
const CONTENT_NAMESPACE: &str = "http://schemas.openxmlformats.org/package/2006/content-types";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentBoundary { Start, End, PageBreak(usize) }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageBreak { pub index: usize, pub position: Option<TextPosition> }

struct BodyRange { start: usize, end: usize, breaks: Vec<(usize, usize)> }
struct OpenElement<'a> { at: usize, name: &'a str, raw: &'a str }

impl FilledDocument {
    /// Set the package's base URL for relative hyperlinks.
    pub fn set_hyperlink_base(&mut self, url: &str) {
        let mut escaped = String::new();
        escape(url, &mut escaped);
        let rels = self.read_part("_rels/.rels").unwrap_or_default();
        let path = tokenize(&rels).unwrap_or_default().iter().find_map(|token| match token {
            Token::Element { name, raw, .. } if name.rsplit(':').next() == Some("Relationship") && read_attribute(raw, "Type") == Some(APP_RELATIONSHIP) => read_attribute(raw, "Target").map(|target| unescape(target).trim_start_matches('/').to_string()),
            _ => None,
        }).unwrap_or_else(|| APP_PART.to_string());
        update_child(&mut self.parts, &path, "Properties", APP_NAMESPACE, "HyperlinkBase", &format!("<HyperlinkBase>{escaped}</HyperlinkBase>"));
        let mut escaped_path = String::new();
        escape(&path, &mut escaped_path);
        update_child(&mut self.parts, "[Content_Types].xml", "Types", CONTENT_NAMESPACE, "", &format!("<Override PartName=\"/{escaped_path}\" ContentType=\"{APP_CONTENT_TYPE}\"/>"));
        if !rels.contains(APP_RELATIONSHIP) {
            let mut index = 0;
            let mut id = "rIdDocxFillApp".to_string();
            while rels.contains(&format!("Id=\"{id}\"")) { index += 1; id = format!("rIdDocxFillApp{index}"); }
            add_relationship(&mut self.parts, "", &id, &format!("<Relationship Id=\"{id}\" Type=\"{APP_RELATIONSHIP}\" Target=\"{APP_PART}\"/>"));
        }
    }

    /// Explicit page-break elements, in document order. Automatic pagination is not counted.
    pub fn page_breaks(&self) -> Vec<PageBreak> {
        let xml = self.read_document();
        let Ok(range) = body_range(&xml) else { return Vec::new() };
        let text = read_body_text(&xml);
        range.breaks.iter().enumerate().map(|(index, (start, _))| {
            let position = text.paragraphs.iter().enumerate().find_map(|(paragraph, value)| {
                value.pieces.iter().find(|piece| piece.raw.0 == *start).map(|piece| TextPosition { paragraph, offset: piece.offset })
            });
            PageBreak { index, position }
        }).collect()
    }

    /// Remove content bounded by explicit page breaks or the body start/end.
    /// Boundary breaks are removed too. Breaks inside tables or text boxes are rejected.
    pub fn drop_content_between(&mut self, start: ContentBoundary, end: ContentBoundary) -> Result<(), FillError> {
        let xml = self.read_document();
        let range = body_range(&xml)?;
        let start = boundary(&range, start, Edge::Start)?;
        let end = boundary(&range, end, Edge::End)?;
        if start > end { return Err(FillError::new("Content boundaries are reversed")); }
        if start == end { return Ok(()); }
        let before = open_elements(&xml, start)?;
        let after = open_elements(&xml, end)?;
        if before.iter().chain(&after).any(|element| matches!(element.name, "w:tbl" | "w:txbxContent")) {
            return Err(FillError::new("Page-range boundaries must be in body paragraphs"));
        }
        let common = before.iter().zip(&after).take_while(|(a, b)| a.at == b.at).count();
        let mut changed = xml[..start].to_string();
        for element in before[common..].iter().rev() { changed.push_str(&format!("</{}>", element.name)); }
        for element in &after[common..] {
            changed.push_str(element.raw);
            let properties = match element.name { "w:p" => "w:pPr", "w:r" => "w:rPr", _ => continue };
            let at = element.at + element.raw.len();
            if xml[at..].starts_with(&format!("<{properties}"))
                && let Some(end) = find_element_end(&xml, at, properties) { changed.push_str(&xml[at..end]); }
        }
        changed.push_str(&xml[end..]);
        let changed = prune_marks(&changed);
        check_balanced(&tokenize(&changed).map_err(FillError::new)?).map_err(FillError::new)?;
        self.write_document(crate::structure::repair_structure(&changed).unwrap_or(changed));
        Ok(())
    }

    /// Loop tags in an unfilled document body, including nested loops.
    pub fn loop_tags(&self) -> Result<Vec<TemplateTag>, FillError> {
        let template = Template::parse(&self.read_document(), &FillOptions::default())?;
        let mut result = Vec::new();
        collect_loops(&template.tags(), &mut result);
        Ok(result)
    }
}

fn collect_loops(tags: &[TemplateTag], out: &mut Vec<TemplateTag>) {
    for tag in tags {
        if matches!(tag.kind, TagType::Loop | TagType::InvertedLoop) { out.push(tag.clone()); }
        collect_loops(&tag.children, out);
    }
}

enum Edge { Start, End }

fn boundary(range: &BodyRange, boundary: ContentBoundary, edge: Edge) -> Result<usize, FillError> {
    match boundary {
        ContentBoundary::Start => Ok(range.start), ContentBoundary::End => Ok(range.end),
        ContentBoundary::PageBreak(index) => range.breaks.get(index).map(|(start, end)| match edge { Edge::Start => *start, Edge::End => *end }).ok_or_else(|| FillError::new("Page break no longer exists")),
    }
}

fn body_range(xml: &str) -> Result<BodyRange, FillError> {
    let tokens = tokenize(xml).map_err(FillError::new)?;
    let mut range = BodyRange { start: 0, end: 0, breaks: Vec::new() };
    let mut depth = 0;
    let mut at = 0;
    for token in tokens {
        let length = match token { Token::Text(text) => text.len(), Token::Element { raw, .. } => raw.len() };
        if let Token::Element { kind, name, raw } = token {
            if name == "w:body" && kind == ElementKind::Open { range.start = at + length; }
            if name == "w:body" && kind == ElementKind::Close && range.end == 0 { range.end = at; }
            if name == "w:sectPr" && depth == 2 && range.end == 0 { range.end = at; }
            if name == "w:br" && kind != ElementKind::Close && read_attribute(raw, "w:type") == Some("page") {
                let end = if kind == ElementKind::Open { find_element_end(xml, at, name).unwrap_or(at + length) } else { at + length };
                range.breaks.push((at, end));
            }
            match kind { ElementKind::Open => depth += 1, ElementKind::Close => depth -= 1, _ => {} }
        }
        at += length;
    }
    if range.start == 0 || range.end < range.start { return Err(FillError::new("Document body not found")); }
    Ok(range)
}

fn open_elements(xml: &str, until: usize) -> Result<Vec<OpenElement<'_>>, FillError> {
    let mut stack = Vec::new();
    let mut at = 0;
    for token in tokenize(xml).map_err(FillError::new)? {
        if at >= until { break; }
        let length = match token { Token::Text(text) => text.len(), Token::Element { raw, .. } => raw.len() };
        match token {
            Token::Element { kind: ElementKind::Open, name, raw } => stack.push(OpenElement { at, name, raw }),
            Token::Element { kind: ElementKind::Close, .. } => { stack.pop(); }
            _ => {}
        }
        at += length;
    }
    Ok(stack)
}

fn update_child(parts: &mut Vec<(String, Cow<[u8]>)>, path: &str, root: &str, namespace: &str, child: &str, added: &str) {
    let index = parts.iter().position(|(name, _)| name == path).unwrap_or_else(|| {
        parts.push((path.to_string(), Cow::Owned(format!("<{root} xmlns=\"{namespace}\"></{root}>").into_bytes())));
        parts.len() - 1
    });
    let mut xml = String::from_utf8_lossy(&parts[index].1).into_owned();
    if child.is_empty()
        && let Some(part) = read_attribute(added, "PartName")
        && xml.contains(&format!("PartName=\"{part}\"")) { return; }
    let tokens = tokenize(&xml).unwrap_or_default();
    let Some((name, raw)) = tokens.iter().find_map(|token| match token {
        Token::Element { name, raw, kind: ElementKind::Open | ElementKind::SelfClosing } if name.rsplit(':').next() == Some(root) => Some((name.to_string(), raw.to_string())),
        _ => None,
    }) else { return };
    let Some(at) = xml.find(&raw) else { return };
    let prefix = name.rsplit_once(':').map(|(prefix, _)| prefix);
    let (child, added) = if let Some(prefix) = prefix {
        let tag = if child.is_empty() { "Override" } else { child };
        (format!("{prefix}:{tag}"), added.replace(&format!("<{tag}"), &format!("<{prefix}:{tag}")).replace(&format!("</{tag}>"), &format!("</{prefix}:{tag}>")))
    } else { (child.to_string(), added.to_string()) };
    if raw.ends_with("/>") { xml.replace_range(at..at + raw.len(), &format!("{}></{name}>", raw.trim_end_matches("/>"))); }
    let Some(start) = xml[at..].find('>').map(|end| at + end + 1) else { return };
    let Some(end) = xml.rfind(&format!("</{name}>")) else { return };
    let inner = if read_attribute(&added, "PartName").is_some() { format!("{}{added}", &xml[start..end]) } else { set_child(&xml[start..end], &child, &added, &[]) };
    parts[index].1 = Cow::Owned(format!("{}{inner}{}", &xml[..start], &xml[end..]).into_bytes());
}

fn prune_marks(xml: &str) -> String {
    let Ok(tokens) = tokenize(xml) else { return xml.to_string() };
    let mut counts = BTreeMap::new();
    for token in &tokens {
        let Token::Element { name, raw, .. } = token else { continue };
        let family = match *name { "w:bookmarkStart" | "w:bookmarkEnd" => "bookmark", "w:commentRangeStart" | "w:commentRangeEnd" | "w:commentReference" => "comment", _ => continue };
        if let Some(id) = read_attribute(raw, "w:id") { *counts.entry((family, id)).or_insert(0) += i32::from(*name != "w:commentReference"); }
    }
    let orphaned: BTreeSet<_> = counts.into_iter().filter_map(|(key, count)| (count != 2).then_some(key)).collect();
    let mut out = String::new();
    for token in tokens {
        if let Token::Element { name, raw, .. } = token {
            let family = match name { "w:bookmarkStart" | "w:bookmarkEnd" => "bookmark", "w:commentRangeStart" | "w:commentRangeEnd" | "w:commentReference" => "comment", _ => "" };
            if read_attribute(raw, "w:id").is_some_and(|id| orphaned.contains(&(family, id))) { continue; }
        }
        match token { Token::Text(raw) | Token::Element { raw, .. } => out.push_str(raw) }
    }
    out
}
