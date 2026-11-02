//! Parses a part into XML, text and tags, and pairs its loops with the XML they repeat.

use std::ops::Range;

use std::collections::BTreeMap;

use crate::comments::CommentDefaults;
use crate::expressions::Expression;
use crate::sections::SectionTag;
use crate::styling::StyleKind;
use crate::table_merge::{Merge, read_merge};
use crate::tag_list::{TemplateTag, list_tags};
use crate::xml::{ElementKind, Token, check_balanced, escape, read_attribute, tokenize, unescape};
use crate::{Delimiters, Extensions, FillError, FillOptions};

mod vertical_columns;

const PARAGRAPH: &str = "w:p";
const TEXT: &str = "w:t";
const CELL: &str = "w:tc";
const ROW: &str = "w:tr";
const RUN: &str = "w:r";
const RUN_PROPS: &str = "w:rPr";
const BREAK: &str = "w:br";
const PRESERVE_SPACE: &str = " xml:space=\"preserve\"";
const VERTICAL_START: &str = ":vt#";
// Word lengths are twentieths of a point; pixels are 1/96 inch.
pub(crate) const PX_PER_TWIP: f64 = 96.0 / 1440.0;

#[derive(Debug)]
pub(crate) enum Part {
    Element { kind: ElementKind, name: String, raw: String },
    Text(String),
    Tag(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TagKind {
    /// `{name}`
    Value,
    /// `{@name}`: raw XML in place of the tag's paragraph.
    Raw,
    /// `{#name}`, `{^name}` (inverted) or `{-w:tr name}` (repeats the named element).
    Open { is_inverted: bool, element: Option<String> },
    /// `{/name}`, or `{/}` for any loop.
    Close,
    /// A tag starting with `FillOptions::module_prefixes[index]`.
    Module(usize),
    /// `{$name}`: the preceding chart's category and series data.
    Chart,
    /// `{:stylecell name}`: a background for the table cell the tag is in.
    CellStyle,
    /// `{~~name}` (block) or `{~name}` (inline): HTML printed as Word runs and paragraphs.
    RichText { is_block: bool },
    /// `{%name}` or `{%%name}`: an image filling its table cell, else the page's width. With
    /// centred images on, `{%%name}` centres its paragraph.
    Image { cell_width: Option<f64>, is_centered: bool },
    /// `{:stylepar name}`, `{:stylerow name}`, `{:stylerun name}`, `{:stylebullets name}`,
    /// `{:styleshape name}` or `{:stylestart name}`.
    Style(StyleTarget),
    /// `{:/style}`: ends the innermost `{:stylestart}` range.
    StyleEnd,
    /// `{::stylepar name}` or `{::stylerun name}`: restyles the definition of the tag
    /// paragraph's or run's style.
    GlobalStyle(StyleKind),
    /// `{:merge-cells-col 1,2}` or `{:merge-cells-row}`: merges equal cells of the tag's table.
    TableMerge(Merge),
    /// `{r@name}`: replaces its run with the value as raw XML.
    RawRun,
    /// `{?name}`: keeps, drops or repeats its paragraph by the value.
    Placeholder,
    /// `{:footnote name}`: a footnote holding the value.
    Footnote,
    /// `{:table name}`: a table built from the value, in place of the tag's paragraph.
    FullTable,
    /// `{:#grid name}`: the paragraphs up to `{:/grid}` printed once per item in a grid of cells.
    GridOpen,
    /// `{:include name}`: subtemplate bodies in place of the tag's paragraph.
    Include,
    /// `{:segment name}` or `{:inlinesegment name}`: a segment's definition up to its closing tag.
    SegmentOpen { is_inline: bool },
    /// `{:includesegment name}`, `{:includeinlinesegment name}`, or with `*` a segment named by
    /// the value.
    IncludeSegment { is_inline: bool, is_dynamic: bool },
    /// `{:subsection name}`, `{:replacesection name}` or `{:page name}`.
    Section(SectionTag),
}

/// What a styling tag styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StyleTarget {
    /// `{:stylepar}` and `{:stylebullets}`.
    Paragraph,
    Row,
    Run,
    Shape,
    /// `{:stylestart}`: the runs up to `{:/style}`.
    Range,
}

#[derive(Debug)]
pub(crate) struct Tag {
    pub kind: TagKind,
    pub name: String,
    /// The tag as written, without its delimiters.
    pub text: String,
    pub part: usize,
    pub paragraph: usize,
    pub offset: usize,
    /// Nothing else is in its paragraph's text.
    pub is_alone: bool,
    /// A value tag's `<w:rPr>`, empty when its run has none; `None` outside a run or without
    /// line breaks.
    pub run_props: Option<String>,
}

#[derive(Debug)]
pub(crate) enum BlockKind {
    Loop { name: String, is_inverted: bool, page_breaks: LoopPageBreaks },
    /// A raw tag, and the tag as written.
    Raw { name: String, written: String },
    /// A paragraph holding `{?name}` tags, printed by their values.
    Placeholder { names: Vec<String> },
    /// A `{:table}` tag's paragraph.
    FullTable { name: String, is_in_cell: bool },
    /// A chart tag's otherwise empty paragraph.
    Chart { name: String },
    /// A grid's tag paragraphs and the paragraphs between them, the body.
    Grid { name: String },
    /// An `{:include}` tag's paragraph.
    Include { name: String },
    /// A segment's definition, printed only where included.
    Segment { name: String },
    /// An `{:includesegment}` tag's paragraph, or an inline one's tag.
    IncludeSegment { name: String, is_dynamic: bool },
    /// A section tag's paragraph.
    Section { name: String, tag: SectionTag },
}

/// Page breaks in a paragraph loop's tag paragraphs, which would go with the tags.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct LoopPageBreaks {
    /// The opening tag's paragraph has one: each item starts with a paragraph holding a break.
    pub before_each: bool,
    /// The closing tag's paragraph has one: a break ends the loop's last paragraph.
    pub after_last: bool,
}

/// A range of parts a loop or raw tag replaces: `start..end` is printed as `body_start..body_end`
/// once per item.
#[derive(Debug)]
pub(crate) struct Block {
    pub kind: BlockKind,
    pub start: usize,
    pub end: usize,
    pub body_start: usize,
    pub body_end: usize,
    pub children: Vec<usize>,
    open_tag: usize,
}

/// A parsed template part, ready to fill any number of times.
#[derive(Debug)]
pub struct Template {
    pub(crate) parts: Vec<Part>,
    pub(crate) tags: Vec<Tag>,
    pub(crate) blocks: Vec<Block>,
    /// Whether a loop repeats cells, which changes how many columns a table needs.
    pub(crate) has_cell_loops: bool,
    /// Blocks not inside another, in document order.
    pub(crate) top: Vec<usize>,
    pub(crate) linebreaks: bool,
    pub(crate) html_extras: bool,
    pub(crate) soft_placeholders: bool,
    pub(crate) html_images: bool,
    pub(crate) table_resize: crate::TableResize,
    pub(crate) html_style_sheet: String,
    pub(crate) html_options: crate::HtmlOptions,
    pub(crate) drop_style_paragraphs: bool,
    /// With comments on, the comment author and date a value gives none of.
    pub(crate) comment_defaults: Option<CommentDefaults>,
    pub(crate) footnotes: crate::FootnoteOptions,
    /// Tag names read as expressions, with expressions on.
    pub(crate) expressions: BTreeMap<String, Expression>,
    /// The largest drawing id the part uses, so new drawings take ids after it.
    pub(crate) max_drawing_id: u32,
    /// The largest bookmark id the part uses, so continuation bookmarks take ids after it.
    pub(crate) max_bookmark_id: u32,
    pub(crate) continued_bookmarks: Option<String>,
    /// Page width and height in pixels, less margins, from the part's last section.
    pub(crate) body: Option<(f64, f64)>,
    pub(crate) picture_tags: BTreeMap<String, PictureTag>,
}

#[derive(Debug)]
pub(crate) struct PictureTag {
    pub name: String,
    pub literal: String,
}

/// An image tag inside a picture's decoded alt text or name.
pub(crate) fn read_image_tag(text: &str, options: &FillOptions) -> Option<PictureTag> {
    let mut from = 0;
    while let Some(start) = text[from..].find(&options.delimiters.start).map(|at| from + at) {
        let name_start = start + options.delimiters.start.len();
        let close = text[name_start..].find(&options.delimiters.end).map(|at| name_start + at)?;
        let end = close + options.delimiters.end.len();
        let (kind, name) = read_tag(text[name_start..close].trim(), options);
        if matches!(kind, TagKind::Image { .. }) && !name.is_empty() {
            return Some(PictureTag { name, literal: text[start..end].to_string() });
        }
        from = end;
    }
    None
}

/// A tag found in a paragraph's joined text: byte offsets of its opening delimiter, its name, and
/// after its closing delimiter.
struct FoundTag {
    start: usize,
    name: Range<usize>,
    end: usize,
    /// A `{=[[ ]]=}` tag, which prints nothing.
    changes_delimiters: bool,
}

struct Paragraph {
    start: usize,
    end: usize,
}

/// Where a part's text sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextLayout {
    /// In `<w:t>` elements of `<w:p>` paragraphs.
    Paragraphs,
    /// In any element, each text its own paragraph, as in `docProps/core.xml`.
    Plain,
}

/// A paragraph's start token and, once it closes, its end token.
type ParagraphTokens = (usize, Option<usize>);

impl TagKind {
    /// Whether the tag reads a value by its name.
    fn reads_value(&self) -> bool {
        match self {
            TagKind::Close | TagKind::StyleEnd | TagKind::TableMerge(_) | TagKind::SegmentOpen { .. } | TagKind::Module(_) => false,
            TagKind::IncludeSegment { is_dynamic, .. } => *is_dynamic,
            _ => true,
        }
    }

    /// Whether printing the tag's own position uses its value. A loop's or block's tags do not,
    /// as their blocks read it.
    pub(crate) fn prints_value(&self) -> bool {
        !matches!(self, TagKind::Raw | TagKind::RawRun | TagKind::Open { .. } | TagKind::Close | TagKind::FullTable | TagKind::GridOpen | TagKind::Include | TagKind::SegmentOpen { .. } | TagKind::IncludeSegment { .. } | TagKind::Section(_) | TagKind::StyleEnd | TagKind::TableMerge(_))
    }
}

/// Each name a value tag reads, parsed, with expressions on.
fn parse_expressions(tags: &[Tag], options: &FillOptions, errors: &mut Vec<FillError>) -> BTreeMap<String, Expression> {
    let mut expressions = BTreeMap::new();
    if !options.expressions { return expressions; }
    for tag in tags.iter().filter(|tag| tag.kind.reads_value()) {
        if expressions.contains_key(&tag.name) { continue; }
        match Expression::parse(&tag.name, &options.filters) {
            Ok(expression) => { expressions.insert(tag.name.clone(), expression); }
            Err(reason) => errors.push(FillError::new(format!("The tag \"{}\" is not a valid expression: {reason}", tag.name)).with_id(crate::FillErrorId::ScopeParserCompilation).at(tag.text.clone(), tag.offset, tag.paragraph)),
        }
    }
    expressions
}

impl Template {
    /// Each tag in document order, closing tags left out. A loop's, grid's or segment's tags are
    /// its children, so the list follows the shape of the data the template reads.
    pub fn tags(&self) -> Vec<TemplateTag> {
        list_tags(self)
    }

    /// Parses `xml`, a body, header or footer using `{ }` delimiters.
    ///
    /// @returns An error for an unclosed or unopened tag or loop, a loop closed by another loop's
    ///   tag, or a dash loop outside the element it names.
    pub fn parse(xml: &str, options: &FillOptions) -> Result<Template, FillError> {
        Template::parse_text(xml, options, TextLayout::Paragraphs)
    }

    pub(crate) fn parse_text(xml: &str, options: &FillOptions, layout: TextLayout) -> Result<Template, FillError> {
        options.tag_prefixes.validate()?;
        let expanded = (options.extensions.paragraph_placeholder && options.paragraph_placeholder.soft_breaks != crate::SoftBreakScope::None)
            .then(|| crate::placeholder_lines::expand(xml, options.paragraph_placeholder.soft_breaks));
        let source_changed = expanded.as_deref().is_some_and(|expanded| expanded != xml);
        let xml = expanded.as_deref().unwrap_or(xml);
        let tokens = tokenize(xml).map_err(|message| FillError::new(message).with_id(crate::FillErrorId::MalformedXml))?;
        check_balanced(&tokens).map_err(|message| FillError::new(message).with_id(crate::FillErrorId::MalformedXml))?;
        let (paragraph_tokens, text_tokens) = scan_paragraphs(&tokens, layout);

        // Tags by paragraph, found in its `<w:t>` text joined, since Word splits a tag across runs.
        let mut found = Vec::with_capacity(text_tokens.len());
        let mut joined_texts = Vec::with_capacity(text_tokens.len());
        let mut delimiters = options.delimiters.clone();
        let mut errors = Vec::new();
        for (paragraph, texts) in text_tokens.iter().enumerate() {
            let joined: String = texts.iter().map(|&i| text_of(&tokens[i])).collect();
            found.push(find_tags(&joined, &mut delimiters, options, paragraph, &mut errors));
            joined_texts.push(joined);
        }

        let mut builder = Builder { parts: Vec::with_capacity(tokens.len()), tags: Vec::new() };
        let mut paragraphs: Vec<Paragraph> = paragraph_tokens.iter().map(|_| Paragraph { start: 0, end: 0 }).collect();
        let mut paragraph_starts = vec![None; tokens.len()];
        let mut paragraph_ends = vec![None; tokens.len()];
        for (paragraph, &(start, end)) in paragraph_tokens.iter().enumerate() {
            paragraph_starts[start] = Some(paragraph);
            if let Some(end) = end {
                paragraph_ends[end] = Some(paragraph);
            }
        }
        let mut text_owner = vec![None; tokens.len()];
        for (paragraph, texts) in text_tokens.iter().enumerate() {
            let mut offset = 0;
            for &token in texts {
                let length = text_of(&tokens[token]).len();
                text_owner[token] = Some((paragraph, offset));
                offset += length;
            }
        }

        // Each open paragraph and how far into its joined text the tokens have reached.
        let mut reached: Vec<(usize, usize)> = Vec::new();
        for (index, token) in tokens.iter().enumerate() {
            match *token {
                Token::Element { kind, name, raw } => {
                    if let Some(paragraph) = paragraph_starts[index] {
                        paragraphs[paragraph].start = builder.parts.len();
                        reached.push((paragraph, 0));
                    }
                    if let Some(paragraph) = paragraph_ends[index] {
                        paragraphs[paragraph].end = builder.parts.len();
                        reached.pop();
                    }
                    // A break inside a tag goes with the tag, as in core.
                    if kind == ElementKind::SelfClosing && name == BREAK && reached.last().is_some_and(|&(paragraph, at)| found[paragraph].iter().any(|tag| tag.start < at && at <= tag.name.end)) {
                        continue;
                    }
                    let touched = kind == ElementKind::Open && name == TEXT && is_touched(&tokens, index + 1, &text_owner, &found);
                    let raw = if touched { preserve_space(raw) } else { raw.to_string() };
                    builder.parts.push(Part::Element { kind, name: name.to_string(), raw });
                }
                Token::Text(text) => match text_owner[index] {
                    Some((paragraph, offset)) => {
                        if let Some(last) = reached.last_mut().filter(|(open, _)| *open == paragraph) {
                            last.1 = offset + text.len();
                        }
                        builder.split_text(text, offset, paragraph, &found[paragraph], &joined_texts[paragraph], options)
                    }
                    None => builder.parts.push(Part::Text(text.to_string())),
                },
            }
        }

        let Builder { parts, mut tags } = builder;
        for tag in &mut tags {
            let paragraph = &found[tag.paragraph];
            tag.is_alone = paragraph.len() == 1 && paragraph[0].end - paragraph[0].start == joined_texts[tag.paragraph].len();
        }

        for tag in &mut tags {
            match &mut tag.kind {
                TagKind::Image { cell_width, .. } => *cell_width = read_cell_width(&parts, tag.part),
                TagKind::Value if options.linebreaks || options.extensions.comments => tag.run_props = read_run_props(&parts, tag.part),
                TagKind::Footnote => tag.run_props = read_run_props(&parts, tag.part),
                _ => {}
            }
        }

        let blocks = pair_blocks(&parts, &tags, &paragraphs, options, &mut errors);
        let has_cell_loops = blocks.iter().any(|block| matches!(&tags[block.open_tag].kind, TagKind::Open { element: Some(element), .. } if element == CELL));
        let mut expressions = parse_expressions(&tags, options, &mut errors);
        let mut picture_tags = BTreeMap::new();
        if options.extensions.images {
            for part in &parts {
                let Part::Element { name, raw, .. } = part else { continue };
                if name != "wp:docPr" { continue; }
                for attribute in ["descr", "name"] {
                    let Some(text) = read_attribute(raw, attribute).map(unescape) else { continue };
                    let Some(tag) = read_image_tag(&text, options) else { continue };
                    if options.expressions {
                        match Expression::parse(&tag.name, &options.filters) {
                            Ok(expression) => { expressions.insert(tag.name.clone(), expression); }
                            Err(reason) => {
                                let mut error = FillError::new(format!("The tag \"{}\" is not a valid expression: {reason}", tag.name)).with_id(crate::FillErrorId::ScopeParserCompilation);
                                error.tag = Some(tag.literal.clone().into_boxed_str());
                                errors.push(error);
                            }
                        }
                    }
                    picture_tags.insert(text, tag);
                }
            }
        }
        let mut template = Template {
            has_cell_loops,
            picture_tags,
            expressions,
            max_drawing_id: crate::images::read_max_drawing_id(xml),
            max_bookmark_id: crate::footnotes::read_max_element_id(xml, "w:bookmarkStart"),
            continued_bookmarks: options.continued_bookmarks.clone(),
            body: crate::images::read_body_size(xml),
            parts,
            tags,
            blocks,
            top: Vec::new(),
            linebreaks: options.linebreaks && layout == TextLayout::Paragraphs,
            html_extras: options.extensions.html_extras,
            soft_placeholders: options.paragraph_placeholder.soft_breaks != crate::SoftBreakScope::None,
            html_images: options.extensions.images,
            table_resize: options.table.resize,
            html_style_sheet: options.html_style_sheet.clone(),
            html_options: options.html.clone(),
            drop_style_paragraphs: options.drop_style_paragraphs,
            comment_defaults: options.extensions.comments.then(|| options.comments.clone()),
            footnotes: options.footnotes.clone(),
        };
        template.nest_blocks(&mut errors);
        if !errors.is_empty() {
            // Expanded soft lines do not have coordinates in the original document.
            if source_changed {
                for error in &mut errors { error.paragraph = None; error.offset = None; }
            }
            return Err(FillError::many(errors));
        }
        Ok(template)
    }

    /// Puts each block under the innermost block containing it.
    fn nest_blocks(&mut self, errors: &mut Vec<FillError>) {
        let mut order: Vec<usize> = (0..self.blocks.len()).collect();
        order.sort_by_key(|&b| (self.blocks[b].start, std::cmp::Reverse(self.blocks[b].end), self.blocks[b].open_tag));

        let mut open: Vec<usize> = Vec::new();
        for block in order {
            while open.last().is_some_and(|&outer| self.blocks[outer].end <= self.blocks[block].start) {
                open.pop();
            }
            match open.last() {
                Some(&outer) if self.blocks[block].end > self.blocks[outer].end => {
                    let tag = &self.tags[self.blocks[block].open_tag];
                    errors.push(FillError::new(format!("The loops \"{}\" and \"{}\" overlap", self.block_name(outer), self.block_name(block))).with_id(crate::FillErrorId::LoopPositionInvalid).at(tag.text.clone(), tag.offset, tag.paragraph));
                    continue;
                }
                Some(&outer) => self.blocks[outer].children.push(block),
                None => self.top.push(block),
            }
            open.push(block);
        }
    }

    fn block_name(&self, block: usize) -> &str {
        match &self.blocks[block].kind {
            BlockKind::Loop { name, .. } | BlockKind::Raw { name, .. } => name,
            BlockKind::Placeholder { names } => names.first().map_or("", String::as_str),
            BlockKind::FullTable { name, .. } | BlockKind::Grid { name } | BlockKind::Chart { name } => name,
            BlockKind::Include { name } | BlockKind::Segment { name } | BlockKind::IncludeSegment { name, .. } | BlockKind::Section { name, .. } => name,
        }
    }
}

struct Builder {
    parts: Vec<Part>,
    tags: Vec<Tag>,
}

impl Builder {
    /// Pushes one `<w:t>` text as text and tags. A tag goes where its `{` is; the text of a tag
    /// that spilled into this run from an earlier one is dropped.
    fn split_text(&mut self, text: &str, offset: usize, paragraph: usize, found: &[FoundTag], joined: &str, options: &FillOptions) {
        let end = offset + text.len();
        let mut cursor = offset;
        for tag in found.iter().filter(|tag| tag.end > offset && tag.start < end) {
            if tag.start > cursor {
                self.parts.push(Part::Text(joined[cursor..tag.start].to_string()));
            }
            if tag.start >= offset && !tag.changes_delimiters {
                let written = &joined[tag.name.clone()];
                let (kind, name) = read_tag(&unescape(written), options);
                self.tags.push(Tag { kind, name, text: written.to_string(), part: self.parts.len(), paragraph, offset: tag.start, is_alone: false, run_props: None });
                self.parts.push(Part::Tag(self.tags.len() - 1));
            }
            cursor = tag.end.min(end);
        }
        if cursor < end {
            self.parts.push(Part::Text(joined[cursor..end].to_string()));
        }
    }
}

/// Each innermost paragraph's start and end token, and its `<w:t>` text tokens.
fn scan_paragraphs(tokens: &[Token], layout: TextLayout) -> (Vec<ParagraphTokens>, Vec<Vec<usize>>) {
    let mut paragraphs = Vec::new();
    let mut texts: Vec<Vec<usize>> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    let mut is_in_text = false;
    for (index, token) in tokens.iter().enumerate() {
        match *token {
            Token::Text(_) if layout == TextLayout::Plain => {
                paragraphs.push((index, Some(index)));
                texts.push(vec![index]);
            }
            Token::Element { kind: ElementKind::Open, name: PARAGRAPH, .. } => {
                open.push(paragraphs.len());
                paragraphs.push((index, None));
                texts.push(Vec::new());
            }
            Token::Element { kind: ElementKind::SelfClosing, name: PARAGRAPH, .. } => {
                paragraphs.push((index, Some(index)));
                texts.push(Vec::new());
            }
            Token::Element { kind: ElementKind::Close, name: PARAGRAPH, .. } => {
                if let Some(paragraph) = open.pop() {
                    paragraphs[paragraph].1 = Some(index);
                }
            }
            Token::Element { kind: ElementKind::Open, name: TEXT, .. } => is_in_text = true,
            Token::Element { kind: ElementKind::Close, name: TEXT, .. } => is_in_text = false,
            Token::Text(_) if is_in_text => {
                if let Some(&paragraph) = open.last() {
                    texts[paragraph].push(index);
                }
            }
            _ => {}
        }
    }
    (paragraphs, texts)
}

fn text_of<'a>(token: &Token<'a>) -> &'a str {
    match *token {
        Token::Text(text) => text,
        Token::Element { .. } => "",
    }
}

/// Whether the text token at `index` holds part of a tag.
fn is_touched(tokens: &[Token], index: usize, owner: &[Option<(usize, usize)>], found: &[Vec<FoundTag>]) -> bool {
    let Some(Some((paragraph, offset))) = owner.get(index) else { return false };
    let end = offset + text_of(&tokens[index]).len();
    found[*paragraph].iter().any(|tag| tag.end > *offset && tag.start < end)
}

fn preserve_space(raw: &str) -> String {
    if raw.contains("xml:space=") {
        return raw.to_string();
    }
    let at = raw.len() - 1;
    format!("{}{PRESERVE_SPACE}{}", &raw[..at], &raw[at..])
}

/// The tags in a paragraph's joined text.
fn find_tags(text: &str, delimiters: &mut Delimiters, options: &FillOptions, paragraph: usize, errors: &mut Vec<FillError>) -> Vec<FoundTag> {
    let mut start = DelimiterForms::new(&delimiters.start);
    let mut end = DelimiterForms::new(&delimiters.end);
    let mut tags: Vec<FoundTag> = Vec::new();
    let mut open: Option<(usize, usize)> = None;
    let mut at = 0;
    while at < text.len() {
        let identical = delimiters.start == delimiters.end;
        let rest = &text[at..];
        let closed = if let Some(length) = start.find(rest) {
            let closed = match open {
                Some((opened, name)) if identical => Some(FoundTag { start: opened, name: name..at, end: at + length, changes_delimiters: false }),
                Some(_) if options.allow_unclosed_tag => { open = Some((at, at + length)); None }
                Some((opened, name)) => {
                    let error = if name == at { duplicate_open_tag(text, opened, &end) } else { unclosed_tag(&text[opened..at]) };
                    let context = if name == at { tag_context(text, at + length, &end) } else { unescape(&text[name..at]) };
                    errors.push(error.at(context, opened, paragraph));
                    open = Some((at, at + length));
                    None
                }
                None if start.overlap_end(text, at, length).is_some() => {
                    if !options.allow_unclosed_tag {
                        errors.push(unclosed_tag(&text[at..]).at(tag_context(text, at + length, &end), at, paragraph));
                    }
                    at += rest.chars().next().map_or(1, char::len_utf8);
                    continue;
                }
                None => { open = Some((at, at + length)); None }
            };
            at += length;
            closed
        } else if let Some(length) = end.find(rest).filter(|_| !identical) {
            let Some((opened, name)) = open.take() else {
                if !options.allow_unopened_tag {
                    // A duplicate close names the tag it follows.
                    let tag = match tags.last() { Some(tag) if tag.end == at => tag.name.clone(), last => last.map_or(0, |tag| tag.end)..at };
                    errors.push(unopened_tag(text, &tags, at, length).at(unescape(&text[tag]), at, paragraph));
                }
                at += length;
                continue;
            };
            if let Some(overlap_end) = end.overlap_end(text, at, length).filter(|_| !options.allow_unopened_tag) {
                let context = unescape(&text[name..overlap_end]);
                errors.push(FillError::new(format!("The tag ending with \"{context}\" has duplicate close tags")).with_id(crate::FillErrorId::DuplicateCloseTag).at(unescape(&text[name..at]), at, paragraph));
            }
            at += length;
            Some(FoundTag { start: opened, name: name..at - length, end: at, changes_delimiters: false })
        } else {
            at += rest.chars().next().map_or(1, char::len_utf8);
            None
        };
        let Some(mut tag) = closed else { continue };
        open = None;
        let name = unescape(&text[tag.name.clone()]);
        match read_new_delimiters(&name, options.change_delimiter_prefix.as_deref()) {
            Ok(Some(next)) => {
                *delimiters = next;
                start = DelimiterForms::new(&delimiters.start);
                end = DelimiterForms::new(&delimiters.end);
                tag.changes_delimiters = true;
            }
            Err(error) => {
                errors.push(error.with_id(crate::FillErrorId::InvalidDelimiter).at(name, tag.start, paragraph));
                tag.changes_delimiters = true;
            }
            Ok(None) => {}
        }
        tags.push(tag);
    }
    if let Some((opened, name)) = open.filter(|_| !options.allow_unclosed_tag) {
        errors.push(unclosed_tag(&text[opened..]).at(unescape(&text[name..]), opened, paragraph));
    }
    tags
}

fn tag_context(text: &str, from: usize, end: &DelimiterForms) -> String {
    let to = text[from..].char_indices().map(|(offset, _)| from + offset).find(|&at| end.find(&text[at..]).is_some()).unwrap_or(text.len());
    unescape(&text[from..to])
}

/// The delimiters a `=[[ ]]=` tag sets, for `prefix` `=`. `None` for any other tag.
///
/// @returns An error for a tag starting with `prefix` that does not hold two delimiters without
///   whitespace or `prefix`, ended by `prefix`.
fn read_new_delimiters(tag: &str, prefix: Option<&str>) -> Result<Option<Delimiters>, FillError> {
    let Some(prefix) = prefix.filter(|prefix| !prefix.is_empty()) else { return Ok(None) };
    let Some(rest) = tag.strip_prefix(prefix) else { return Ok(None) };

    let invalid = || FillError::new(format!("New delimiters cannot be parsed from \"{tag}\""));
    let inner = rest.strip_suffix(prefix).ok_or_else(invalid)?;
    let (start, end) = inner.split_once(char::is_whitespace).ok_or_else(invalid)?;
    let end = end.trim_start();
    let is_valid = |delimiter: &str| !delimiter.is_empty() && !delimiter.contains(char::is_whitespace) && !delimiter.contains(prefix);
    if !is_valid(start) || !is_valid(end) {
        return Err(invalid());
    }
    Ok(Some(Delimiters { start: start.to_string(), end: end.to_string() }))
}

/// A delimiter as it can appear in a part's text: `<<` is `&lt;&lt;`, and `>>` is `&gt;&gt;` or
/// `>>`.
struct DelimiterForms {
    escaped: String,
    /// Unless it holds `&` or `<`, which XML text never holds raw.
    raw: Option<String>,
}

impl DelimiterForms {
    fn new(delimiter: &str) -> Self {
        let mut escaped = String::new();
        escape(delimiter, &mut escaped);
        let raw = (!delimiter.contains(['&', '<'])).then(|| delimiter.to_string());
        DelimiterForms { escaped, raw }
    }

    /// The byte length of the delimiter `text` starts with.
    fn find(&self, text: &str) -> Option<usize> {
        std::iter::once(&self.escaped).chain(&self.raw).find(|form| text.starts_with(form.as_str())).map(String::len)
    }

    /// Where another of this delimiter ends, when one starts inside the one `length` bytes long
    /// at `at`.
    fn overlap_end(&self, text: &str, at: usize, length: usize) -> Option<usize> {
        (at + 1..at + length).filter(|&inner| text.is_char_boundary(inner)).find_map(|inner| self.find(&text[inner..]).map(|found| inner + found))
    }
}

fn unclosed_tag(context: &str) -> FillError {
    FillError::new(format!("The tag beginning with \"{}\" is unclosed", unescape(context))).with_id(crate::FillErrorId::UnclosedTag)
}

/// `{{name}`: a second open delimiter straight after the first.
fn duplicate_open_tag(text: &str, opened: usize, end: &DelimiterForms) -> FillError {
    let context_end = text[opened..].char_indices().map(|(offset, _)| opened + offset).find(|&at| end.find(&text[at..]).is_some()).unwrap_or(text.len());
    FillError::new(format!("The tag beginning with \"{}\" has duplicate open tags", unescape(&text[opened..context_end]))).with_id(crate::FillErrorId::DuplicateOpenTag)
}

/// A close delimiter with no tag open. `{name}}`, straight after a tag's close, is a duplicate.
fn unopened_tag(text: &str, tags: &[FoundTag], at: usize, length: usize) -> FillError {
    match tags.last() {
        Some(tag) if tag.end == at => {
            let context = unescape(&text[tag.name.start..at + length]);
            FillError::new(format!("The tag ending with \"{context}\" has duplicate close tags")).with_id(crate::FillErrorId::DuplicateCloseTag)
        }
        last => {
            let context_start = last.map_or(0, |tag| tag.end);
            FillError::new(format!("The tag beginning with \"{}\" is unopened", unescape(&text[context_start..at]))).with_id(crate::FillErrorId::UnopenedTag)
        }
    }
}

/// A tag's kind and the name it looks up.
fn read_tag(text: &str, options: &FillOptions) -> (TagKind, String) {
    if let Some((index, prefix)) = options.module_prefixes.iter().enumerate().find(|(_, prefix)| text.starts_with(prefix.as_str())) {
        return (TagKind::Module(index), text[prefix.len()..].to_string());
    }
    if let Some(found) = read_extension_tag(text, &options.extensions) {
        return found;
    }

    let prefixes = &options.tag_prefixes;
    let kinds = [
        (&prefixes.loop_start, true, TagKind::Open { is_inverted: false, element: None }),
        (&prefixes.inverted_loop, true, TagKind::Open { is_inverted: true, element: None }),
        (&prefixes.loop_end, true, TagKind::Close),
        (&prefixes.raw, true, TagKind::Raw),
        (&prefixes.word_run, options.extensions.word_run, TagKind::RawRun),
        (&prefixes.html, options.extensions.rich_text, TagKind::RichText { is_block: false }),
        (&prefixes.block_html, options.extensions.rich_text, TagKind::RichText { is_block: true }),
        (&prefixes.image, options.extensions.images, TagKind::Image { cell_width: None, is_centered: false }),
        (&prefixes.centered_image, options.extensions.images, TagKind::Image { cell_width: None, is_centered: options.extensions.centered_images }),
    ];
    let found = kinds.into_iter().filter(|(prefix, enabled, _)| *enabled && text.starts_with(prefix.as_str())).max_by_key(|(prefix, _, _)| prefix.len());
    let named = |kind: TagKind, rest: &str| {
        let name = if matches!(kind, TagKind::Image { .. } | TagKind::RawRun) { rest.trim() } else { rest };
        (kind, name.to_string())
    };
    // An empty prefix only takes tags nothing else reads, element loops included.
    let fallback = match found {
        Some((prefix, _, kind)) if !prefix.is_empty() => return named(kind, &text[prefix.len()..]),
        Some((_, _, kind)) => Some(kind),
        None => None,
    };

    let rest = text.get(1..).unwrap_or_default().to_string();
    // `-w:tr rows`: the element, then the name. Without both, `-a` is a plain name.
    if text.starts_with('-')
        && let Some((element, name)) = rest.split_once(char::is_whitespace)
        && !element.is_empty()
    {
        return (TagKind::Open { is_inverted: false, element: Some(element.to_string()) }, name.trim_start().to_string());
    }
    fallback.map_or_else(|| (TagKind::Value, text.to_string()), |kind| named(kind, text))
}

fn read_extension_tag(text: &str, extensions: &Extensions) -> Option<(TagKind, String)> {
    if extensions.charts && let Some(name) = text.strip_prefix('$') {
        return Some((TagKind::Chart, name.trim().to_string()));
    }
    if extensions.sections {
        for (prefix, tag) in [(":subsection ", SectionTag::Subsection), (":replacesection ", SectionTag::Replace), (":page ", SectionTag::Page)] {
            if let Some(name) = text.strip_prefix(prefix) {
                return Some((TagKind::Section(tag), name.trim().to_string()));
            }
        }
    }
    if extensions.subtemplates {
        if matches!(text.trim(), ":segment/" | ":inlinesegment/") {
            return Some((TagKind::Close, String::new()));
        }
        let prefixes = [
            (":include ", TagKind::Include),
            (":segment ", TagKind::SegmentOpen { is_inline: false }),
            (":inlinesegment ", TagKind::SegmentOpen { is_inline: true }),
            (":includesegment* ", TagKind::IncludeSegment { is_inline: false, is_dynamic: true }),
            (":includesegment ", TagKind::IncludeSegment { is_inline: false, is_dynamic: false }),
            (":includeinlinesegment* ", TagKind::IncludeSegment { is_inline: true, is_dynamic: true }),
            (":includeinlinesegment ", TagKind::IncludeSegment { is_inline: true, is_dynamic: false }),
        ];
        for (prefix, kind) in prefixes {
            if let Some(name) = text.strip_prefix(prefix) {
                return Some((kind, name.trim().to_string()));
            }
        }
    }
    if extensions.full_tables {
        if let Some(name) = text.strip_prefix(":table ") {
            return Some((TagKind::FullTable, name.trim().to_string()));
        }
        if let Some(name) = text.strip_prefix(":#grid ") {
            return Some((TagKind::GridOpen, name.trim().to_string()));
        }
        if text.trim() == ":/grid" {
            return Some((TagKind::Close, String::new()));
        }
    }
    if extensions.table_merge {
        for (prefix, is_columns) in [(":merge-cells-col", true), (":merge-cells-row", false)] {
            let Some(rest) = text.strip_prefix(prefix) else { continue };
            if rest.is_empty() || rest.starts_with(char::is_whitespace) {
                return Some((TagKind::TableMerge(read_merge(is_columns, rest)), String::new()));
            }
        }
    }
    if extensions.styling {
        for (prefix, kind) in [("::stylepar ", StyleKind::Paragraph), ("::stylerun ", StyleKind::Character)] {
            if let Some(name) = text.strip_prefix(prefix) {
                return Some((TagKind::GlobalStyle(kind), name.trim().to_string()));
            }
        }
        if text.trim() == ":/style" {
            return Some((TagKind::StyleEnd, String::new()));
        }
        let targets = [
            (":stylepar ", StyleTarget::Paragraph),
            (":stylebullets ", StyleTarget::Paragraph),
            (":stylerow ", StyleTarget::Row),
            (":stylerun ", StyleTarget::Run),
            (":styleshape ", StyleTarget::Shape),
            (":stylestart ", StyleTarget::Range),
        ];
        for (prefix, target) in targets {
            if let Some(name) = text.strip_prefix(prefix) {
                return Some((TagKind::Style(target), name.trim().to_string()));
            }
        }
    }
    if extensions.footnotes
        && let Some(name) = text.strip_prefix(":footnote ")
    {
        return Some((TagKind::Footnote, name.trim().to_string()));
    }
    if extensions.paragraph_placeholder
        && let Some(name) = text.strip_prefix('?')
    {
        return Some((TagKind::Placeholder, name.trim().to_string()));
    }
    if extensions.vertical_tables {
        // `{:vt#cols}` … `{:vt/cols}` repeat the cells from the opening tag's to the closing tag's.
        if let Some(name) = text.strip_prefix(VERTICAL_START) {
            return Some((TagKind::Open { is_inverted: false, element: Some(CELL.to_string()) }, name.trim().to_string()));
        }
        if let Some(name) = text.strip_prefix(":vt/") {
            return Some((TagKind::Close, name.trim().to_string()));
        }
    }
    if extensions.cell_styling
        && let Some(name) = text.strip_prefix(":stylecell ")
    {
        return Some((TagKind::CellStyle, name.trim().to_string()));
    }
    None
}

/// The width in pixels of the innermost table cell around part `at`, when its `<w:tcW>` gives
/// one in twips.
fn read_cell_width(parts: &[Part], at: usize) -> Option<f64> {
    let (start, _) = find_enclosing(parts, at, CELL)?;
    for part in &parts[start + 1..at] {
        let Part::Element { name, raw, .. } = part else { continue };
        if name == PARAGRAPH {
            return None;
        }
        if name == "w:tcW" {
            if read_attribute(raw, "w:type") != Some("dxa") {
                return None;
            }
            return read_attribute(raw, "w:w")?.parse::<f64>().ok().map(|twips| twips * PX_PER_TWIP);
        }
    }
    None
}

/// The raw `<w:rPr>` opening the run around part `at`, empty when there is none.
fn read_run_props(parts: &[Part], at: usize) -> Option<String> {
    let (start, _) = find_enclosing(parts, at, RUN)?;
    let mut props = String::new();
    for part in &parts[start + 1..at] {
        let Part::Element { kind, name, raw } = part else { break };
        match (kind, name.as_str()) {
            (ElementKind::SelfClosing, RUN_PROPS) => return Some(String::new()),
            (ElementKind::Close, RUN_PROPS) => {
                props.push_str(raw);
                return Some(props);
            }
            _ if props.is_empty() && name != RUN_PROPS => break,
            _ => props.push_str(raw),
        }
    }
    Some(String::new())
}

/// Pairs each loop's tags and works out the parts every loop and raw tag replaces.
fn pair_blocks(parts: &[Part], tags: &[Tag], paragraphs: &[Paragraph], options: &FillOptions, errors: &mut Vec<FillError>) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    for (index, tag) in tags.iter().enumerate() {
        let result = (|| -> Result<(), FillError> {
        match tag.kind {
            TagKind::Open { .. } | TagKind::GridOpen | TagKind::SegmentOpen { .. } => open.push(index),
            TagKind::Close => {
                let Some(opening) = open.pop() else {
                    return Err(FillError::new(format!("The tag \"{}\" has no opening loop tag", tag.text)).with_id(crate::FillErrorId::UnopenedLoop));
                };
                if !tag.name.is_empty() && tag.name != tags[opening].name {
                    return Err(FillError::new(format!("The tag \"{}\" is closed by the tag \"{}\"", tags[opening].name, tag.name)).with_id(crate::FillErrorId::ClosingTagMismatch));
                }
                if tags[opening].kind == TagKind::GridOpen {
                    blocks.push(expand_grid(tags, paragraphs, opening, index)?);
                    return Ok(());
                }
                if let TagKind::SegmentOpen { is_inline } = tags[opening].kind {
                    blocks.push(expand_segment(tags, paragraphs, opening, index, is_inline)?);
                    return Ok(());
                }
                check_cell_loops(parts, tags, opening, index, &options.delimiters)?;
                if let Some(columns) = vertical_columns::column_blocks(parts, tags, opening, index)? {
                    blocks.extend(columns);
                    return Ok(());
                }
                blocks.push(expand_loop(parts, tags, paragraphs, opening, index, options)?);
            }
            TagKind::Raw if !tag.is_alone => {
                return Err(FillError::new(format!("The raw tag \"{}\" should be the only text in its paragraph", tag.name)));
            }
            TagKind::Raw | TagKind::RawRun => {
                let element = if tag.kind == TagKind::Raw { PARAGRAPH } else { RUN };
                let (start, end) = find_enclosing(parts, tag.part, element).ok_or_else(|| not_inside(tag, element))?;
                blocks.push(Block {
                    kind: BlockKind::Raw { name: tag.name.clone(), written: tag.text.clone() },
                    start,
                    end: end + 1,
                    body_start: end + 1,
                    body_end: end + 1,
                    children: Vec::new(),
                    open_tag: index,
                });
            }
            TagKind::Image { is_centered: true, .. } if !tag.is_alone => {
                return Err(FillError::new(format!("Centered Images should be placed in empty paragraphs, but the tag \"{}\" is not", tag.text)));
            }
            TagKind::Include | TagKind::IncludeSegment { is_inline: false, .. } => {
                if !tag.is_alone {
                    return Err(segment_not_alone());
                }
                let (start, end) = find_enclosing(parts, tag.part, PARAGRAPH).ok_or_else(|| not_inside(tag, PARAGRAPH))?;
                let kind = match tag.kind {
                    TagKind::IncludeSegment { is_dynamic, .. } => BlockKind::IncludeSegment { name: tag.name.clone(), is_dynamic },
                    _ => BlockKind::Include { name: tag.name.clone() },
                };
                blocks.push(Block { kind, start, end: end + 1, body_start: end + 1, body_end: end + 1, children: Vec::new(), open_tag: index });
            }
            TagKind::IncludeSegment { is_inline: true, is_dynamic } => blocks.push(Block {
                kind: BlockKind::IncludeSegment { name: tag.name.clone(), is_dynamic },
                start: tag.part,
                end: tag.part + 1,
                body_start: tag.part + 1,
                body_end: tag.part + 1,
                children: Vec::new(),
                open_tag: index,
            }),
            TagKind::Section(section) => {
                let (start, end) = find_enclosing(parts, tag.part, PARAGRAPH).ok_or_else(|| not_inside(tag, PARAGRAPH))?;
                let kind = BlockKind::Section { name: tag.name.clone(), tag: section };
                blocks.push(Block { kind, start, end: end + 1, body_start: end + 1, body_end: end + 1, children: Vec::new(), open_tag: index });
            }
            TagKind::FullTable => {
                if !tag.is_alone && options.table.paragraph_validation == crate::TableValidation::Error {
                    return Err(FillError::new(format!("The table tag \"{}\" should be the only text in its paragraph", tag.text)));
                }
                let (start, end) = find_enclosing(parts, tag.part, PARAGRAPH).ok_or_else(|| not_inside(tag, PARAGRAPH))?;
                blocks.push(Block {
                    kind: BlockKind::FullTable { name: tag.name.clone(), is_in_cell: find_enclosing(parts, tag.part, CELL).is_some() },
                    start,
                    end: end + 1,
                    body_start: end + 1,
                    body_end: end + 1,
                    children: Vec::new(),
                    open_tag: index,
                });
            }
            TagKind::Placeholder => {
                let paragraph = find_enclosing(parts, tag.part, PARAGRAPH).ok_or_else(|| not_inside(tag, PARAGRAPH))?;
                let line = matches!(&parts[paragraph.0], Part::Element { raw, .. } if crate::placeholder_lines::is_line(raw));
                let shape = if options.paragraph_placeholder.expand_to_shape && !line && find_enclosing(parts, tag.part, "w:txbxContent").is_some() {
                    find_enclosing(parts, tag.part, "v:shape").or_else(|| find_enclosing(parts, tag.part, "w:drawing"))
                } else { None };
                let (start, end) = shape.unwrap_or(paragraph);
                // One block per paragraph, naming each of its placeholders.
                if let Some(Block { kind: BlockKind::Placeholder { names }, .. }) = blocks.iter_mut().find(|block| block.start == start && matches!(block.kind, BlockKind::Placeholder { .. })) {
                    names.push(tag.name.clone());
                    return Ok(());
                }
                blocks.push(Block {
                    kind: BlockKind::Placeholder { names: vec![tag.name.clone()] },
                    start,
                    end: end + 1,
                    body_start: start,
                    body_end: end + 1,
                    children: Vec::new(),
                    open_tag: index,
                });
            }
            TagKind::Chart => {
                let (start, end) = find_enclosing(parts, tag.part, PARAGRAPH).ok_or_else(|| not_inside(tag, PARAGRAPH))?;
                let chart_count = |items: &[Part]| items.iter().filter(|part| matches!(part, Part::Element { kind: ElementKind::Open | ElementKind::SelfClosing, name, .. } if name == "c:chart")).count();
                let has_chart = chart_count(&parts[start..tag.part]) > 0;
                if !has_chart {
                    let previous = parts[..start].iter().rposition(|part| matches!(part, Part::Element { kind: ElementKind::Open, name, .. } if name == PARAGRAPH));
                    let previous_count = previous.map_or(0, |previous| chart_count(&parts[previous..start]));
                    let has_later = chart_count(&parts[tag.part..=end]) > 0;
                    if !has_later && tag.is_alone && previous_count > 1 {
                        return Err(FillError::new(format!("The chart tag \"{}\" is ambiguous because the preceding paragraph contains multiple charts", tag.text)));
                    }
                    if has_later || !tag.is_alone || previous_count != 1 {
                        return Err(FillError::new(format!("The chart tag \"{}\" must be in its chart's paragraph or alone immediately after it", tag.text)));
                    }
                    blocks.push(Block { kind: BlockKind::Chart { name: tag.name.clone() }, start, end: end + 1, body_start: end + 1, body_end: end + 1, children: Vec::new(), open_tag: index });
                }
            }
            TagKind::Value
            | TagKind::Module(_)
            | TagKind::CellStyle
            | TagKind::RichText { .. }
            | TagKind::Image { .. }
            | TagKind::Style(_)
            | TagKind::StyleEnd
            | TagKind::GlobalStyle(_)
            | TagKind::TableMerge(_)
            | TagKind::Footnote => {}
        }
        Ok(())
        })();
        if let Err(mut error) = result {
            if error.id == crate::FillErrorId::TemplateError {
                error.id = match tag.kind {
                    TagKind::Raw => crate::FillErrorId::RawTagNotAlone,
                    TagKind::RawRun => crate::FillErrorId::NoXmlTagFound,
                    TagKind::Image { .. } => crate::FillErrorId::ImageTagNotAlone,
                    TagKind::FullTable => crate::FillErrorId::TableTagNotAlone,
                    TagKind::Include | TagKind::IncludeSegment { .. } | TagKind::SegmentOpen { .. } => crate::FillErrorId::SegmentTagNotAlone,
                    _ => crate::FillErrorId::LoopPositionInvalid,
                };
            }
            errors.push(error.at(tag.text.clone(), tag.offset, tag.paragraph));
        }
    }
    for opening in open.into_iter().rev() {
        let tag = &tags[opening];
        errors.push(FillError::new(format!("The loop with tag \"{}\" is unclosed", tag.name)).with_id(crate::FillErrorId::UnclosedLoop).at(tag.text.clone(), tag.offset, tag.paragraph));
    }
    for tag in tags {
        if !matches!(tag.kind, TagKind::IncludeSegment { is_dynamic: false, .. }) { continue; }
        if !blocks.iter().any(|block| matches!(&block.kind, BlockKind::Segment { name } if *name == tag.name)) {
            errors.push(FillError::new(format!("The subsegment for \"{}\" must be defined in order to include it", tag.name)).with_id(crate::FillErrorId::UndefinedSegment).at(tag.text.clone(), tag.offset, tag.paragraph));
        }
    }
    blocks
}

fn segment_not_alone() -> FillError {
    FillError::new("Segment tag should be the only text in a paragraph").with_id(crate::FillErrorId::SegmentTagNotAlone)
}

/// A segment's parts: a block segment's tag paragraphs, each holding only its tag, and the
/// paragraphs between; an inline segment's tags and the parts between.
fn expand_segment(tags: &[Tag], paragraphs: &[Paragraph], opening: usize, closing: usize, is_inline: bool) -> Result<Block, FillError> {
    let (open, close) = (&tags[opening], &tags[closing]);
    let kind = BlockKind::Segment { name: open.name.clone() };
    if is_inline {
        return Ok(Block { kind, start: open.part, end: close.part + 1, body_start: open.part + 1, body_end: close.part, children: Vec::new(), open_tag: opening });
    }
    let (first, last) = (&paragraphs[open.paragraph], &paragraphs[close.paragraph]);
    if open.paragraph == close.paragraph || !open.is_alone || !close.is_alone || first.end <= first.start || last.end <= last.start {
        return Err(segment_not_alone());
    }
    Ok(Block { kind, start: first.start, end: last.end + 1, body_start: first.end + 1, body_end: last.start, children: Vec::new(), open_tag: opening })
}

/// A grid's parts: its tags' paragraphs, each holding only its tag, and the paragraphs between.
fn expand_grid(tags: &[Tag], paragraphs: &[Paragraph], opening: usize, closing: usize) -> Result<Block, FillError> {
    let (open, close) = (&tags[opening], &tags[closing]);
    let (first, last) = (&paragraphs[open.paragraph], &paragraphs[close.paragraph]);
    if open.paragraph == close.paragraph || !open.is_alone || !close.is_alone || first.end <= first.start || last.end <= last.start {
        return Err(FillError::new(format!("The grid tags \"{}\" should each be alone in their own paragraph", open.name)));
    }
    Ok(Block {
        kind: BlockKind::Grid { name: open.name.clone() },
        start: first.start,
        end: last.end + 1,
        body_start: first.end + 1,
        body_end: last.start,
        children: Vec::new(),
        open_tag: opening,
    })
}

/// The parts a loop repeats:
/// - a dash loop's element, from the one holding the opening tag to the one holding the closing;
/// - the table rows it spans, when its tags sit in different cells;
/// - with `paragraph_loop`, the paragraphs between its tags, when each tag is alone in its own
///   paragraph (the tags' paragraphs go);
/// - otherwise the XML between its tags.
fn expand_loop(parts: &[Part], tags: &[Tag], paragraphs: &[Paragraph], opening: usize, closing: usize, options: &FillOptions) -> Result<Block, FillError> {
    let (open, close) = (&tags[opening], &tags[closing]);
    let TagKind::Open { is_inverted, element } = &open.kind else { unreachable!("pairs start at an opening tag") };
    let block = |start, end, body_start, body_end, page_breaks| Block {
        kind: BlockKind::Loop { name: open.name.clone(), is_inverted: *is_inverted, page_breaks },
        start,
        end,
        body_start,
        body_end,
        children: Vec::new(),
        open_tag: opening,
    };

    let element = match element {
        Some(element) => Some(element.clone()),
        None => is_crossing_cells(parts, open.part, close.part).map_err(|_| misplaced_loop(open))?.then(|| ROW.to_string()),
    };
    if let Some(element) = element {
        let (start, _) = find_enclosing(parts, open.part, &element).ok_or_else(|| not_inside(open, &element))?;
        let (_, end) = find_enclosing(parts, close.part, &element).ok_or_else(|| not_inside(close, &element))?;
        return Ok(block(start, end + 1, start, end + 1, LoopPageBreaks::default()));
    }

    let is_paragraph_loop = options.paragraph_loop && open.paragraph != close.paragraph && open.is_alone && close.is_alone;
    if is_paragraph_loop {
        let (first, last) = (&paragraphs[open.paragraph], &paragraphs[close.paragraph]);
        if first.end > first.start && last.end > last.start {
            let page_breaks = LoopPageBreaks { before_each: has_page_break(&parts[first.start..first.end]), after_last: has_page_break(&parts[last.start..last.end]) };
            return Ok(block(first.start, last.end + 1, first.end + 1, last.start, page_breaks));
        }
    }
    Ok(block(open.part, close.part + 1, open.part + 1, close.part, LoopPageBreaks::default()))
}

fn has_page_break(parts: &[Part]) -> bool {
    parts.iter().any(|part| matches!(part, Part::Element { name, raw, .. } if name == "w:br" && read_attribute(raw, "w:type") == Some("page")))
}

/// Whether a loop's tags sit in different table cells. An error when only one is in a cell.
fn is_crossing_cells(parts: &[Part], from: usize, to: usize) -> Result<bool, ()> {
    match (find_enclosing(parts, from, CELL), find_enclosing(parts, to, CELL)) {
        (Some(open), Some(close)) => Ok(open != close),
        (None, None) => Ok(false),
        _ => Err(()),
    }
}

fn misplaced_loop(tag: &Tag) -> FillError {
    FillError::new(format!("The position of the loop tags \"{}\" would produce invalid XML", tag.name))
}

/// The start and end parts of the innermost `element` around part `at`.
fn find_enclosing(parts: &[Part], at: usize, element: &str) -> Option<(usize, usize)> {
    let mut depth = 0usize;
    let mut start = None;
    for index in (0..at).rev() {
        let Part::Element { kind, name, .. } = &parts[index] else { continue };
        if name != element {
            continue;
        }
        match kind {
            ElementKind::Close => depth += 1,
            ElementKind::Open if depth == 0 => {
                start = Some(index);
                break;
            }
            ElementKind::Open => depth -= 1,
            _ => {}
        }
    }
    let start = start?;

    for (index, part) in parts.iter().enumerate().skip(at + 1) {
        let Part::Element { kind, name, .. } = part else { continue };
        if name != element {
            continue;
        }
        match kind {
            ElementKind::Open => depth += 1,
            ElementKind::Close if depth == 0 => return Some((start, index)),
            ElementKind::Close => depth -= 1,
            _ => {}
        }
    }
    None
}

/// A loop across table cells repeats the row, so no other loop may close before it in its first
/// cell or open after it in its last: `{#a}1{/a}{#b}` | `2{/b}`.
fn check_cell_loops(parts: &[Part], tags: &[Tag], opening: usize, closing: usize, delimiters: &Delimiters) -> Result<(), FillError> {
    let first_cell = find_enclosing(parts, tags[opening].part, CELL);
    let last_cell = find_enclosing(parts, tags[closing].part, CELL);
    if first_cell.is_none() || first_cell == last_cell {
        return Ok(());
    }

    let is_in = |cell: Option<(usize, usize)>, tag: &Tag| find_enclosing(parts, tag.part, CELL) == cell;
    let before = tags[..opening].iter().rev().take_while(|tag| is_in(first_cell, tag)).find(|tag| tag.kind == TagKind::Close);
    let after = tags[closing + 1..].iter().take_while(|tag| is_in(last_cell, tag)).find(|tag| matches!(tag.kind, TagKind::Open { .. }));
    let Some(other) = before.or(after) else { return Ok(()) };

    let (start, end) = (&delimiters.start, &delimiters.end);
    let pair = |name: &str| format!("{start}#{name}{end}{start}/{name}{end}");
    let (loop_name, other_name) = (&tags[opening].name, &other.name);
    let written = if before.is_some() { pair(other_name) + &pair(loop_name) } else { pair(loop_name) + &pair(other_name) };
    Err(FillError::new(format!("Unbalanced loop tags {written}: a loop across table cells cannot share its first or last cell with another loop")).with_id(crate::FillErrorId::UnbalancedLoopTags))
}

fn not_inside(tag: &Tag, element: &str) -> FillError {
    FillError::new(format!("The tag \"{}\" is not inside a <{element}>", tag.text)).with_id(crate::FillErrorId::NoXmlTagFound)
}
