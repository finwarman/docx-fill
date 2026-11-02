use std::collections::BTreeMap;

use crate::xml::{ElementKind, Token, escape, tokenize, unescape};

const EXTENDED: &str = "http://schemas.microsoft.com/office/drawing/2014/chartex";
const CHART: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";
const DRAWING: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const RELATIONSHIPS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const PACKAGE: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const CONTENT_TYPES: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
const COMPATIBILITY: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";

type Bindings = BTreeMap<String, String>;

struct Attribute<'a> {
    name: &'a str,
    value: &'a str,
    name_start: usize,
    value_start: usize,
    value_end: usize,
}

struct Frame {
    bindings: Bindings,
    active: bool,
    alternative: Option<bool>,
}

struct NormalFrame {
    bindings: Bindings,
    output: Bindings,
    original_name: String,
    name: String,
}

pub(super) struct Relation {
    pub id: String,
    pub kind: String,
    pub target: String,
    pub external: bool,
}

/// Canonicalize only ChartEx parts and chart-reference names. Other namespaces keep their URIs.
pub(super) fn normalize(xml: &str) -> Option<String> {
    let tokens = tokenize(xml).ok()?;
    let mut frames: Vec<NormalFrame> = Vec::new();
    let mut out = String::with_capacity(xml.len());
    let mut extended_part = false;
    let mut seen_root = false;
    let mut next_prefix = 0;
    for token in tokens {
        let Token::Element { kind, name, raw } = token else {
            if let Token::Text(text) = token { out.push_str(text); }
            continue;
        };
        if kind == ElementKind::Other { out.push_str(raw); continue; }
        if kind == ElementKind::Close {
            let frame = frames.pop()?;
            if frame.original_name != name { return None; }
            out.push_str(&replace_name(raw, name, &frame.name, 2));
            continue;
        }
        let attrs = attributes(raw, name)?;
        let bindings = read_bindings(frames.last().map(|frame| &frame.bindings), &attrs);
        let mut output = read_bindings(frames.last().map(|frame| &frame.output), &attrs);
        let uri = element_uri(name, &bindings);
        if !seen_root { extended_part = uri == Some(EXTENDED) && local(name) == "chartSpace"; seen_root = true; }
        let reference = local(name) == "chart" && matches!(uri, Some(EXTENDED | CHART)) && relationship_id(&attrs, &bindings).is_some();
        let mut declarations = Bindings::new();
        let mut locked = Bindings::new();
        let renamed = if extended_part && uri == Some(EXTENDED) { format!("cx:{}", local(name)) }
            else if extended_part && uri == Some(DRAWING) { format!("a:{}", local(name)) }
            else if reference { format!("{}:chart", if uri == Some(EXTENDED) { "cx" } else { "c" }) }
            else if extended_part && foreign_reserved(name, uri) { fresh_name(local(name), uri?, &output, &mut next_prefix) }
            else { name.to_string() };
        bind_name(&renamed, uri, &mut output, &mut declarations, &mut locked);
        let mut edits = Vec::new();
        if renamed != name { edits.push((1, 1 + name.len(), renamed.clone())); }
        for attr in &attrs {
            if attr.name == "xmlns" || attr.name.starts_with("xmlns:") { continue; }
            let uri = attribute_uri(attr.name, &bindings);
            let mut renamed = if extended_part && uri == Some(EXTENDED) { format!("cx:{}", local(attr.name)) }
                else if (extended_part || reference) && uri == Some(RELATIONSHIPS) { format!("r:{}", local(attr.name)) }
                else { attr.name.to_string() };
            let conflict = prefix(&renamed).is_some_and(|prefix| locked.get(prefix).is_some_and(|locked| Some(locked.as_str()) != uri));
            if conflict || (extended_part && foreign_reserved(&renamed, uri)) {
                renamed = fresh_name(local(attr.name), uri?, &output, &mut next_prefix);
            }
            if prefix(&renamed).is_some() { bind_name(&renamed, uri, &mut output, &mut declarations, &mut locked); }
            if renamed != attr.name { edits.push((attr.name_start, attr.name_start + attr.name.len(), renamed)); }
        }
        for attr in &attrs {
            let uri = attribute_uri(attr.name, &bindings);
            let prefix_list = uri == Some(COMPATIBILITY) && matches!(local(attr.name), "Ignorable" | "MustUnderstand" | "ProcessContent" | "PreserveAttributes" | "PreserveElements")
                || attr.name == "Requires" && element_uri(name, &bindings) == Some(COMPATIBILITY);
            if prefix_list {
                let value = preserve_qnames(attr.value, &bindings, &mut output, &mut declarations, &mut locked, &mut next_prefix);
                if value != attr.value { edits.push((attr.value_start, attr.value_end, value)); }
            }
        }
        let mut added = String::new();
        for (prefix, uri) in declarations {
            let name = if prefix.is_empty() { "xmlns".to_string() } else { format!("xmlns:{prefix}") };
            let mut escaped = String::new(); escape(&uri, &mut escaped);
            if let Some(attr) = attrs.iter().find(|attr| attr.name == name) { edits.push((attr.value_start, attr.value_end, escaped)); }
            else { added.push_str(&format!(" {name}=\"{escaped}\"")); }
        }
        let at = raw.len() - if kind == ElementKind::SelfClosing { 2 } else { 1 };
        if !added.is_empty() { edits.push((at, at, added)); }
        edits.sort_by_key(|edit| edit.0);
        let mut changed = raw.to_string();
        for (start, end, value) in edits.into_iter().rev() { changed.replace_range(start..end, &value); }
        out.push_str(&changed);
        if kind == ElementKind::Open { frames.push(NormalFrame { bindings, output, original_name: name.to_string(), name: renamed }); }
    }
    (out != xml).then_some(out)
}

/// Follow the first supported AlternateContent choice and retain fallback XML untouched.
pub(super) fn references(xml: &str) -> (Vec<usize>, bool) {
    let mut frames: Vec<Frame> = Vec::new();
    let mut positions = Vec::new();
    let mut offset = 0;
    for token in tokenize(xml).unwrap_or_default() {
        let length = match token { Token::Element { raw, .. } => raw.len(), Token::Text(text) => text.len() };
        if let Token::Element { kind, name, raw } = token {
            if kind == ElementKind::Close { frames.pop(); }
            else if matches!(kind, ElementKind::Open | ElementKind::SelfClosing) {
                let Some(attrs) = attributes(raw, name) else { offset += length; continue };
                let bindings = read_bindings(frames.last().map(|frame| &frame.bindings), &attrs);
                let uri = element_uri(name, &bindings);
                let mut active = frames.last().is_none_or(|frame| frame.active);
                if uri == Some(COMPATIBILITY) && matches!(local(name), "Choice" | "Fallback")
                    && let Some(chosen) = frames.last_mut().and_then(|frame| frame.alternative.as_mut()) {
                    let supported = local(name) == "Fallback" || attrs.iter().find(|attr| attr.name == "Requires").is_some_and(|attr| {
                        let decoded = unescape(attr.value);
                        let required = decoded.split_whitespace().collect::<Vec<_>>();
                        !required.is_empty() && required.iter().all(|prefix| bindings.get(*prefix).is_some_and(|uri| matches!(uri.as_str(), EXTENDED | CHART | DRAWING)))
                    });
                    let selected = !*chosen && supported;
                    *chosen |= selected;
                    active &= selected;
                }
                if active && local(name) == "chart" && matches!(uri, Some(EXTENDED | CHART)) && relationship_id(&attrs, &bindings).is_some() { positions.push(offset); }
                let alternative = (uri == Some(COMPATIBILITY) && local(name) == "AlternateContent").then_some(false);
                if kind == ElementKind::Open { frames.push(Frame { bindings, active, alternative }); }
            }
        }
        offset += length;
    }
    (positions, frames.last().is_none_or(|frame| frame.active))
}

pub(super) fn relations(xml: &str) -> Vec<Relation> {
    let mut frames: Vec<Bindings> = Vec::new();
    let mut result = Vec::new();
    for token in tokenize(xml).unwrap_or_default() {
        let Token::Element { kind, name, raw } = token else { continue };
        if kind == ElementKind::Close { frames.pop(); continue; }
        if !matches!(kind, ElementKind::Open | ElementKind::SelfClosing) { continue; }
        let Some(attrs) = attributes(raw, name) else { continue };
        let bindings = read_bindings(frames.last(), &attrs);
        if local(name) == "Relationship" && (element_uri(name, &bindings) == Some(PACKAGE) || name == "Relationship" && element_uri(name, &bindings).is_none()) {
            let attr = |name| attrs.iter().find(|attr| attr.name == name).map(|attr| unescape(attr.value));
            if let (Some(id), Some(kind), Some(target)) = (attr("Id"), attr("Type"), attr("Target")) { result.push(Relation { id, kind, target, external: attr("TargetMode").as_deref() == Some("External") }); }
        }
        if kind == ElementKind::Open { frames.push(bindings); }
    }
    result
}

pub(super) fn append_relation(xml: &str, relationship: &str) -> Option<String> {
    append_child(xml, PackageRoot::Relationships, relationship)
}

pub(super) fn append_type(xml: &str, content_type: &str) -> Option<String> {
    append_child(xml, PackageRoot::Types, content_type)
}

enum PackageRoot { Relationships, Types }

fn append_child(xml: &str, root: PackageRoot, child: &str) -> Option<String> {
    let (root_name, child_name, namespace) = match root {
        PackageRoot::Relationships => ("Relationships", "Relationship", PACKAGE),
        PackageRoot::Types => ("Types", "Override", CONTENT_TYPES),
    };
    for token in tokenize(xml).ok()? {
        let Token::Element { kind: ElementKind::Open | ElementKind::SelfClosing, name, raw } = token else { continue };
        let attrs = attributes(raw, name)?;
        let bindings = read_bindings(None, &attrs);
        if local(name) != root_name || element_uri(name, &bindings).is_some_and(|uri| uri != namespace) { return None; }
        let child = child.replace(&format!("<{child_name} "), &format!("<{child_name} xmlns=\"{namespace}\" "));
        if let Some(open) = raw.strip_suffix("/>") {
            let start = xml.find(raw)?;
            return Some(format!("{}{open}>{child}</{name}>{}", &xml[..start], &xml[start + raw.len()..]));
        }
        let close = format!("</{name}>");
        let at = xml.rfind(&close)?;
        return Some(format!("{}{child}{}", &xml[..at], &xml[at..]));
    }
    None
}

fn attributes<'a>(raw: &'a str, name: &str) -> Option<Vec<Attribute<'a>>> {
    let mut at = 1 + name.len();
    let bytes = raw.as_bytes();
    let mut out = Vec::new();
    while at < bytes.len() {
        while at < bytes.len() && bytes[at].is_ascii_whitespace() { at += 1; }
        if at == bytes.len() || matches!(bytes[at], b'/' | b'>') { break; }
        let name_start = at;
        while at < bytes.len() && !bytes[at].is_ascii_whitespace() && !matches!(bytes[at], b'=' | b'/' | b'>') { at += 1; }
        let name_end = at;
        while at < bytes.len() && bytes[at].is_ascii_whitespace() { at += 1; }
        if bytes.get(at) != Some(&b'=') { return None; }
        at += 1;
        while at < bytes.len() && bytes[at].is_ascii_whitespace() { at += 1; }
        let quote = *bytes.get(at)?;
        if !matches!(quote, b'\'' | b'"') { return None; }
        at += 1;
        let value_start = at;
        while at < bytes.len() && bytes[at] != quote { at += 1; }
        if at == bytes.len() { return None; }
        out.push(Attribute { name: &raw[name_start..name_end], value: &raw[value_start..at], name_start, value_start, value_end: at });
        at += 1;
    }
    Some(out)
}

fn read_bindings(parent: Option<&Bindings>, attrs: &[Attribute]) -> Bindings {
    let mut out = parent.cloned().unwrap_or_default();
    for attr in attrs {
        if attr.name == "xmlns" { out.insert(String::new(), unescape(attr.value)); }
        else if let Some(prefix) = attr.name.strip_prefix("xmlns:") { out.insert(prefix.to_string(), unescape(attr.value)); }
    }
    out
}

fn relationship_id<'a>(attrs: &'a [Attribute], bindings: &Bindings) -> Option<&'a str> {
    attrs.iter().find(|attr| local(attr.name) == "id" && attribute_uri(attr.name, bindings) == Some(RELATIONSHIPS)).map(|attr| attr.value)
}

fn element_uri<'a>(name: &str, bindings: &'a Bindings) -> Option<&'a str> { bindings.get(prefix(name).unwrap_or("")).map(String::as_str).filter(|uri| !uri.is_empty()) }
fn attribute_uri<'a>(name: &str, bindings: &'a Bindings) -> Option<&'a str> { bindings.get(prefix(name)?).map(String::as_str).filter(|uri| !uri.is_empty()) }
fn prefix(name: &str) -> Option<&str> { name.split_once(':').map(|(prefix, _)| prefix) }
fn local(name: &str) -> &str { name.rsplit(':').next().unwrap_or(name) }

fn foreign_reserved(name: &str, uri: Option<&str>) -> bool {
    match prefix(name) { Some("cx") => uri != Some(EXTENDED), Some("a") => uri != Some(DRAWING), Some("r") => uri != Some(RELATIONSHIPS), _ => false }
}

fn fresh_name(local: &str, uri: &str, output: &Bindings, next: &mut usize) -> String {
    loop {
        let prefix = format!("docxFillNs{next}");
        *next += 1;
        if output.get(&prefix).is_none_or(|bound| bound == uri) { return format!("{prefix}:{local}"); }
    }
}

fn bind_name(name: &str, uri: Option<&str>, output: &mut Bindings, declarations: &mut Bindings, locked: &mut Bindings) {
    let prefix = prefix(name).unwrap_or("");
    let uri = uri.unwrap_or("");
    if output.get(prefix).map(String::as_str).unwrap_or("") != uri {
        output.insert(prefix.to_string(), uri.to_string());
        declarations.insert(prefix.to_string(), uri.to_string());
    }
    locked.insert(prefix.to_string(), uri.to_string());
}

fn preserve_qnames(value: &str, original: &Bindings, output: &mut Bindings, declarations: &mut Bindings, locked: &mut Bindings, next: &mut usize) -> String {
    let mut changed = false;
    let decoded = unescape(value);
    let words = decoded.split_whitespace().map(|word| {
        let (old, suffix) = word.split_once(':').map_or((word, ""), |(prefix, _)| (prefix, &word[prefix.len()..]));
        let Some(uri) = original.get(old) else { return word.to_string() };
        if output.get(old) == Some(uri) { return word.to_string(); }
        let name = fresh_name("name", uri, output, next);
        let prefix = prefix(&name).expect("fresh names are prefixed");
        bind_name(&name, Some(uri), output, declarations, locked);
        changed = true;
        format!("{prefix}{suffix}")
    }).collect::<Vec<_>>();
    if !changed { return value.to_string(); }
    let mut escaped = String::new();
    escape(&words.join(" "), &mut escaped);
    escaped
}

fn replace_name(raw: &str, old: &str, new: &str, start: usize) -> String {
    if old == new { return raw.to_string(); }
    format!("{}{new}{}", &raw[..start], &raw[start + old.len()..])
}
