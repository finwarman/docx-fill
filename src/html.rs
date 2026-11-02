//! Configuration and callbacks for HTML values.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HtmlBlockMode {
    Block,
    #[default]
    Inline,
    Error,
    /// Keep surrounding template text in sibling paragraphs when HTML inserts blocks.
    Preserve,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HtmlTagKind { Inline, Block }

/// Legacy render methods remain best-effort; checked renders enforce these flags.
#[derive(Debug, Clone, Default)]
pub struct HtmlOptions {
    pub ignore_unknown_tags: bool,
    pub ignore_css_errors: bool,
    pub on_invalid_block: HtmlBlockMode,
    pub custom_tags: BTreeMap<String, HtmlTagKind>,
}

pub struct HtmlElement<'a> {
    name: &'a str,
    attributes: &'a [(String, String)],
    body: &'a str,
    matches: &'a dyn Fn(&str) -> bool,
}

impl<'a> HtmlElement<'a> {
    pub(crate) fn new(name: &'a str, attributes: &'a [(String, String)], body: &'a str, matches: &'a dyn Fn(&str) -> bool) -> Self { Self { name, attributes, body, matches } }
    pub fn name(&self) -> &str { self.name }
    pub fn attribute(&self, name: &str) -> Option<&str> { self.attributes.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str()) }
    pub fn inner_html(&self) -> &str { crate::rich_text::element_inner(self.name, self.body).0 }
    pub fn text(&self) -> String { crate::rich_text::plain_html(self.inner_html()) }
    pub fn matches(&self, selector: &str) -> bool { (self.matches)(selector) }
}

#[derive(Debug, Clone, Default)]
pub struct HtmlCustomization { pub(crate) css: String, pub(crate) paragraph_style: Option<String> }

impl HtmlCustomization {
    pub fn css(mut self, css: impl Into<String>) -> Self { self.css = css.into(); self }
    pub fn paragraph_style(mut self, style: impl Into<String>) -> Self { self.paragraph_style = Some(style.into()); self }
}

#[derive(Debug, Clone)]
pub struct HtmlFragment { pub(crate) kind: HtmlTagKind, pub(crate) xml: String }

impl HtmlFragment {
    pub fn inline(xml: impl Into<String>) -> Self { Self { kind: HtmlTagKind::Inline, xml: xml.into() } }
    pub fn block(xml: impl Into<String>) -> Self { Self { kind: HtmlTagKind::Block, xml: xml.into() } }
}
