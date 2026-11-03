//! Split a template paragraph around block HTML while retaining its surrounding content.

use super::*;

const PROPERTY_ELEMENTS: &[&str] = &["w:pPr", "w:rPr", "w:sdtPr", "w:sdtEndPr"];
const WRAPPERS: &[&str] = &["w:p", "w:r", "w:t", "w:hyperlink", "w:sdt", "w:sdtContent", "w:ins", "w:del", "w:moveFrom", "w:moveTo"];
// Layout hints Word rewrites on save; they never keep a paragraph.
const HINTS: &[&str] = &["w:proofErr", "w:lastRenderedPageBreak"];
// Zero-width ranges; valid between paragraphs, so they outlive an empty segment.
const RANGES: &[&str] = &[
    "w:bookmarkStart", "w:bookmarkEnd", "w:commentRangeStart", "w:commentRangeEnd", "w:permStart", "w:permEnd",
    "w:moveFromRangeStart", "w:moveFromRangeEnd", "w:moveToRangeStart", "w:moveToRangeEnd",
];
// Reopening a simple field after a split would repeat its instruction.
const SINGLE_USE: &[&str] = &["w:fldSimple"];

struct Open<'a> { name: &'a str, raw: &'a str, start: usize, props: Vec<&'a str>, is_dropped: bool }

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
    let mut is_numbered = false;
    let mut split_props = String::new();
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
                    let layout = paragraph_layout(&without(&props, "w:sectPr"), is_numbered);
                    split_props.clone_from(&props);
                    let before = segment.replacen(&props, &layout, usize::from(!props.is_empty()));
                    let rendered = write_paragraph(&before, values, links, continued.as_mut().map(|(prefix, next)| (*prefix, &mut **next)), numbering.as_deref_mut());
                    if has_content(&rendered) {
                        out.push_str(&rendered);
                        is_numbered = true;
                    } else {
                        out.push_str(&ranges(&rendered));
                    }
                    let layout = paragraph_layout(&layout, is_numbered);
                    let opening = stack.first().map_or("<w:p>", |open| open.raw);
                    let shell = format!("{opening}{layout}</w:p>");
                    let run_props = stack.iter().rev().find(|open| open.name == "w:r").map(|open| open.props.concat()).unwrap_or_default();
                    let inserted = write_blocks(&shell, &blocks, &run_props, links, None, numbering.as_deref_mut());
                    if out.ends_with("</w:tbl>") && inserted.starts_with("<w:tbl>") { out.push_str("<w:p/>"); }
                    out.push_str(&inserted);
                    is_numbered = true;
                    segment.clear();
                    for open in &mut stack {
                        if SINGLE_USE.contains(&open.name) { open.is_dropped = true; continue; }
                        segment.push_str(open.raw);
                        for props in &open.props { segment.push_str(props); }
                    }
                }
                segment.push_str(rest);
                at += raw.len();
            }
            Token::Text(raw) => { segment.push_str(raw); at += raw.len(); }
            Token::Element { kind, name, raw } => {
                match kind {
                    ElementKind::Open => stack.push(Open { name, raw, start: at, props: Vec::new(), is_dropped: false }),
                    ElementKind::Close => {
                        let closed = stack.pop();
                        if closed.as_ref().is_some_and(|open| open.is_dropped) { at += raw.len(); continue; }
                        if let Some(closed) = closed
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
                segment.push_str(raw);
                at += raw.len();
            }
        }
    }
    let segment = segment.replacen(&split_props, &paragraph_layout(&split_props, is_numbered), usize::from(!split_props.is_empty()));
    let rendered = write_paragraph(&segment, values, links, continued, numbering);
    if has_content(&rendered) { out.push_str(&rendered); } else { out.push_str(&ranges(&rendered)); }
    out
}

/// Keeps the paragraph's list number on the first paragraph written only.
fn paragraph_layout(props: &str, is_numbered: bool) -> String {
    if is_numbered { without(props, "w:numPr") } else { props.to_string() }
}

pub(super) fn without(props: &str, name: &str) -> String {
    let Some(start) = find_element_start(props, &format!("<{name}")) else { return props.to_string() };
    let Some(end) = crate::properties::find_element_end(props, start, name) else { return props.to_string() };
    format!("{}{}", &props[..start], &props[end..])
}

fn ranges(xml: &str) -> String {
    tokenize(xml).unwrap_or_default().into_iter()
        .filter_map(|token| match token { Token::Element { name, raw, .. } if RANGES.contains(&name) => Some(raw), _ => None })
        .collect()
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
            Token::Element { name, .. } if HINTS.contains(&name) || RANGES.contains(&name) => {}
            Token::Element { kind: ElementKind::Open | ElementKind::SelfClosing | ElementKind::Other, name, .. } if !WRAPPERS.contains(&name) => return true,
            _ => {}
        }
    }
    false
}
