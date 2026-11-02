use crate::properties::{find_element_end, find_starts, set_attribute};
use crate::xml::{ElementKind, Token, read_attribute, tokenize};

const LINE_GROUP: &str = "data-docx-fill-line";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SoftBreakScope { #[default] None, Shapes, All }

#[derive(Debug, Clone)]
pub struct ParagraphPlaceholderOptions {
    pub expand_to_shape: bool,
    pub soft_breaks: SoftBreakScope,
}

impl Default for ParagraphPlaceholderOptions {
    fn default() -> Self { Self { expand_to_shape: true, soft_breaks: SoftBreakScope::None } }
}

pub(crate) fn is_line(raw: &str) -> bool { read_attribute(raw, LINE_GROUP).is_some() }

/// Expose soft lines as temporary paragraphs so the normal placeholder rules apply to them.
pub(crate) fn expand(xml: &str, scope: SoftBreakScope) -> String {
    if scope == SoftBreakScope::None { return xml.to_string(); }
    let Ok(tokens) = tokenize(xml) else { return xml.to_string() };
    let mut shapes = 0usize;
    let mut paragraphs: Vec<(usize, bool, bool)> = Vec::new();
    let mut edits = Vec::new();
    let mut at = 0;
    for token in tokens {
        let length = match token { Token::Text(raw) | Token::Element { raw, .. } => raw.len() };
        match token {
            Token::Element { name: "w:txbxContent", kind: ElementKind::Open, .. } => shapes += 1,
            Token::Element { name: "w:txbxContent", kind: ElementKind::Close, .. } => shapes = shapes.saturating_sub(1),
            Token::Element { name: "w:p", kind: ElementKind::Open, .. } => {
                if let Some(parent) = paragraphs.last_mut() { parent.2 = true; }
                paragraphs.push((at, scope == SoftBreakScope::All || shapes > 0, false));
            }
            Token::Element { name: "w:p", kind: ElementKind::Close, .. } => {
                if let Some((start, true, false)) = paragraphs.pop()
                    && let Some(replacement) = split_lines(&xml[start..at + length], start) {
                    edits.push((start, at + length, replacement));
                }
            }
            _ => {}
        }
        at += length;
    }
    let mut out = String::new();
    let mut at = 0;
    for (start, end, replacement) in edits {
        out.push_str(&xml[at..start]); out.push_str(&replacement); at = end;
    }
    out.push_str(&xml[at..]);
    out
}

fn split_lines(paragraph: &str, group: usize) -> Option<String> {
    let tokens = tokenize(paragraph).ok()?;
    let opening_end = paragraph.find('>')? + 1;
    let opening = set_attribute(&paragraph[..opening_end], LINE_GROUP, &group.to_string());
    let properties = leading_props(paragraph, opening_end, "w:pPr");
    let mut out = opening.clone();
    let mut stack: Vec<(&str, &str, usize)> = Vec::new();
    let mut from = opening_end;
    let mut at = 0;
    let mut split = false;
    for token in tokens {
        let length = match token { Token::Text(raw) | Token::Element { raw, .. } => raw.len() };
        if at < from && at != 0 { at += length; continue; }
        if let Token::Element { name: "w:br", kind: ElementKind::Open | ElementKind::SelfClosing, raw } = token
            && read_attribute(raw, "w:type").is_none_or(|kind| kind == "textWrapping") {
            out.push_str(&paragraph[from..at]);
            for (name, _, _) in stack.iter().skip(1).rev() { out.push_str(&format!("</{name}>")); }
            out.push_str("</w:p>"); out.push_str(&opening); out.push_str(properties);
            for (name, raw, end) in stack.iter().skip(1) {
                out.push_str(raw);
                if *name == "w:r" { out.push_str(leading_props(paragraph, *end, "w:rPr")); }
            }
            from = if matches!(token, Token::Element { kind: ElementKind::Open, .. }) { find_element_end(paragraph, at, "w:br")? } else { at + length };
            split = true;
        } else if let Token::Element { kind, name, raw } = token {
            match kind { ElementKind::Open => stack.push((name, raw, at + length)), ElementKind::Close => { stack.pop(); }, _ => {} }
        }
        at += length;
    }
    if !split { return None; }
    out.push_str(&paragraph[from..]);
    Some(out)
}

fn leading_props<'a>(xml: &'a str, at: usize, name: &str) -> &'a str {
    let trimmed = xml[at..].trim_start();
    if !trimmed.starts_with(&format!("<{name}")) { return ""; }
    let start = xml.len() - trimmed.len();
    find_element_end(xml, start, name).map_or("", |end| &xml[start..end])
}

/// Rejoin surviving lines without changing the original paragraph's soft-break layout.
pub(crate) fn collapse(xml: &str) -> String {
    if !xml.contains(LINE_GROUP) { return xml.to_string(); }
    let starts = find_starts(xml, "w:p");
    let mut out = String::new();
    let mut from = 0;
    let mut index = 0;
    while index < starts.len() {
        let start = starts[index]; index += 1;
        if start < from { continue; }
        let Some(open_end) = xml[start..].find('>').map(|end| start + end + 1) else { break };
        let Some(group) = read_attribute(&xml[start..open_end], LINE_GROUP) else { continue };
        let Some(mut end) = paragraph_end(xml, open_end) else { continue };
        let opening = xml[start..open_end].replace(&format!(" {LINE_GROUP}=\"{group}\""), "");
        out.push_str(&xml[from..start]); out.push_str(&opening); out.push_str(&xml[open_end..end - "</w:p>".len()]);
        while let Some(&next) = starts.get(index) {
            if !xml[end..next].trim().is_empty() { break; }
            let Some(next_open) = xml[next..].find('>').map(|at| next + at + 1) else { break };
            if read_attribute(&xml[next..next_open], LINE_GROUP) != Some(group) { break; }
            let Some(next_end) = paragraph_end(xml, next_open) else { break };
            let properties = leading_props(xml, next_open, "w:pPr");
            let body_start = if properties.is_empty() { next_open } else { xml[next_open..].find(properties).map_or(next_open, |offset| next_open + offset + properties.len()) };
            out.push_str("<w:r><w:br/></w:r>"); out.push_str(&xml[body_start..next_end - "</w:p>".len()]);
            end = next_end; index += 1;
        }
        out.push_str("</w:p>"); from = end;
    }
    out.push_str(&xml[from..]);
    out
}

fn paragraph_end(xml: &str, from: usize) -> Option<usize> {
    xml[from..].find("</w:p>").map(|end| from + end + "</w:p>".len())
}
