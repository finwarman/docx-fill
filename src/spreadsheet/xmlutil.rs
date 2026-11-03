//! Small lossless helpers for SpreadsheetML package parts.

use crate::xml::{ElementKind, Token, check_balanced, escape, tokenize};
use crate::FillError;

#[derive(Debug, Clone)]
pub(super) struct Element {
    pub name: String,
    pub start: usize,
    pub open_end: usize,
    pub close_start: usize,
    pub end: usize,
}

pub(super) fn local_name(name: &str) -> &str { name.rsplit(':').next().unwrap_or(name) }
pub(super) fn prefix(name: &str) -> &str { name.rfind(':').map_or("", |at| &name[..=at]) }
pub(super) fn escaped(text: &str) -> String { let mut out = String::new(); escape(text, &mut out); out }

pub(super) fn elements(xml: &str) -> Result<Vec<Element>, FillError> {
    let tokens = tokenize(xml).map_err(super::error)?;
    check_balanced(&tokens).map_err(super::error)?;
    let mut elements: Vec<Element> = Vec::new();
    let mut stack = Vec::new();
    let mut at = 0;
    for token in tokens {
        let len = match token { Token::Text(text) => text.len(), Token::Element { raw, .. } => raw.len() };
        if let Token::Element { kind, name, .. } = token {
            match kind {
                ElementKind::Open => {
                    stack.push(elements.len());
                    elements.push(Element { name: name.into(), start: at, open_end: at + len, close_start: at + len, end: at + len });
                }
                ElementKind::SelfClosing => elements.push(Element { name: name.into(), start: at, open_end: at + len, close_start: at + len, end: at + len }),
                ElementKind::Close => {
                    let index = stack.pop().ok_or_else(|| super::error("Unmatched spreadsheet XML closing element"))?;
                    elements[index].close_start = at;
                    elements[index].end = at + len;
                }
                ElementKind::Other => {}
            }
        }
        at += len;
    }
    Ok(elements)
}
