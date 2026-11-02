//! Split a template paragraph around block HTML while retaining its surrounding content.

use super::*;

const PROPERTY_ELEMENTS: &[&str] = &["w:pPr", "w:rPr", "w:sdtPr", "w:sdtEndPr"];
const WRAPPERS: &[&str] = &["w:p", "w:r", "w:t", "w:hyperlink", "w:sdt", "w:sdtContent", "w:ins", "w:del", "w:moveFrom", "w:moveTo"];

struct Open<'a> { name: &'a str, raw: &'a str, start: usize, props: Vec<&'a str> }

pub(super) fn has_blocks(paragraph: &str, values: &[RichValue]) -> bool {
    let mut in_text = false;
    for token in tokenize(paragraph).unwrap_or_default() {
        match token {
            Token::Element { kind: ElementKind::Open, name: "w:t", .. } => in_text = true,
            Token::Element { kind: ElementKind::Close, name: "w:t", .. } => in_text = false,
            Token::Text(mut rest) if in_text => {
                while let Some((mark, index)) = find_mark(rest) {
                    if values.get(index).is_some_and(|value| value.is_block && value.block_mode == crate::HtmlBlockMode::Preserve) { return true; }
                    rest = &rest[rest.find(&mark).unwrap() + mark.len()..];
                }
            }
            _ => {}
        }
    }
    false
}

pub(super) fn write(paragraph: &str, values: &[RichValue], links: &mut Vec<String>, mut continued: Option<(&str, &mut u32)>, mut numbering: Option<&mut Numberings>) -> String {
    let Ok(tokens) = tokenize(paragraph) else { return paragraph.to_string() };
    let mut stack: Vec<Open<'_>> = Vec::new();
    let mut segment = String::new();
    let mut out = String::new();
    let mut at = 0;
    for token in tokens {
        match token {
            Token::Text(raw) if stack.last().is_some_and(|open| open.name == "w:t") => {
                let mut rest = raw;
                while let Some((mark, index)) = find_mark(rest) {
                    let offset = rest.find(&mark).unwrap();
                    segment.push_str(&rest[..offset]);
                    rest = &rest[offset + mark.len()..];
                    let Some(value) = values.get(index).filter(|value| value.is_block && value.block_mode == crate::HtmlBlockMode::Preserve) else {
                        segment.push_str(&mark); continue;
                    };
                    let blocks = read_html(&value.html, value.has_extras, SpaceEdges::default(), &value.style_sheet, &value.raw);
                    if blocks.is_empty() { continue; }
                    for open in stack.iter().rev() { segment.push_str(&format!("</{}>", open.name)); }
                    let props = stack.first().map(|open| open.props.concat()).unwrap_or_default();
                    let layout = without_section(&props);
                    let before = segment.replacen(&props, &layout, usize::from(!props.is_empty()));
                    let rendered = write_paragraph(&before, values, links, continued.as_mut().map(|(prefix, next)| (*prefix, &mut **next)), numbering.as_deref_mut());
                    if has_content(&rendered) { out.push_str(&rendered); }
                    let opening = stack.first().map_or("<w:p>", |open| open.raw);
                    let shell = format!("{opening}{layout}</w:p>");
                    let run_props = stack.iter().rev().find(|open| open.name == "w:r").map(|open| open.props.concat()).unwrap_or_default();
                    let inserted = write_blocks(&shell, &blocks, &run_props, links, None, numbering.as_deref_mut());
                    if out.ends_with("</w:tbl>") && inserted.starts_with("<w:tbl>") { out.push_str("<w:p/>"); }
                    out.push_str(&inserted);
                    segment.clear();
                    for open in &stack { segment.push_str(open.raw); for props in &open.props { segment.push_str(props); } }
                }
                segment.push_str(rest);
                at += raw.len();
            }
            Token::Text(raw) => { segment.push_str(raw); at += raw.len(); }
            Token::Element { kind, name, raw } => {
                segment.push_str(raw);
                match kind {
                    ElementKind::Open => stack.push(Open { name, raw, start: at, props: Vec::new() }),
                    ElementKind::Close => {
                        if let Some(closed) = stack.pop()
                            && PROPERTY_ELEMENTS.contains(&name)
                            && let Some(parent) = stack.last_mut() {
                            parent.props.push(&paragraph[closed.start..at + raw.len()]);
                        }
                    }
                    ElementKind::SelfClosing if PROPERTY_ELEMENTS.contains(&name) => {
                        if let Some(parent) = stack.last_mut() { parent.props.push(raw); }
                    }
                    _ => {}
                }
                at += raw.len();
            }
        }
    }
    let rendered = write_paragraph(&segment, values, links, continued, numbering);
    if has_content(&rendered) { out.push_str(&rendered); }
    out
}

fn without_section(props: &str) -> String {
    let Some(start) = find_element_start(props, "<w:sectPr") else { return props.to_string() };
    let Some(end) = crate::properties::find_element_end(props, start, "w:sectPr") else { return props.to_string() };
    format!("{}{}", &props[..start], &props[end..])
}

fn has_content(xml: &str) -> bool {
    if find_element_start(xml, "<w:sectPr").is_some() { return true; }
    let mut props = 0usize;
    for token in tokenize(xml).unwrap_or_default() {
        match token {
            Token::Element { kind: ElementKind::Close, .. } if props > 0 => props -= 1,
            Token::Element { kind: ElementKind::Open, .. } if props > 0 => props += 1,
            _ if props > 0 => {}
            Token::Element { kind: ElementKind::Open, name, .. } if PROPERTY_ELEMENTS.contains(&name) => props = 1,
            Token::Element { kind: ElementKind::SelfClosing, name, .. } if PROPERTY_ELEMENTS.contains(&name) => {}
            Token::Text(raw) if !raw.trim().is_empty() => return true,
            Token::Element { kind: ElementKind::Open | ElementKind::SelfClosing | ElementKind::Other, name, .. } if !WRAPPERS.contains(&name) => return true,
            _ => {}
        }
    }
    false
}
