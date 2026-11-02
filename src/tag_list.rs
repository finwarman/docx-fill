//! Lists a parsed template's tags, with loops holding the tags inside them.

use crate::template::{Tag, TagKind, Template};

/// What a listed tag does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagType {
    /// `{name}`
    Value,
    /// `{@name}`
    Raw,
    /// `{#name}` or `{-w:tr name}`
    Loop,
    /// `{^name}`
    InvertedLoop,
    /// A tag starting with one of `FillOptions::module_prefixes`.
    Module,
    /// `{$name}`
    Chart,
    /// `{~~name}` or `{~name}`
    RichText,
    /// `{%name}` or `{%%name}`
    Image,
    /// `{:stylecell}`, `{:stylepar}` and the other styling tags.
    Style,
    /// `{:merge-cells-col}` or `{:merge-cells-row}`
    TableMerge,
    /// `{r@name}`
    RawRun,
    /// `{?name}`
    Placeholder,
    /// `{:footnote name}`
    Footnote,
    /// `{:table name}`
    Table,
    /// `{:#grid name}`
    Grid,
    /// `{:include name}`
    Include,
    /// `{:segment name}`
    Segment,
    /// `{:includesegment name}`
    IncludeSegment,
    /// `{:subsection name}`, `{:replacesection name}` or `{:page name}`
    Section,
}

/// A tag in a template, as [`Template::tags`] lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct TemplateTag {
    /// The name the tag reads, without its prefix. Empty for a tag that reads none, such as a merge.
    pub name: String,
    pub kind: TagType,
    /// The tag as written, without delimiters.
    pub written: String,
    /// For a loop, grid or segment, the tags up to its closing tag.
    pub children: Vec<TemplateTag>,
}

/// Each tag of `template` in document order, without closing tags.
pub(crate) fn list_tags(template: &Template) -> Vec<TemplateTag> {
    let mut levels: Vec<Vec<TemplateTag>> = vec![Vec::new()];
    for tag in &template.tags {
        if tag.kind == TagKind::Close {
            let children = levels.pop().unwrap_or_default();
            let Some(opening) = levels.last_mut().and_then(|level| level.last_mut()) else { continue };
            opening.children = children;
            continue;
        }

        let Some(kind) = tag_type(&tag.kind) else { continue };
        levels.last_mut().expect("the top level stays").push(TemplateTag { name: tag.name.clone(), kind, written: tag.text.clone(), children: Vec::new() });
        if is_opening(tag) {
            levels.push(Vec::new());
        }
    }
    levels.swap_remove(0)
}

fn is_opening(tag: &Tag) -> bool {
    matches!(tag.kind, TagKind::Open { .. } | TagKind::GridOpen | TagKind::SegmentOpen { .. })
}

/// The listed type of a tag, or `None` for one that only ends a range.
fn tag_type(kind: &TagKind) -> Option<TagType> {
    let kind = match kind {
        TagKind::Value => TagType::Value,
        TagKind::Raw => TagType::Raw,
        TagKind::Open { is_inverted: true, .. } => TagType::InvertedLoop,
        TagKind::Open { is_inverted: false, .. } => TagType::Loop,
        TagKind::Module(_) => TagType::Module,
        TagKind::Chart => TagType::Chart,
        TagKind::CellStyle | TagKind::Style(_) | TagKind::GlobalStyle(_) => TagType::Style,
        TagKind::RichText { .. } => TagType::RichText,
        TagKind::Image { .. } => TagType::Image,
        TagKind::TableMerge(_) => TagType::TableMerge,
        TagKind::RawRun => TagType::RawRun,
        TagKind::Placeholder => TagType::Placeholder,
        TagKind::Footnote => TagType::Footnote,
        TagKind::FullTable => TagType::Table,
        TagKind::GridOpen => TagType::Grid,
        TagKind::Include => TagType::Include,
        TagKind::SegmentOpen { .. } => TagType::Segment,
        TagKind::IncludeSegment { .. } => TagType::IncludeSegment,
        TagKind::Section(_) => TagType::Section,
        TagKind::Close | TagKind::StyleEnd => return None,
    };
    Some(kind)
}
