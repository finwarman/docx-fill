//! `{~~name}` and `{~name}`: HTML printed as Word paragraphs and runs. Built from the
//! WordprocessingML spec and the HTML a sanitiser lets through: p, br, h1-h6, strong/b, em/i,
//! u, ul, ol and li, without attributes.
//!
//! A block tag alone in its paragraph prints each HTML paragraph, heading or list item as a Word
//! paragraph with the tag paragraph's properties and the tag run's font; bold, italic and
//! underline carry over. A tag sharing its paragraph with other text, or an inline tag, prints
//! inline, paragraphs joined by line breaks. An empty value leaves the tag's paragraph empty.
//!
//! With extras on, it also reads attributes and the HTML module's documented elements: links,
//! s/del/strike, ins, sup, sub, mark, code, pre, hr, blockquote, div, center, tables, checkboxes,
//! `<ol start>`, `<li value>`, list-style-type, and inline `style` (color, background-color,
//! font-size in px or pt, font-family, font-weight, font-style, text-decoration, text-align).

use crate::cell_styling::find_element_start;
use crate::properties::{PARAGRAPH_ORDER, RUN_ORDER, read_child, set_attribute, set_child};
use crate::xml::{ElementKind, Token, escape, tokenize, unescape};
use std::collections::BTreeMap;
use std::sync::Arc;

pub(crate) mod numbering;
pub(crate) mod headings;
mod css;
mod preserve;
mod table;
use table::{CellLayout, MergeRow, TableLayout};
pub(crate) mod images;
use numbering::Numberings;

// Stands in for a value in the filled XML until its paragraph closes.
pub(crate) const MARK_START: char = '\u{E000}';
pub(crate) const MARK_END: char = '\u{E001}';

const LIST_INDENT_TWIPS: u32 = 720;
const LIST_HANGING_TWIPS: u32 = 360;
// Word numbering levels are zero-based, with nine levels.
const MAX_LIST_LEVEL: usize = 8;
const QUOTE_INDENT_TWIPS: u32 = 720;
const BULLET: &str = "\u{2022}";
// Half-points, h1 to h6.
const HEADING_SIZES: [u32; 6] = [32, 28, 26, 24, 24, 24];
const CODE_FONT: &str = "Courier New";
const LINK_COLOR: &str = "0563C1";
const CHECKED: &str = "\u{2612}";
const UNCHECKED: &str = "\u{2610}";
const OBJECT_REPLACEMENT: char = '\u{fffc}';
const TWIPS_PER_POINT: f64 = 20.0;
const POINTS_PER_CSS_PIXEL: f64 = 0.75;
const LINE_HEIGHT_UNIT: f64 = 240.0;
const DEFAULT_FONT_POINTS: f64 = 11.0;
const CELL_SIDES: [&str; 4] = ["top", "left", "bottom", "right"];
const RAW_INLINE: &str = "docx-fill-inline";
const RAW_BLOCK: &str = "docx-fill-block";
const RAW_TAG_ATTRIBUTE: &str = "data-docx-fill-tag";
const STYLE_ATTRIBUTE: &str = "data-docx-fill-style";
// Writing and dropping a table recurses, and a value decides how deep it goes.
const MAX_TABLE_DEPTH: usize = 32;
/// Word's column limit, so a user's `colspan` cannot grow the output without bound.
const MAX_COLUMN_SPAN: usize = 63;
/// Elements whose contents never print.
const HIDDEN_ELEMENTS: [&str; 3] = ["script", "style", "template"];

pub(crate) struct RichValue {
    pub html: String,
    pub is_block: bool,
    pub has_extras: bool,
    pub style_sheet: String,
    pub block_mode: crate::HtmlBlockMode,
    pub raw: Vec<crate::HtmlFragment>,
    pub headings: Option<Arc<headings::Headings>>,
    /// Content width in twips, `None` when the template does not give it.
    pub container_width: Option<u32>,
}

pub(crate) enum ImageMode { Embed, Ignore }

pub(crate) struct PreparedHtml {
    pub images: Vec<images::HtmlImage>,
    pub html: String,
    pub raw: Vec<crate::HtmlFragment>,
    pub errors: Vec<crate::FillError>,
    pub has_block: bool,
    pub has_headings: bool,
}

/// How a run looks beyond the tag run's own properties.
#[derive(Debug, Clone, Default, PartialEq)]
struct Format {
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    vert_align: Option<&'static str>,
    highlight: Option<&'static str>,
    color: Option<Arc<str>>,
    /// Half-points.
    size: Option<u32>,
    font: Option<Arc<str>>,
    shade: Option<Arc<str>>,
    link: Option<Arc<str>>,
    pre_spaces: Option<bool>,
    rtl: Option<bool>,
    paragraph_style: Option<Arc<str>>,
}

impl Format {
    /// This format with `over`'s settings on top.
    fn and(mut self, over: &Format) -> Format {
        self.bold = over.bold.or(self.bold);
        self.italic = over.italic.or(self.italic);
        self.underline = over.underline.or(self.underline);
        self.strike = over.strike.or(self.strike);
        self.vert_align = over.vert_align.or(self.vert_align);
        self.highlight = over.highlight.or(self.highlight);
        self.color = over.color.clone().or(self.color);
        self.size = over.size.or(self.size);
        self.font = over.font.clone().or(self.font);
        self.shade = over.shade.clone().or(self.shade);
        self.link = over.link.clone().or(self.link);
        self.pre_spaces = over.pre_spaces.or(self.pre_spaces);
        self.rtl = over.rtl.or(self.rtl);
        self.paragraph_style = over.paragraph_style.clone().or(self.paragraph_style);
        self
    }
}

#[derive(Debug, Clone, Default)]
struct Run {
    text: Option<String>,
    is_break: bool,
    is_bold: bool,
    is_italic: bool,
    is_underlined: bool,
    format: Format,
    preserve_spaces: bool,
    raw_xml: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct Block {
    runs: Vec<Run>,
    heading_level: Option<usize>,
    list: Option<ListItem>,
    align: Option<&'static str>,
    quote_level: u32,
    is_rule: bool,
    table: Option<Vec<Vec<Cell>>>,
    table_layout: Option<TableLayout>,
    layout: ParagraphLayout,
    raw_xml: Option<String>,
    paragraph_style: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineHeight { Multiple(u32), Exact(u32) }

#[derive(Debug, Clone, Copy, Default)]
struct ParagraphLayout {
    left: Option<i64>, right: Option<i64>, before: Option<i64>, after: Option<i64>,
    text_indent: Option<i64>, line_height: Option<LineHeight>, rtl: Option<bool>,
    page_before: bool, page_after: bool,
}

impl ParagraphLayout {
    fn and(self, over: Self) -> Self {
        Self { left: over.left.or(self.left), right: over.right.or(self.right), before: over.before.or(self.before), after: over.after.or(self.after),
            text_indent: over.text_indent.or(self.text_indent), line_height: over.line_height.or(self.line_height), rtl: over.rtl.or(self.rtl),
            page_before: over.page_before || self.page_before, page_after: over.page_after || self.page_after }
    }
    fn inherited(self) -> Self { Self { before: None, after: None, page_before: false, page_after: false, ..self } }
    /// `inner` inside this layout: side margins add up, as CSS boxes nest.
    fn nest(self, inner: Self) -> Self {
        let add = |outer: Option<i64>, inner: Option<i64>| if outer.is_none() && inner.is_none() { None } else { Some(outer.unwrap_or(0).saturating_add(inner.unwrap_or(0))) };
        Self { left: add(self.left, inner.left), right: add(self.right, inner.right), ..self.and(inner) }
    }
}

#[derive(Debug, Clone, Copy)]
struct ListItem {
    group: usize,
    level: usize,
    format: Numbering,
    value: i64,
}

#[derive(Clone, Copy)]
enum MarkerFont { Inherited, Symbol }

impl MarkerFont {
    fn from_run(props: &str) -> Self {
        let inner = read_child(props, "w:rPr").unwrap_or("");
        if read_child(inner, "w:rFonts").is_some() { Self::Inherited } else { Self::Symbol }
    }
}

struct ListWriter<'a> {
    numbering: Option<&'a mut Numberings>,
    ids: BTreeMap<usize, u32>,
    marker_font: MarkerFont,
}

struct BlockWriter<'a, 'n> {
    run_props: &'a str,
    links: &'a mut Vec<String>,
    lists: ListWriter<'n>,
    width: Option<u32>,
}

#[derive(Default)]
struct SpaceEdges {
    start: bool,
    end: bool,
}

impl SpaceEdges {
    fn between(before: Option<char>, after: Option<char>) -> Self {
        Self {
            start: before.is_some_and(|c| !is_html_space(c)),
            end: after.is_some_and(|c| !is_html_space(c)),
        }
    }
}

impl ListWriter<'_> {
    fn id_for(&mut self, item: &ListItem) -> Option<u32> {
        if let Some(id) = self.ids.get(&item.group) {
            return Some(*id);
        }
        let id = self.numbering.as_mut()?.add(item.format, item.level, item.value, self.marker_font)?;
        self.ids.insert(item.group, id);
        Some(id)
    }
}

#[derive(Debug, Clone, Default)]
struct Cell {
    span: usize,
    row_span: usize,
    blocks: Vec<Block>,
    layout: CellLayout,
}

#[derive(Clone, Copy)]
enum CellMerge { None, Start, Continue }

struct PlacedCell<'a> {
    span: usize,
    merge: CellMerge,
    blocks: &'a [Block],
    layout: CellLayout,
}

fn place_cells(rows: &[Vec<Cell>]) -> Vec<Vec<PlacedCell<'_>>> {
    let mut active = BTreeMap::<usize, (usize, usize, &CellLayout)>::new();
    let mut placed = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let mut occupied = active.into_iter().peekable();
        active = BTreeMap::new();
        let mut cells = row.iter().peekable();
        let mut column = 0;
        let mut output = Vec::new();
        while cells.peek().is_some() || occupied.peek().is_some() {
            if occupied.peek().is_some_and(|(at, _)| *at == column) {
                let (_, (span, left, origin)) = occupied.next().expect("checked above");
                let row = if left > 1 { MergeRow::Middle } else { MergeRow::Last };
                output.push(PlacedCell { span, merge: CellMerge::Continue, blocks: &[], layout: origin.in_merge(row) });
                if left > 1 { active.insert(column, (span, left - 1, origin)); }
                column += span;
                continue;
            }
            let next = occupied.peek().map(|(at, _)| *at);
            if let Some(cell) = cells.peek()
                && next.is_none_or(|at| column + cell.span <= at) {
                let cell = cells.next().expect("checked above");
                let height = if cell.row_span == 0 { rows.len() - index } else { cell.row_span.min(rows.len() - index) };
                let (merge, layout) = if height > 1 {
                    active.insert(column, (cell.span, height - 1, &cell.layout));
                    (CellMerge::Start, cell.layout.in_merge(MergeRow::First))
                } else { (CellMerge::None, cell.layout.clone()) };
                output.push(PlacedCell { span: cell.span, merge, blocks: &cell.blocks, layout });
                column += cell.span;
                continue;
            }
            if let Some(next) = next {
                output.push(PlacedCell { span: next - column, merge: CellMerge::None, blocks: &[], layout: CellLayout::default() });
                column = next;
            }
        }
        placed.push(output);
    }
    placed
}

pub(crate) fn mark(index: usize) -> String {
    format!("{MARK_START}{index}{MARK_END}")
}

/// A filled paragraph holding rich text marks, rewritten with the values' runs and paragraphs.
/// External links are added to `links`, whose position names their relationship. `continued`
/// gives the bookmark prefix whose later paragraphs get numbered bookmarks, and the next free id.
pub(crate) fn write_paragraph(paragraph: &str, values: &[RichValue], links: &mut Vec<String>, continued: Option<(&str, &mut u32)>, numbering: Option<&mut Numberings>) -> String {
    let paragraph = without_empty_marks(paragraph, values);
    if preserve::has_blocks(&paragraph, values) {
        return preserve::write(&paragraph, values, links, continued, numbering);
    }
    let mut paragraph = write_earlier_marks(&paragraph, values, links);
    while let Some((mark, index)) = find_mark(&paragraph) {
        // Only printed values hold marks by accident, and those have no value to print.
        let Some(value) = values.get(index) else {
            paragraph = paragraph.replace(&mark, "");
            continue;
        };
        let other_text = read_text(&paragraph).replacen(&mark, "", 1);
        let run = find_run(&paragraph, &mark);
        let run_props = run.and_then(|run| find_element(run, "w:rPr")).unwrap_or("");
        let is_block = value.is_block && (other_text.trim().is_empty() || value.block_mode == crate::HtmlBlockMode::Block) && run.is_some();
        let spaces = if is_block { SpaceEdges::default() } else {
            let text = read_flow_text(&paragraph);
            let (before, after) = text.split_once(&mark).unwrap_or(("", ""));
            SpaceEdges::between(before.chars().next_back(), after.chars().next())
        };
        let mut blocks = read_html(&value.html, value.has_extras, spaces, &value.style_sheet, &value.raw);
        if is_block && let Some(headings) = &value.headings { apply_headings(&mut blocks, &headings.ids); }

        // Alone in its paragraph: the paragraph becomes the value's paragraphs.
        if is_block && !blocks.is_empty() {
            return write_blocks(&paragraph, &blocks, run_props, links, continued, numbering, value.container_width);
        }

        // Beside other text: the runs go where the tag was, joined by breaks.
        let inline = join_inline(&blocks, run_props, links);
        let replaced = match run {
            Some(run) => paragraph.replacen(run, &split_run(run, &mark, &inline), 1),
            None => paragraph.replacen(&mark, "", 1),
        };
        // Broken XML can copy a mark into a run's properties, and it would never go.
        paragraph = replaced.replace(&mark, "");
    }
    paragraph
}

/// Empty fragments must not hide the real text on either side of another fragment.
fn without_empty_marks(paragraph: &str, values: &[RichValue]) -> String {
    let mut out = String::with_capacity(paragraph.len());
    let mut rest = paragraph;
    while let Some((mark, index)) = find_mark(rest) {
        let at = rest.find(&mark).unwrap_or(0);
        out.push_str(&rest[..at]);
        if let Some(value) = values.get(index)
            && !read_html(&value.html, value.has_extras, SpaceEdges { start: true, end: true }, &value.style_sheet, &value.raw).is_empty() {
            out.push_str(&mark);
        }
        rest = &rest[at + mark.len()..];
    }
    out.push_str(rest);
    out
}

/// `paragraph` with every mark but its last printed inline, in one pass, as only a last mark can
/// be alone in its paragraph.
fn write_earlier_marks(paragraph: &str, values: &[RichValue], links: &mut Vec<String>) -> String {
    let mut left = paragraph.matches(MARK_START).count().saturating_sub(1);
    let mut out = String::with_capacity(paragraph.len());
    let flow = read_flow_text(paragraph);
    let mut following = flow.as_str();
    let mut previous = None;
    let mut from = 0;
    while left > 0 {
        let Some(at) = find_element_start(&paragraph[from..], "<w:r") else { break };
        let start = from + at;
        let Some(length) = paragraph[start..].find("</w:r>") else { break };
        let end = start + length + "</w:r>".len();
        let run = &paragraph[start..end];
        out.push_str(&paragraph[from..start]);
        from = end;
        if !run.contains(MARK_START) {
            out.push_str(run);
            continue;
        }

        let run_props = find_element(run, "w:rPr").unwrap_or("");
        let mut rest = run.to_string();
        while left > 0 {
            let Some((mark, index)) = find_mark(&read_text(&rest)) else { break };
            let (written, cut) = cut_run(&rest, &mark);
            out.push_str(&written);
            let (before, after) = following.split_once(&mark).unwrap_or(("", ""));
            previous = before.chars().next_back().or(previous);
            if let Some(value) = values.get(index) {
                let spaces = SpaceEdges::between(previous, after.chars().next());
                let inline = join_inline(&read_html(&value.html, value.has_extras, spaces, &value.style_sheet, &value.raw), run_props, links);
                previous = read_flow_text(&inline).chars().next_back().or(previous);
                out.push_str(&inline);
            }
            following = after;
            rest = cut;
            left -= 1;
        }
        out.push_str(&rest);
    }
    out.push_str(&paragraph[from..]);
    out
}

/// The paragraph as the value's paragraphs, in place of its own.
fn write_blocks(paragraph: &str, blocks: &[Block], run_props: &str, links: &mut Vec<String>, continued: Option<(&str, &mut u32)>, numbering: Option<&mut Numberings>, width: Option<u32>) -> String {
    let open = &paragraph[..paragraph.find('>').map_or(0, |at| at + 1)];
    let paragraph_props = find_element(paragraph, "w:pPr").unwrap_or("");
    let bookmarks = find_bookmarks(paragraph);
    let mut continued = continued.and_then(|(prefix, next_id)| find_numbered_bookmark(&bookmarks, prefix).map(|name| (name, next_id)));
    let mut out = String::new();
    let mut writer = BlockWriter { run_props, links, lists: ListWriter { numbering, ids: BTreeMap::new(), marker_font: MarkerFont::from_run(run_props) }, width };
    // A section break belongs to the last paragraph only.
    let has_section = find_element_start(paragraph_props, "<w:sectPr").is_some();
    let body_props = if has_section { preserve::without(paragraph_props, "w:sectPr") } else { paragraph_props.to_string() };
    let ends_in_paragraph = blocks.last().is_some_and(|block| block.table.is_none() && block.raw_xml.is_none());
    for (index, block) in blocks.iter().enumerate() {
        let later = match (&mut continued, index) {
            (_, 0) => String::new(),
            (Some((name, next_id)), _) => {
                let id = **next_id;
                **next_id += 1;
                format!("<w:bookmarkStart w:id=\"{id}\" w:name=\"{name}_{index}\"/><w:bookmarkEnd w:id=\"{id}\"/>")
            }
            (None, _) => String::new(),
        };
        let bookmarks = if index == 0 { bookmarks.as_str() } else { later.as_str() };
        if follows_table(blocks, index) {
            out.push_str(&format!("{open}{body_props}</w:p>"));
        }
        let props = if ends_in_paragraph && index + 1 == blocks.len() { paragraph_props } else { body_props.as_str() };
        write_block(&mut out, open, props, bookmarks, block, &mut writer);
    }
    if has_section && !ends_in_paragraph {
        out.push_str(&format!("{open}{paragraph_props}</w:p>"));
    }
    // The final structure pass adds a trailing paragraph only where required.
    out
}

fn join_inline(blocks: &[Block], run_props: &str, links: &mut Vec<String>) -> String {
    blocks.iter().map(|block| write_inline(block, run_props, links)).collect::<Vec<_>>().join(&format!("<w:r>{run_props}<w:br/></w:r>"))
}

fn apply_headings(blocks: &mut [Block], styles: &[String; 6]) {
    for block in blocks {
        if block.paragraph_style.is_none() && let Some(level) = block.heading_level { block.paragraph_style = Some(styles[level - 1].clone()); }
        if let Some(rows) = &mut block.table {
            for cell in rows.iter_mut().flatten() { apply_headings(&mut cell.blocks, styles); }
        }
    }
}

fn write_block(out: &mut String, open: &str, paragraph_props: &str, bookmarks: &str, block: &Block, writer: &mut BlockWriter) {
    if let Some(raw) = &block.raw_xml { out.push_str(raw); return; }
    let Some(rows) = &block.table else {
        let number = block.list.as_ref().and_then(|item| writer.lists.id_for(item));
        let after = if block.layout.page_after { "<w:r><w:br w:type=\"page\"/></w:r>" } else { "" };
        out.push_str(&format!("{open}{}{bookmarks}{}{after}</w:p>", write_paragraph_props(paragraph_props, block, number, writer.run_props), write_runs(block, writer.run_props, writer.links, number)));
        return;
    };
    table::write(out, rows, block.table_layout.as_ref().expect("table layout accompanies rows"), writer);
}

/// Whether a table follows another table, which Word would merge into it.
fn follows_table(blocks: &[Block], index: usize) -> bool {
    index > 0 && blocks[index].table.is_some() && blocks[index - 1].table.is_some()
}

/// A block's runs for printing inside a paragraph; a table's cells are joined by tabs and its
/// rows by breaks.
fn write_inline(block: &Block, run_props: &str, links: &mut Vec<String>) -> String {
    if let Some(raw) = &block.raw_xml { return write_text_run("<w:r>", run_props, &read_text(raw)); }
    let Some(rows) = &block.table else { return write_runs(block, run_props, links, None) };
    let mut lines = Vec::new();
    for row in rows {
        let cells: Vec<String> = row.iter().map(|cell| cell.blocks.iter().map(|block| write_inline(block, run_props, links)).collect()).collect();
        lines.push(cells.join(&format!("<w:r>{run_props}<w:tab/></w:r>")));
    }
    lines.join(&format!("<w:r>{run_props}<w:br/></w:r>"))
}

fn find_mark(xml: &str) -> Option<(String, usize)> {
    let start = xml.find(MARK_START)?;
    let length = xml[start..].find(MARK_END)?;
    let index = xml[start + MARK_START.len_utf8()..start + length].parse().ok()?;
    Some((xml[start..start + length + MARK_END.len_utf8()].to_string(), index))
}

/// The run whose text holds `mark`.
fn find_run<'a>(paragraph: &'a str, mark: &str) -> Option<&'a str> {
    let mut from = 0;
    while let Some(at) = find_element_start(&paragraph[from..], "<w:r") {
        let start = from + at;
        let end = start + paragraph[start..].find("</w:r>")? + "</w:r>".len();
        let run = &paragraph[start..end];
        if run.contains(mark) {
            return Some(run);
        }
        from = end;
    }
    None
}

/// `run` with `mark` replaced by `runs`, its content either side kept.
fn split_run(run: &str, mark: &str, runs: &str) -> String {
    let (before, after) = cut_run(run, mark);
    format!("{before}{runs}{after}")
}

/// `run` cut at `mark` into the runs before and after it, keeping tabs, breaks and other content.
fn cut_run(run: &str, mark: &str) -> (String, String) {
    let open = &run[..run.find('>').map_or(0, |at| at + 1)];
    let props = find_element(run, "w:rPr").unwrap_or("");
    let body = run.strip_suffix("</w:r>").unwrap_or(run);
    let at = body.find(mark).unwrap_or(0);
    let text_start = [body[..at].rfind("<w:t>"), body[..at].rfind("<w:t ")].into_iter().flatten().max().filter(|start| *start >= open.len());
    let text_open = text_start.and_then(|start| body[start..].find('>').map(|end| start + end + 1)).filter(|end| *end <= at);
    let (Some(text_start), Some(text_open)) = (text_start, text_open) else {
        // Broken XML can put the mark outside text.
        let text = read_text(run);
        let (before, after) = text.split_once(mark).unwrap_or((&text, ""));
        return (write_text_run(open, props, before), write_text_run(open, props, after));
    };
    let text_end = at + body[at..].find("</w:t>").unwrap_or(body.len() - at);
    let tail = &body[(text_end + "</w:t>".len()).min(body.len())..];
    let text = |text: &str| if text.is_empty() { String::new() } else { format!("<w:t xml:space=\"preserve\">{text}</w:t>") };
    let before = format!("{}{}", &body[open.len()..text_start], text(&body[text_open..at]));
    let after = format!("{props}{}{tail}", text(&body[at + mark.len()..text_end]));
    let write = |content: String| if content == props { String::new() } else { format!("{open}{content}</w:r>") };
    (write(before), write(after))
}

fn write_text_run(open: &str, props: &str, text: &str) -> String {
    if text.is_empty() { String::new() } else { format!("{open}{props}<w:t xml:space=\"preserve\">{text}</w:t></w:r>") }
}

/// The joined `<w:t>` text of `xml`, as written.
fn read_text(xml: &str) -> String {
    let mut text = String::new();
    let mut from = 0;
    while let Some(at) = find_element_start(&xml[from..], "<w:t") {
        let start = from + at;
        let Some(open_end) = xml[start..].find('>').map(|end| start + end) else { break };
        from = open_end + 1;
        if xml[..open_end].ends_with('/') {
            continue;
        }
        let Some(close) = xml[from..].find("</w:t>") else { break };
        text.push_str(&xml[from..from + close]);
        from += close;
    }
    text
}

/// Text and line boundaries used to collapse only the inserted HTML's edge spaces.
fn read_flow_text(xml: &str) -> String {
    let mut text = String::new();
    let mut in_text = false;
    for token in tokenize(xml).unwrap_or_default() {
        match token {
            Token::Element { kind: ElementKind::Open, name: "w:t", .. } => in_text = true,
            Token::Element { kind: ElementKind::Close, name: "w:t", .. } => in_text = false,
            Token::Text(value) if in_text => text.push_str(&unescape(value)),
            Token::Element { kind: ElementKind::Open | ElementKind::SelfClosing, name: "w:br" | "w:cr" | "w:tab", .. } => text.push(' '),
            Token::Element { kind: ElementKind::Open | ElementKind::SelfClosing, name: "w:drawing" | "w:pict" | "w:object", .. } => text.push(OBJECT_REPLACEMENT),
            _ => {}
        }
    }
    text
}

/// The first `name` element, whole, including any nested `name` inside it.
fn find_element<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let start = find_element_start(xml, &format!("<{name}"))?;
    let end = crate::properties::find_element_end(xml, start, name)?;
    Some(&xml[start..end])
}

/// The name of the first bookmark named `prefix` and a number, such as `tag_3`.
fn find_numbered_bookmark(bookmarks: &str, prefix: &str) -> Option<String> {
    let mut rest = bookmarks;
    while let Some(at) = rest.find("w:name=\"") {
        rest = &rest[at + "w:name=\"".len()..];
        let name = &rest[..rest.find('"')?];
        if name.strip_prefix(prefix).is_some_and(|number| !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())) {
            return Some(name.to_string());
        }
    }
    None
}

/// Every self-closing `<w:bookmarkStart>` and `<w:bookmarkEnd>`, in order.
fn find_bookmarks(paragraph: &str) -> String {
    let mut found = String::new();
    let mut from = 0;
    while let Some(at) = paragraph[from..].find("<w:bookmark") {
        let start = from + at;
        let Some(end) = paragraph[start..].find('>').map(|end| start + end + 1) else { break };
        let element = &paragraph[start..end];
        if element.ends_with("/>") && (element.starts_with("<w:bookmarkStart") || element.starts_with("<w:bookmarkEnd")) {
            found.push_str(element);
        }
        from = end;
    }
    found
}

/// How list items are numbered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Numbering {
    Bullet,
    Decimal,
    UpperAlpha,
    LowerAlpha,
    UpperRoman,
    LowerRoman,
}

impl Numbering {
    fn read(name: &str) -> Option<Numbering> {
        match name.trim() {
            "1" | "decimal" => Some(Numbering::Decimal),
            "A" | "upper-alpha" | "upper-latin" => Some(Numbering::UpperAlpha),
            "a" | "lower-alpha" | "lower-latin" => Some(Numbering::LowerAlpha),
            "I" | "upper-roman" => Some(Numbering::UpperRoman),
            "i" | "lower-roman" => Some(Numbering::LowerRoman),
            "disc" | "circle" | "square" | "none" => Some(Numbering::Bullet),
            _ => None,
        }
    }

    fn marker(self, number: i64) -> String {
        match self {
            Numbering::Bullet => BULLET.to_string(),
            Numbering::Decimal => format!("{number}."),
            Numbering::UpperAlpha => format!("{}.", write_letters(number)),
            Numbering::LowerAlpha => format!("{}.", write_letters(number).to_ascii_lowercase()),
            Numbering::UpperRoman => format!("{}.", write_roman(number)),
            Numbering::LowerRoman => format!("{}.", write_roman(number).to_ascii_lowercase()),
        }
    }
}

/// 1 → A, 26 → Z, 27 → AA.
fn write_letters(number: i64) -> String {
    let mut number = number.max(1);
    let mut letters = Vec::new();
    while number > 0 {
        number -= 1;
        letters.push((b'A' + (number % 26) as u8) as char);
        number /= 26;
    }
    letters.iter().rev().collect()
}

fn write_roman(number: i64) -> String {
    const NUMERALS: [(i64, &str); 13] =
        [(1000, "M"), (900, "CM"), (500, "D"), (400, "CD"), (100, "C"), (90, "XC"), (50, "L"), (40, "XL"), (10, "X"), (9, "IX"), (5, "V"), (4, "IV"), (1, "I")];
    let mut number = number.max(1);
    let mut out = String::new();
    for (value, numeral) in NUMERALS {
        while number >= value {
            out.push_str(numeral);
            number -= value;
        }
    }
    out
}

/// What an open container element gives the blocks inside it.
#[derive(Debug, Clone, Copy, Default)]
struct Container {
    align: Option<&'static str>,
    quote_level: u32,
    is_pre: bool,
    layout: ParagraphLayout,
    collection: usize,
    first: usize,
}

impl Container {
    fn nest(self, inner: &Self) -> Self {
        Self { align: inner.align.or(self.align), quote_level: self.quote_level + inner.quote_level,
            is_pre: self.is_pre || inner.is_pre, layout: self.layout.nest(inner.layout), ..Self::default() }
    }
}

struct OpenStyle<T> { tag: String, own: T, inherited: T }

/// Cache each prefix so ordinary nested elements inherit without rescanning their ancestors.
struct StyleStack<T> { elements: Vec<OpenStyle<T>>, merge: fn(T, &T) -> T }

impl<T: Clone + Default> StyleStack<T> {
    fn new(merge: fn(T, &T) -> T) -> Self { Self { elements: Vec::new(), merge } }

    fn current(&self) -> T { self.elements.last().map(|element| element.inherited.clone()).unwrap_or_default() }

    fn push(&mut self, tag: &str, own: T) {
        let inherited = (self.merge)(self.current(), &own);
        self.elements.push(OpenStyle { tag: tag.to_string(), own, inherited });
    }

    /// Crossed closing tags leave the later elements open, with their inheritance updated.
    fn close(&mut self, tag: &str) -> Option<T> {
        let at = self.elements.iter().rposition(|element| element.tag == tag)?;
        let removed = self.elements.remove(at);
        let mut inherited = at.checked_sub(1).map(|before| self.elements[before].inherited.clone()).unwrap_or_default();
        for element in &mut self.elements[at..] {
            inherited = (self.merge)(inherited, &element.own);
            element.inherited = inherited.clone();
        }
        Some(removed.own)
    }
}

/// Builds blocks from HTML tokens.
struct Reader {
    /// Blocks being collected: the value's, then each open table cell's.
    collections: Vec<Vec<Block>>,
    current: Option<usize>,
    lists: Vec<ListItem>,
    next_list: usize,
    // Without extras, nesting counts of b/strong, i/em and u.
    counts: (i32, i32, i32),
    /// Open inline elements and the format each adds.
    formats: StyleStack<Format>,
    containers: StyleStack<Container>,
    /// Open tables: how many collections were open before each, and its rows.
    tables: Vec<(usize, Vec<Vec<Cell>>, TableLayout)>,
    /// Open tables past `MAX_TABLE_DEPTH`, printed as part of the table around them.
    ignored_tables: usize,
    spaces: SpaceEdges,
}

impl Reader {
    fn blocks(&mut self) -> &mut Vec<Block> {
        self.collections.last_mut().expect("the value's collection stays")
    }

    fn format(&self) -> Format {
        self.formats.current()
    }

    fn container(&self) -> Container {
        self.containers.current()
    }

    fn start_block(&mut self, block: Block) {
        let container = self.container();
        let paragraph_style = block.paragraph_style.clone().or_else(|| self.format().paragraph_style.map(|style| style.to_string()));
        let block = Block { align: block.align.or(container.align), quote_level: container.quote_level, layout: container.layout.inherited().nest(block.layout), paragraph_style, ..block };
        self.blocks().push(block);
        self.current = Some(self.blocks().len() - 1);
    }

    fn add_run(&mut self, text: Option<String>, is_break: bool) {
        if self.current.is_none() {
            self.start_block(Block::default());
        }
        let (bold, italic, underline) = self.counts;
        let preserve_spaces = self.preformatted();
        let run = Run { text, is_break, is_bold: bold > 0, is_italic: italic > 0, is_underlined: underline > 0, format: self.format(), preserve_spaces, raw_xml: None };
        let index = self.current.expect("started above");
        if is_break && !preserve_spaces {
            trim_spaces(&mut self.blocks()[index]);
        }
        self.blocks()[index].runs.push(run);
    }

    /// Collapse HTML whitespace across inline boundaries, without collapsing NBSP.
    fn add_text(&mut self, text: &str) {
        let keep_start = self.spaces.start && self.collections.len() == 1 && self.blocks().is_empty();
        let mut space = self.current.map_or(!keep_start, |index| {
            self.blocks()[index].runs.last().is_none_or(|run| run.is_break || (run.raw_xml.is_none() && run.text.as_deref().is_none_or(|text| text.ends_with(' '))))
        });
        let mut collapsed = String::new();
        for c in text.chars() {
            if is_html_space(c) {
                if !space {
                    collapsed.push(' ');
                }
                space = true;
            } else {
                collapsed.push(c);
                space = false;
            }
        }
        if !collapsed.is_empty() {
            self.add_run(Some(collapsed), false);
        }
    }

    fn is_in_list(&mut self) -> bool {
        let current = self.current;
        current.is_some_and(|index| self.blocks()[index].list.is_some())
    }

    fn preformatted(&self) -> bool { self.format().pre_spaces.unwrap_or(self.container().is_pre) }

    fn close_container(&mut self, name: &str) {
        let Some(container) = self.containers.close(name) else { return };
        let Some(blocks) = self.collections.get_mut(container.collection) else { return };
        if let Some(first) = blocks.get_mut(container.first) {
            first.layout.before = container.layout.before.or(first.layout.before);
            first.layout.page_before |= container.layout.page_before;
        }
        if blocks.len() > container.first
            && let Some(last) = blocks.last_mut() {
            last.layout.after = container.layout.after.or(last.layout.after);
            last.layout.page_after |= container.layout.page_after;
        }
    }

    /// Ends the open cell, if any, giving it the blocks collected in it.
    fn close_cell(&mut self) {
        let Some((depth, rows, _)) = self.tables.last_mut() else { return };
        if self.collections.len() <= *depth + 1 {
            return;
        }
        let blocks = self.collections.pop().expect("checked above");
        if let Some(cell) = rows.last_mut().and_then(|row| row.last_mut()) {
            cell.blocks = blocks;
        }
        self.current = None;
    }

}

/// The HTML's paragraphs, headings, list items and, with extras, tables, with their runs.
fn read_html(html: &str, has_extras: bool, spaces: SpaceEdges, style_sheet: &str, raw: &[crate::HtmlFragment]) -> Vec<Block> {
    let mut reader = Reader {
        collections: vec![Vec::new()],
        current: None,
        lists: Vec::new(),
        next_list: 0,
        counts: (0, 0, 0),
        formats: StyleStack::new(Format::and),
        containers: StyleStack::new(Container::nest),
        tables: Vec::new(),
        ignored_tables: 0,
        spaces,
    };

    let embedded = embedded_sheets(html);
    let sheets: Vec<&str> = std::iter::once(style_sheet).chain(embedded.iter().map(String::as_str)).collect();
    let mut cascade = css::Cascade::new(&sheets);
    for token in read_html_tokens(html, has_extras) {
        let (is_closing, tag, mut attributes) = match token {
            HtmlToken::Text(text) => {
                let decoded = decode_entities(text);
                if reader.preformatted() {
                    for (index, line) in decoded.split('\n').enumerate() {
                        if index > 0 {
                            reader.add_run(None, true);
                        }
                        if !line.is_empty() {
                            reader.add_run(Some(line.to_string()), false);
                        }
                    }
                } else {
                    reader.add_text(&decoded);
                }
                continue;
            }
            HtmlToken::Tag { is_closing, name, attributes } => (is_closing, name.to_ascii_lowercase(), attributes),
        };
        if !is_closing && matches!(tag.as_str(), RAW_INLINE | RAW_BLOCK) {
            if let Some(original) = read_attribute(&attributes, RAW_TAG_ATTRIBUTE) {
                cascade.open(original, &attributes);
                cascade.close(original);
            }
            if let Some(fragment) = read_attribute(&attributes, "index").and_then(|index| index.parse::<usize>().ok()).and_then(|index| raw.get(index)) {
                match fragment.kind {
                    crate::HtmlTagKind::Block => { reader.start_block(Block { raw_xml: Some(fragment.xml.clone()), ..Block::default() }); reader.current = None; }
                    crate::HtmlTagKind::Inline => {
                        if reader.current.is_none() { reader.start_block(Block::default()); }
                        let index = reader.current.unwrap();
                        let format = reader.format();
                        reader.blocks()[index].runs.push(Run { raw_xml: Some(fragment.xml.clone()), format, ..Run::default() });
                    }
                }
            }
            continue;
        }
        if has_extras {
            if is_closing { cascade.close(&tag); }
            else {
                let style = cascade.open(&tag, &attributes);
                attributes.retain(|(name, _)| name != "style");
                if !style.is_empty() { attributes.push(("style".to_string(), style)); }
            }
        }
        if has_extras && read_extra_tag(&mut reader, is_closing, &tag, &attributes) {
            continue;
        }

        let step = if is_closing { -1 } else { 1 };
        let heading = tag.strip_prefix('h').and_then(|level| level.parse::<usize>().ok()).filter(|level| (1..=6).contains(level));
        let is_in_list = reader.is_in_list();

        match tag.as_str() {
            "strong" | "b" => reader.counts.0 += step,
            "em" | "i" => reader.counts.1 += step,
            "u" => reader.counts.2 += step,
            // A <br> between paragraphs is a blank line.
            "br" if reader.current.is_some() => reader.add_run(None, true),
            "br" => {
                reader.blocks().push(Block::default());
                reader.current = None;
            }
            "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" if !is_in_list => {
                if is_closing {
                    reader.current = None;
                } else {
                    let align = has_extras.then(|| read_style(&attributes, "text-align").and_then(read_align)).flatten();
                    let layout = if has_extras { read_layout(&attributes) } else { ParagraphLayout::default() };
                    reader.start_block(Block { heading_level: heading, align, layout, ..Block::default() });
                }
            }
            // A list item's later paragraphs start new lines in it.
            "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" if !is_closing => {
                let index = reader.current.expect("in a list item");
                if !reader.blocks()[index].runs.is_empty() {
                    reader.add_run(None, true);
                }
            }
            "ul" | "ol" => {
                reader.current = None;
                if is_closing {
                    reader.lists.pop();
                    continue;
                }
                let numbering = if tag == "ol" { Numbering::Decimal } else { Numbering::Bullet };
                let numbering = has_extras
                    .then(|| read_style(&attributes, "list-style-type").or_else(|| read_attribute(&attributes, "type")).and_then(Numbering::read))
                    .flatten()
                    .unwrap_or(numbering);
                let start = has_extras.then(|| read_attribute(&attributes, "start").and_then(|start| start.trim().parse::<i64>().ok())).flatten().unwrap_or(1);
                let item = ListItem { group: reader.next_list, level: reader.lists.len(), format: numbering, value: start.saturating_sub(1) };
                reader.next_list += 1;
                reader.lists.push(item);
            }
            "li" => {
                match reader.lists.last().copied() {
                    Some(mut item) if !is_closing => {
                        let value = has_extras.then(|| read_attribute(&attributes, "value").and_then(|value| value.trim().parse::<i64>().ok())).flatten();
                        let next = item.value.saturating_add(1);
                        item.value = value.unwrap_or(next);
                        if value.is_some_and(|value| value != next) {
                            item.group = reader.next_list;
                            reader.next_list += 1;
                        }
                        *reader.lists.last_mut().expect("list read above") = item;
                        let layout = if has_extras { read_layout(&attributes) } else { ParagraphLayout::default() };
                        reader.start_block(Block { list: Some(item), layout, ..Block::default() });
                    }
                    _ => reader.current = None,
                }
            }
            _ => {}
        }
    }
    // A table left open ends with the value.
    while !reader.tables.is_empty() {
        read_extra_tag(&mut reader, true, "table", &[]);
    }
    let mut blocks = reader.collections.swap_remove(0);
    trim_blocks(&mut blocks, reader.spaces);
    blocks
}

fn is_html_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{c}')
}

pub(crate) fn validate_style_sheet(sheet: &str) -> Result<(), String> { css::validate_sheet(sheet) }

const KNOWN_HTML: &[&str] = &["p", "br", "h1", "h2", "h3", "h4", "h5", "h6", "strong", "b", "em", "i", "u", "ul", "ol", "li", "div", "center", "blockquote", "pre", "section", "article", "header", "footer", "main", "hr", "input", "table", "tr", "td", "th", "thead", "tbody", "tfoot", "col", "colgroup", "caption", "img", "svg", "script", "style", "template", "cite", "var", "ins", "s", "strike", "del", "sup", "sub", "mark", "code", "kbd", "samp", "tt", "a", "span", "font", "small", "big", "abbr", "q"];
const BLOCK_HTML: &[&str] = &["p", "h1", "h2", "h3", "h4", "h5", "h6", "ul", "ol", "li", "div", "center", "blockquote", "pre", "section", "article", "header", "footer", "main", "hr", "table"];
const HEADING_HTML: &[&str] = &["h1", "h2", "h3", "h4", "h5", "h6"];
const VOID_HTML: &[&str] = &["br", "hr", "input", "img", "col"];

pub(crate) fn prepare_html(html: &str, options: &crate::HtmlOptions, sheet: &str, image_mode: ImageMode, hooks: &mut dyn crate::FillHooks) -> PreparedHtml {
    let mut prepared = PreparedHtml { images: Vec::new(), html: String::new(), raw: Vec::new(), errors: Vec::new(), has_block: false, has_headings: false };
    if !options.ignore_css_errors {
        for sheet in std::iter::once(sheet.to_string()).chain(embedded_sheets(html)) {
            if let Err(error) = css::validate_sheet(&sheet) { prepared.errors.push(crate::FillError::new(error).with_id(crate::FillErrorId::HtmlCssInvalid)); }
        }
    }
    let mut rest = html;
    let mut renamed: Vec<(String, String)> = Vec::new();
    let embedded = embedded_sheets(html);
    let sheets: Vec<_> = std::iter::once(sheet).chain(embedded.iter().map(String::as_str)).collect();
    let mut context = css::Cascade::new(&sheets);
    while let Some(start) = rest.find('<') {
        prepared.html.push_str(&rest[..start]);
        rest = &rest[start..];
        if rest.starts_with("<!--") {
            let Some(end) = rest.find("-->") else { break };
            rest = &rest[end + 3..]; continue;
        }
        let Some((token, length)) = read_html_tag(rest, true) else { prepared.html.push('<'); rest = &rest[1..]; continue };
        let HtmlToken::Tag { is_closing, name, mut attributes } = token else { unreachable!() };
        let name = name.to_ascii_lowercase();
        if is_closing {
            context.close(&name);
            let emitted = if let Some(at) = renamed.iter().rposition(|(original, _)| original == &name) { renamed.remove(at).1 } else { name };
            prepared.html.push_str(&format!("</{emitted}>"));
            rest = &rest[length..]; continue;
        }
        context.open(&name, &attributes);
        if is_hidden(&name) {
            context.close(&name);
            let skip = find_closing_tag(&rest[length..], &name).unwrap_or(rest.len() - length);
            prepared.html.push_str(&rest[..length + skip]);
            rest = &rest[length + skip..]; continue;
        }
        let self_closing = rest[..length].ends_with("/>") || VOID_HTML.contains(&name.as_str());
        let body = if self_closing { "" } else { &rest[length..] };
        let matches = |selector: &str| context.matches_current(selector);
        let element = crate::HtmlElement::new(&name, &attributes, body, &matches);
        if options.custom_tags.contains_key(&name)
            && let Some(fragment) = hooks.html_custom_tag(&element) {
            let valid = tokenize(&fragment.xml).and_then(|tokens| crate::xml::check_balanced(&tokens));
            if let Err(error) = valid { prepared.errors.push(crate::FillError::new(error).with_id(crate::FillErrorId::MalformedXml)); }
            else {
                let tag = match fragment.kind { crate::HtmlTagKind::Inline => RAW_INLINE, crate::HtmlTagKind::Block => { prepared.has_block = true; RAW_BLOCK } };
                write_raw_marker(&mut prepared.html, tag, prepared.raw.len(), &name, &attributes);
                prepared.raw.push(fragment);
            }
            let consumed = if self_closing { 0 } else { element_inner(&name, body).1 };
            context.close(&name);
            rest = &rest[length + consumed..]; continue;
        }
        let transformed = hooks.html_style_transform(&element);
        let custom = hooks.html_customize(&element);
        let known = KNOWN_HTML.contains(&name.as_str());
        if !known && !options.ignore_unknown_tags && !options.custom_tags.contains_key(&name) && transformed.is_none() {
            prepared.errors.push(crate::FillError::new(format!("Unsupported HTML tag {name}")).with_id(crate::FillErrorId::HtmlUnknownTag));
        }
        let mut style = read_attribute(&attributes, "style").unwrap_or_default().to_string();
        if !custom.css.is_empty() { style.push(';'); style.push_str(&custom.css); }
        if !options.ignore_css_errors
            && let Err(error) = css::validate_style(&style) { prepared.errors.push(crate::FillError::new(error).with_id(crate::FillErrorId::HtmlCssInvalid)); }
        attributes.retain(|(name, _)| name != "style" && name != STYLE_ATTRIBUTE);
        if !style.is_empty() { attributes.push(("style".to_string(), style)); }
        if let Some(style) = custom.paragraph_style.or(transformed) { attributes.push((STYLE_ATTRIBUTE.to_string(), style)); }
        if matches!(name.as_str(), "img" | "svg") {
            let consumed = if name == "svg" && !self_closing { element_inner(&name, body).1 } else { 0 };
            if matches!(image_mode, ImageMode::Embed) {
                let declarations = vec![("style".into(), context.style_current(&attributes))];
                for dimension in ["width", "height"] {
                    if let Some(value) = read_style(&declarations, dimension) {
                        attributes.retain(|(name, _)| name != dimension);
                        attributes.push((dimension.to_string(), value.to_string()));
                    }
                }
                let source = if name == "svg" { rest[..length + consumed].to_string() } else { read_attribute(&attributes, "src").unwrap_or_default().to_string() };
                let slot = prepared.raw.len();
                write_raw_marker(&mut prepared.html, RAW_INLINE, slot, &name, &attributes);
                prepared.raw.push(crate::HtmlFragment::inline(""));
                prepared.images.push(images::HtmlImage { slot, source, attributes });
            }
            context.close(&name);
            rest = &rest[length + consumed..]; continue;
        }
        let emitted = if known { name.clone() } else if options.custom_tags.get(&name) == Some(&crate::HtmlTagKind::Block) { "p".into() } else { "span".into() };
        prepared.has_block |= BLOCK_HTML.contains(&emitted.as_str());
        prepared.has_headings |= HEADING_HTML.contains(&emitted.as_str());
        prepared.html.push('<'); prepared.html.push_str(&emitted);
        for (name, value) in &attributes {
            prepared.html.push(' '); prepared.html.push_str(name); prepared.html.push_str("=\""); escape(value, &mut prepared.html); prepared.html.push('"');
        }
        prepared.html.push('>');
        if self_closing {
            context.close(&name);
            if !VOID_HTML.contains(&emitted.as_str()) { prepared.html.push_str(&format!("</{emitted}>")); }
        } else { renamed.push((name, emitted)); }
        rest = &rest[length..];
    }
    prepared.html.push_str(rest);
    prepared
}

fn write_raw_marker(html: &mut String, kind: &str, index: usize, tag: &str, attributes: &[(String, String)]) {
    html.push_str(&format!("<{kind} index=\"{index}\" {RAW_TAG_ATTRIBUTE}=\"{tag}\""));
    for (name, value) in attributes {
        if name == "index" || name == RAW_TAG_ATTRIBUTE { continue; }
        html.push(' '); html.push_str(name); html.push_str("=\""); escape(value, html); html.push('"');
    }
    html.push_str("/>");
}

pub(crate) fn element_inner<'a>(name: &str, body: &'a str) -> (&'a str, usize) {
    let mut depth = 1usize;
    let mut at = 0;
    while let Some(start) = body[at..].find('<').map(|index| at + index) {
        if body[start..].starts_with("<!--") {
            let Some(end) = body[start..].find("-->") else { break };
            at = start + end + 3; continue;
        }
        let Some((HtmlToken::Tag { is_closing, name: tag, .. }, length)) = read_html_tag(&body[start..], true) else { at = start + 1; continue };
        if tag.eq_ignore_ascii_case(name) {
            if is_closing { depth -= 1; }
            else if !body[start..start + length].ends_with("/>") { depth += 1; }
            if depth == 0 { return (&body[..start], start + length); }
        }
        at = start + length;
    }
    (body, body.len())
}

pub(crate) fn plain_html(html: &str) -> String {
    read_html_tokens(html, true).into_iter().filter_map(|token| match token { HtmlToken::Text(text) => Some(decode_entities(text)), HtmlToken::Tag { name: "br", .. } => Some("\n".into()), _ => None }).collect()
}

fn embedded_sheets(html: &str) -> Vec<String> {
    let mut sheets = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        rest = &rest[start..];
        if rest.starts_with("<!--") {
            let Some(end) = rest.find("-->") else { break };
            rest = &rest[end + 3..];
            continue;
        }
        let Some((token, length)) = read_html_tag(rest, true) else { rest = &rest[1..]; continue };
        let HtmlToken::Tag { is_closing: false, name, .. } = token else { rest = &rest[length..]; continue };
        let name = name.to_ascii_lowercase();
        if !is_hidden(&name) { rest = &rest[length..]; continue; }
        let body = &rest[length..];
        let closing = format!("</{name}");
        let Some(end) = body.to_ascii_lowercase().find(&closing) else { break };
        if name == "style" { sheets.push(body[..end].to_string()); }
        rest = &body[end..];
    }
    sheets
}

fn trim_spaces(block: &mut Block) {
    for run in block.runs.iter_mut().rev() {
        if run.is_break || run.preserve_spaces || run.raw_xml.is_some() {
            break;
        }
        let Some(text) = &mut run.text else { continue };
        text.truncate(text.trim_end_matches(is_html_space).len());
        if !text.is_empty() {
            break;
        }
    }
}

fn trim_blocks(blocks: &mut [Block], spaces: SpaceEdges) {
    let last = blocks.len().saturating_sub(1);
    for (index, block) in blocks.iter_mut().enumerate() {
        if index != last || !spaces.end {
            trim_spaces(block);
        }
        block.runs.retain(|run| run.is_break || run.raw_xml.is_some() || run.text.as_deref().is_some_and(|text| !text.is_empty()));
        if let Some(rows) = &mut block.table {
            for cell in rows.iter_mut().flatten() {
                trim_blocks(&mut cell.blocks, SpaceEdges::default());
            }
        }
    }
}

/// Handles an element only extras read. Returns whether it was one.
fn read_extra_tag(reader: &mut Reader, is_closing: bool, tag: &str, attributes: &[(String, String)]) -> bool {
    let inline = match tag {
        "strong" | "b" => Some(Format { bold: Some(true), ..Format::default() }),
        "em" | "i" | "cite" | "var" => Some(Format { italic: Some(true), ..Format::default() }),
        "u" | "ins" => Some(Format { underline: Some(true), ..Format::default() }),
        "s" | "strike" | "del" => Some(Format { strike: Some(true), ..Format::default() }),
        "sup" => Some(Format { vert_align: Some("superscript"), ..Format::default() }),
        "sub" => Some(Format { vert_align: Some("subscript"), ..Format::default() }),
        "mark" => Some(Format { highlight: Some("yellow"), ..Format::default() }),
        "code" | "kbd" | "samp" | "tt" => Some(Format { font: Some(CODE_FONT.into()), highlight: Some("lightGray"), ..Format::default() }),
        "a" => Some(Format {
            link: read_attribute(attributes, "href").map(Arc::from),
            color: Some(LINK_COLOR.into()),
            underline: Some(true),
            ..Format::default()
        }),
        "span" | "font" | "small" | "big" | "abbr" | "q" => Some(Format::default()),
        _ => None,
    };
    if let Some(format) = inline {
        if is_closing {
            reader.formats.close(tag);
        } else {
            reader.formats.push(tag, format.and(&read_css(attributes)));
        }
        return true;
    }

    match tag {
        "div" | "center" | "blockquote" | "pre" | "section" | "article" | "header" | "footer" | "main" => {
            reader.current = None;
            if is_closing {
                reader.close_container(tag);
                reader.formats.close(tag);
                return true;
            }
            let align = if tag == "center" { Some("center") } else { read_style(attributes, "text-align").and_then(read_align) };
            let first = reader.blocks().len();
            let container = Container { align, quote_level: u32::from(tag == "blockquote"), is_pre: tag == "pre", layout: read_layout(attributes), first, collection: reader.collections.len() - 1 };
            reader.containers.push(tag, container);
            let mut format = read_css(attributes);
            if tag == "pre" {
                format.font = format.font.or_else(|| Some(CODE_FONT.into()));
            }
            reader.formats.push(tag, format);
            true
        }
        "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "li" => {
            // The core branch starts or ends the block; its CSS formats the text inside.
            if is_closing {
                reader.formats.close(tag);
            } else {
                reader.formats.push(tag, read_css(attributes));
            }
            false
        }
        "hr" => {
            reader.start_block(Block { is_rule: true, ..Block::default() });
            reader.current = None;
            true
        }
        "input" => {
            if read_attribute(attributes, "type").is_some_and(|kind| kind.eq_ignore_ascii_case("checkbox")) {
                let is_checked = attributes.iter().any(|(name, _)| name == "checked");
                reader.add_run(Some(if is_checked { CHECKED } else { UNCHECKED }.to_string()), false);
            }
            true
        }
        "table" => {
            reader.current = None;
            if !is_closing {
                if reader.tables.len() >= MAX_TABLE_DEPTH {
                    reader.ignored_tables += 1;
                    return true;
                }
                let depth = reader.collections.len() - 1;
                reader.tables.push((depth, Vec::new(), table::read_table(attributes)));
                return true;
            }
            if reader.ignored_tables > 0 {
                reader.ignored_tables -= 1;
                return true;
            }
            reader.close_cell();
            if let Some((_, rows, table_layout)) = reader.tables.pop() {
                reader.blocks().push(Block { table: Some(rows), table_layout: Some(table_layout), ..Block::default() });
            }
            reader.current = None;
            true
        }
        // Outside a table, as browsers do: the tag is dropped and its text kept.
        "tr" | "td" | "th" if reader.tables.is_empty() => true,
        "tr" => {
            reader.close_cell();
            if let (false, Some((_, rows, _))) = (is_closing, reader.tables.last_mut()) {
                rows.push(Vec::new());
            }
            true
        }
        "td" | "th" => {
            reader.close_cell();
            if !is_closing {
                let span = read_attribute(attributes, "colspan").and_then(|span| span.trim().parse::<usize>().ok()).unwrap_or(1).clamp(1, MAX_COLUMN_SPAN);
                let row_span = read_attribute(attributes, "rowspan").and_then(|span| span.trim().parse::<usize>().ok()).unwrap_or(1);
                if let Some((_, rows, _)) = reader.tables.last_mut() {
                    if rows.is_empty() {
                        rows.push(Vec::new());
                    }
                    rows.last_mut().expect("pushed above").push(Cell { span, row_span, blocks: Vec::new(), layout: table::read_cell(attributes) });
                }
                reader.collections.push(Vec::new());
                reader.current = None;
                let mut format = read_css(attributes);
                if tag == "th" {
                    format.bold = format.bold.or(Some(true));
                }
                reader.formats.push(tag, format);
                return true;
            }
            reader.formats.close(tag);
            true
        }
        "col" => {
            if !is_closing && let Some((_, _, layout)) = reader.tables.last_mut() { table::read_column(layout, attributes); }
            true
        }
        "thead" | "tbody" | "tfoot" | "colgroup" | "caption" | "img" | "svg" | "script" | "style" | "template" => true,
        _ => false,
    }
}

fn read_attribute<'a>(attributes: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attributes.iter().find(|(each, _)| each == name).map(|(_, value)| value.as_str())
}

/// One property of an element's `style` attribute.
fn read_style<'a>(attributes: &'a [(String, String)], property: &str) -> Option<&'a str> {
    read_attribute(attributes, "style")?.split(';').find_map(|declaration| {
        let (name, value) = declaration.split_once(':')?;
        (name.trim().eq_ignore_ascii_case(property)).then(|| value.trim())
    })
}

fn css_twips(value: &str, font_points: f64) -> Option<i64> {
    let value = value.trim();
    let (number, factor) = if let Some(value) = value.strip_suffix("px") { (value, POINTS_PER_CSS_PIXEL * TWIPS_PER_POINT) }
        else if let Some(value) = value.strip_suffix("pt") { (value, TWIPS_PER_POINT) }
        else if let Some(value) = value.strip_suffix("em") { (value, font_points * TWIPS_PER_POINT) }
        else if value == "0" { (value, 1.0) } else { return None };
    let twips = number.trim().parse::<f64>().ok()? * factor;
    (twips.is_finite() && twips.abs() <= i32::MAX as f64).then(|| twips.round() as i64)
}

fn read_layout(attributes: &[(String, String)]) -> ParagraphLayout {
    let style = |name| read_style(attributes, name);
    let font = style("font-size").and_then(read_font_size).map_or(DEFAULT_FONT_POINTS, |size| size as f64 / 2.0);
    let length = |name| style(name).and_then(|value| css_twips(value, font));
    let side = |margin, padding| match (length(margin), length(padding)) {
        (None, None) => None,
        (margin, padding) => Some(margin.unwrap_or(0) + padding.unwrap_or(0).max(0)),
    };
    let line_height = style("line-height").and_then(|value| {
        let multiple = value.strip_suffix('%').and_then(|value| value.trim().parse::<f64>().ok()).map(|value| value / 100.0)
            .or_else(|| value.parse::<f64>().ok());
        if let Some(multiple) = multiple {
            return (multiple.is_finite() && multiple > 0.0 && multiple * LINE_HEIGHT_UNIT <= i32::MAX as f64)
                .then(|| LineHeight::Multiple((multiple * LINE_HEIGHT_UNIT).round() as u32));
        }
        css_twips(value, font).filter(|value| *value > 0).map(|value| LineHeight::Exact(value as u32))
    });
    ParagraphLayout {
        left: side("margin-left", "padding-left"), right: side("margin-right", "padding-right"),
        before: side("margin-top", "padding-top"), after: side("margin-bottom", "padding-bottom"),
        text_indent: length("text-indent"), line_height,
        rtl: style("direction").and_then(|value| match value { "rtl" => Some(true), "ltr" => Some(false), _ => None }),
        page_before: style("break-before") == Some("page"), page_after: style("break-after") == Some("page"),
    }
}

fn layout_is_empty(layout: ParagraphLayout) -> bool {
    layout.left.is_none() && layout.right.is_none() && layout.before.is_none() && layout.after.is_none()
        && layout.text_indent.is_none() && layout.line_height.is_none() && layout.rtl.is_none()
        && !layout.page_before && !layout.page_after
}

fn read_property(inner: &str, name: &str) -> String {
    let Some(start) = find_element_start(inner, &format!("<{name}")) else { return format!("<{name}/>") };
    let Some(end) = inner[start..].find('>') else { return format!("<{name}/>") };
    inner[start..start + end + 1].to_string()
}

fn remove_attribute(raw: &str, name: &str) -> String {
    let needle = format!(" {name}=\"");
    let Some(start) = raw.find(&needle) else { return raw.to_string() };
    let from = start + needle.len();
    let Some(end) = raw[from..].find('"') else { return raw.to_string() };
    format!("{}{}", &raw[..start], &raw[from + end + 1..])
}

fn write_layout(mut inner: String, layout: ParagraphLayout) -> String {
    if layout.left.is_some() || layout.right.is_some() || layout.text_indent.is_some() {
        let mut ind = read_property(&inner, "w:ind");
        for (name, value) in [("w:left", layout.left), ("w:right", layout.right)] {
            if let Some(value) = value { ind = set_attribute(&ind, name, &value.to_string()); }
        }
        if let Some(value) = layout.text_indent {
            ind = remove_attribute(&remove_attribute(&ind, "w:firstLine"), "w:hanging");
            ind = set_attribute(&ind, if value < 0 { "w:hanging" } else { "w:firstLine" }, &value.unsigned_abs().to_string());
        }
        inner = set_child(&inner, "w:ind", &ind, PARAGRAPH_ORDER);
    }
    if layout.before.is_some() || layout.after.is_some() || layout.line_height.is_some() {
        let mut spacing = read_property(&inner, "w:spacing");
        for (name, value) in [("w:before", layout.before), ("w:after", layout.after)] {
            if let Some(value) = value { spacing = set_attribute(&spacing, name, &value.max(0).to_string()); }
        }
        if let Some(height) = layout.line_height {
            let (value, rule) = match height { LineHeight::Multiple(value) => (value, "auto"), LineHeight::Exact(value) => (value, "exact") };
            spacing = set_attribute(&set_attribute(&spacing, "w:line", &value.to_string()), "w:lineRule", rule);
        }
        inner = set_child(&inner, "w:spacing", &spacing, PARAGRAPH_ORDER);
    }
    if let Some(rtl) = layout.rtl { inner = set_child(&inner, "w:bidi", if rtl { "<w:bidi/>" } else { "<w:bidi w:val=\"0\"/>" }, PARAGRAPH_ORDER); }
    if layout.page_before { inner = set_child(&inner, "w:pageBreakBefore", "<w:pageBreakBefore/>", PARAGRAPH_ORDER); }
    inner
}

fn read_align(align: &str) -> Option<&'static str> {
    match align.trim() {
        "left" | "start" => Some("left"),
        "center" => Some("center"),
        "right" | "end" => Some("right"),
        "justify" => Some("both"),
        _ => None,
    }
}

/// The run format an element's `style` attribute asks for.
fn read_css(attributes: &[(String, String)]) -> Format {
    let style = |property: &str| read_style(attributes, property);
    let decoration = style("text-decoration").unwrap_or_default();
    Format {
        bold: style("font-weight").and_then(|weight| match weight {
            "bold" | "bolder" => Some(true),
            "normal" | "lighter" => Some(false),
            number => number.parse::<u32>().ok().map(|weight| weight >= 600),
        }),
        italic: style("font-style").map(|font_style| font_style == "italic" || font_style == "oblique"),
        underline: decoration.contains("underline").then_some(true).or_else(|| (decoration == "none").then_some(false)),
        strike: decoration.contains("line-through").then_some(true).or_else(|| (decoration == "none").then_some(false)),
        color: style("color").and_then(read_color).map(Arc::from),
        shade: style("background-color").or_else(|| style("background")).and_then(read_color).map(Arc::from),
        size: style("font-size").and_then(read_font_size),
        font: style("font-family").map(|family| family.split(',').next().unwrap_or(family).trim().trim_matches(['"', '\''])).filter(|family| !family.is_empty()).map(Arc::from),
        pre_spaces: style("white-space").and_then(|value| match value { "pre" | "pre-wrap" | "break-spaces" => Some(true), "normal" | "nowrap" => Some(false), _ => None }),
        paragraph_style: read_attribute(attributes, STYLE_ATTRIBUTE).map(Arc::from),
        rtl: style("direction").and_then(|value| match value { "rtl" => Some(true), "ltr" => Some(false), _ => None }),
        ..Format::default()
    }
}

/// A CSS colour as six hex digits: `#abc`, `#aabbcc`, `rgb(r, g, b)` or a basic name.
fn read_color(color: &str) -> Option<String> {
    let color = color.trim().to_ascii_lowercase();
    if let Some(hex) = color.strip_prefix('#') {
        if !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        return match hex.len() {
            3 => Some(hex.chars().flat_map(|digit| [digit, digit]).collect::<String>().to_ascii_uppercase()),
            6 => Some(hex.to_ascii_uppercase()),
            _ => None,
        };
    }
    if let Some(channels) = color.strip_prefix("rgb(").or_else(|| color.strip_prefix("rgba(")).and_then(|rest| rest.strip_suffix(')')) {
        let values: Vec<u8> = channels.split(',').take(3).filter_map(|channel| channel.trim().parse::<f64>().ok()).map(|value| value.clamp(0.0, 255.0).round() as u8).collect();
        return (values.len() == 3).then(|| format!("{:02X}{:02X}{:02X}", values[0], values[1], values[2]));
    }
    let named = match color.as_str() {
        "black" => "000000",
        "white" => "FFFFFF",
        "red" => "FF0000",
        "green" => "008000",
        "blue" => "0000FF",
        "yellow" => "FFFF00",
        "orange" => "FFA500",
        "purple" => "800080",
        "gray" | "grey" => "808080",
        "silver" => "C0C0C0",
        "navy" => "000080",
        "teal" => "008080",
        "maroon" => "800000",
        _ => return None,
    };
    Some(named.to_string())
}

/// A CSS font size in px or pt as Word half-points. Pixels are 96 to the inch.
fn read_font_size(size: &str) -> Option<u32> {
    let size = size.trim();
    let (number, points_per_unit) = if let Some(px) = size.strip_suffix("px") {
        (px, 0.75)
    } else {
        (size.strip_suffix("pt")?, 1.0)
    };
    let points = number.trim().parse::<f64>().ok()? * points_per_unit;
    (points > 0.0).then(|| (points * 2.0).round() as u32)
}

enum HtmlToken<'a> {
    Tag { is_closing: bool, name: &'a str, attributes: Vec<(String, String)> },
    Text(&'a str),
}

/// `<name>`, `</name>`, `<name/>` and the text between. A `<` starting anything else is skipped.
/// With extras, start tags may carry attributes, and comments are dropped.
fn read_html_tokens(html: &str, has_extras: bool) -> Vec<HtmlToken<'_>> {
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < html.len() {
        let rest = &html[at..];
        if !rest.starts_with('<') {
            let end = rest.find('<').unwrap_or(rest.len());
            tokens.push(HtmlToken::Text(&rest[..end]));
            at += end;
            continue;
        }
        if has_extras && rest.starts_with("<!--") {
            at += rest.find("-->").map_or(rest.len(), |end| end + 3);
            continue;
        }
        match read_html_tag(rest, has_extras) {
            Some((token @ HtmlToken::Tag { is_closing: false, name, .. }, length)) if has_extras && is_hidden(name) => {
                tokens.push(token);
                tokens.push(HtmlToken::Tag { is_closing: true, name, attributes: Vec::new() });
                at += length + find_closing_tag(&rest[length..], name).unwrap_or(rest.len() - length);
            }
            Some((token, length)) => {
                tokens.push(token);
                at += length;
            }
            None => {
                tokens.push(HtmlToken::Text(&rest[..1]));
                at += 1;
            }
        }
    }
    tokens
}

fn is_hidden(name: &str) -> bool {
    HIDDEN_ELEMENTS.iter().any(|hidden| hidden.eq_ignore_ascii_case(name))
}

/// The bytes up to and including `</name>`, matched without case.
fn find_closing_tag(html: &str, name: &str) -> Option<usize> {
    let closing = format!("</{}", name.to_ascii_lowercase());
    let start = html.to_ascii_lowercase().find(&closing)?;
    Some(start + html[start..].find('>')? + 1)
}

fn read_html_tag(text: &str, has_extras: bool) -> Option<(HtmlToken<'_>, usize)> {
    let is_closing = text[1..].starts_with('/');
    let name_start = if is_closing { 2 } else { 1 };
    let name_length = text[name_start..].bytes().take_while(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_')).count();
    if name_length == 0 {
        return None;
    }
    let name = &text[name_start..name_start + name_length];
    let mut at = name_start + name_length;
    let mut attributes = Vec::new();
    // Read recognized tags completely; extras control how their attributes render.
    let known = KNOWN_HTML.iter().any(|known| known.eq_ignore_ascii_case(name));
    if !is_closing && (has_extras || known || name.starts_with("docx-fill-")) {
        at += read_attributes(&text[at..], &mut attributes)?;
    }
    at += text[at..].bytes().take_while(u8::is_ascii_whitespace).count();
    if text[at..].starts_with('/') {
        at += 1;
    }
    if !text[at..].starts_with('>') {
        return None;
    }
    Some((HtmlToken::Tag { is_closing, name, attributes }, at + 1))
}

/// Reads `name="value"`, `name='value'`, `name=value` and bare `name` attributes up to the tag's
/// end. Returns the bytes read; `None` for a tag that never ends.
fn read_attributes(text: &str, attributes: &mut Vec<(String, String)>) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut at = 0;
    loop {
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        if at >= bytes.len() {
            return None;
        }
        if bytes[at] == b'>' || bytes[at] == b'/' {
            return Some(at);
        }
        let name_start = at;
        while at < bytes.len() && !bytes[at].is_ascii_whitespace() && !matches!(bytes[at], b'=' | b'>' | b'/') {
            at += 1;
        }
        let name = text[name_start..at].to_ascii_lowercase();
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        if at >= bytes.len() || bytes[at] != b'=' {
            attributes.push((name, String::new()));
            continue;
        }
        at += 1;
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        let value = match bytes.get(at) {
            Some(&quote @ (b'"' | b'\'')) => {
                let end = at + 1 + text[at + 1..].find(quote as char)?;
                let value = &text[at + 1..end];
                at = end + 1;
                value
            }
            Some(_) => {
                let start = at;
                while at < bytes.len() && !bytes[at].is_ascii_whitespace() && bytes[at] != b'>' {
                    at += 1;
                }
                &text[start..at]
            }
            None => return None,
        };
        attributes.push((name, decode_entities(value)));
    }
}

fn write_paragraph_props(paragraph_props: &str, block: &Block, number: Option<u32>, run_props: &str) -> String {
    let mut props = match &block.list {
        Some(item) => {
            let level = if number.is_some() { item.level.min(MAX_LIST_LEVEL) } else { item.level };
            let indent = format!("<w:ind w:left=\"{}\" w:hanging=\"{LIST_HANGING_TWIPS}\"/>", LIST_INDENT_TWIPS * (level as u32 + 1));
            let inner = paragraph_props.strip_prefix("<w:pPr>").and_then(|props| props.strip_suffix("</w:pPr>")).unwrap_or("");
            let numbering = number.map(|id| format!("<w:numPr><w:ilvl w:val=\"{level}\"/><w:numId w:val=\"{id}\"/></w:numPr>")).unwrap_or_default();
            let mut inner = set_child(&set_child(inner, "w:numPr", &numbering, PARAGRAPH_ORDER), "w:ind", &indent, PARAGRAPH_ORDER);
            // Native markers use the paragraph mark's formatting, separate from the text runs.
            if number.is_some() && !run_props.is_empty() {
                inner = set_child(&inner, "w:rPr", run_props, PARAGRAPH_ORDER);
            }
            format!("<w:pPr>{inner}</w:pPr>")
        }
        None => paragraph_props.to_string(),
    };
    if block.align.is_none() && block.quote_level == 0 && !block.is_rule && layout_is_empty(block.layout) && block.paragraph_style.is_none() {
        return props;
    }

    let mut inner = props.strip_prefix("<w:pPr>").and_then(|props| props.strip_suffix("</w:pPr>")).unwrap_or("").to_string();
    if block.heading_level.is_some() && block.paragraph_style.is_some() { inner = headings::paragraph_props(&inner); }
    if let Some(style) = &block.paragraph_style {
        let mut escaped = String::new(); escape(style, &mut escaped);
        inner = set_child(&inner, "w:pStyle", &format!("<w:pStyle w:val=\"{escaped}\"/>"), PARAGRAPH_ORDER);
    }
    if block.is_rule {
        inner = set_child(&inner, "w:pBdr", "<w:pBdr><w:bottom w:val=\"single\" w:sz=\"6\" w:space=\"1\" w:color=\"auto\"/></w:pBdr>", PARAGRAPH_ORDER);
    }
    if block.quote_level > 0 && block.list.is_none() {
        inner = set_child(&inner, "w:ind", &format!("<w:ind w:left=\"{}\"/>", QUOTE_INDENT_TWIPS * block.quote_level), PARAGRAPH_ORDER);
    }
    if let Some(align) = block.align {
        inner = set_child(&inner, "w:jc", &format!("<w:jc w:val=\"{align}\"/>"), PARAGRAPH_ORDER);
    }
    inner = write_layout(inner, block.layout);
    props = format!("<w:pPr>{inner}</w:pPr>");
    props
}

fn write_runs(block: &Block, run_props: &str, links: &mut Vec<String>, number: Option<u32>) -> String {
    let heading_props = (block.heading_level.is_some() && block.paragraph_style.is_some()).then(|| headings::inherited_props(run_props));
    let run_props = heading_props.as_deref().unwrap_or(run_props);
    let size = block.heading_level.filter(|_| block.paragraph_style.is_none()).map(|level| HEADING_SIZES[level - 1]);
    let mut out = match &block.list {
        Some(item) if number.is_none() => format!("<w:r>{run_props}<w:t>{}</w:t></w:r><w:r>{run_props}<w:tab/></w:r>", item.format.marker(item.value)),
        _ => String::new(),
    };
    let mut open_link: Option<&str> = None;
    for run in &block.runs {
        let link = run.format.link.as_deref();
        if link != open_link {
            if open_link.is_some() {
                out.push_str("</w:hyperlink>");
            }
            if let Some(link) = link {
                out.push_str(&write_link_start(link, links));
            }
            open_link = link;
        }
        if let Some(raw) = &run.raw_xml { out.push_str(raw); continue; }
        let props = write_run_props(run_props, run.is_bold || (block.heading_level.is_some() && block.paragraph_style.is_none()), run.is_italic, run.is_underlined, size, &run.format);
        out.push_str("<w:r>");
        out.push_str(&props);
        if run.is_break {
            out.push_str("<w:br/>");
        } else {
            out.push_str("<w:t xml:space=\"preserve\">");
            escape_text(run.text.as_deref().unwrap_or(""), &mut out);
            out.push_str("</w:t>");
        }
        out.push_str("</w:r>");
    }
    if open_link.is_some() {
        out.push_str("</w:hyperlink>");
    }
    out
}

/// A hyperlink's start: to a bookmark for `#name`, else by a relationship to the address.
fn write_link_start(link: &str, links: &mut Vec<String>) -> String {
    let mut escaped = String::new();
    if let Some(anchor) = link.strip_prefix('#') {
        escape(anchor, &mut escaped);
        return format!("<w:hyperlink w:anchor=\"{escaped}\" w:history=\"1\">");
    }
    let index = links.iter().position(|each| each == link).unwrap_or_else(|| {
        links.push(link.to_string());
        links.len() - 1
    });
    format!("<w:hyperlink r:id=\"{}\" w:history=\"1\">", link_relationship_id(index))
}

/// The relationship id of a part's `index`th link.
pub(crate) fn link_relationship_id(index: usize) -> String {
    format!("rIdDocxFillLink{}", index + 1)
}

fn write_run_props(run_props: &str, is_bold: bool, is_italic: bool, is_underlined: bool, size: Option<u32>, format: &Format) -> String {
    let mut inner = run_props.strip_prefix("<w:rPr>").and_then(|props| props.strip_suffix("</w:rPr>")).unwrap_or("").to_string();
    if let Some(size) = size {
        inner = set_child(&inner, "w:sz", &format!("<w:sz w:val=\"{size}\"/>"), RUN_ORDER);
        inner = set_child(&inner, "w:szCs", &format!("<w:szCs w:val=\"{size}\"/>"), RUN_ORDER);
    }
    if is_bold {
        inner = set_child(&inner, "w:b", "<w:b/>", RUN_ORDER);
        inner = set_child(&inner, "w:bCs", "<w:bCs/>", RUN_ORDER);
    }
    if is_italic {
        inner = set_child(&inner, "w:i", "<w:i/>", RUN_ORDER);
        inner = set_child(&inner, "w:iCs", "<w:iCs/>", RUN_ORDER);
    }
    if is_underlined && !inner.contains("<w:u ") {
        inner = set_child(&inner, "w:u", "<w:u w:val=\"single\"/>", RUN_ORDER);
    }
    if *format != Format::default() {
        inner = write_format(&inner, format);
    }
    if inner.is_empty() { String::new() } else { format!("<w:rPr>{inner}</w:rPr>") }
}

/// Run properties with an extras format set, in schema order.
fn write_format(inner: &str, format: &Format) -> String {
    let mut inner = inner.to_string();
    let toggle = |is_on: bool, name: &str| if is_on { format!("<{name}/>") } else { format!("<{name} w:val=\"0\"/>") };
    if let Some(rtl) = format.rtl { inner = set_child(&inner, "w:rtl", &toggle(rtl, "w:rtl"), RUN_ORDER); }
    if let Some(font) = &format.font {
        let mut family = String::new();
        escape(font, &mut family);
        inner = set_child(&inner, "w:rFonts", &format!("<w:rFonts w:ascii=\"{family}\" w:hAnsi=\"{family}\" w:cs=\"{family}\"/>"), RUN_ORDER);
    }
    if let Some(bold) = format.bold {
        inner = set_child(&inner, "w:b", &toggle(bold, "w:b"), RUN_ORDER);
        inner = set_child(&inner, "w:bCs", &toggle(bold, "w:bCs"), RUN_ORDER);
    }
    if let Some(italic) = format.italic {
        inner = set_child(&inner, "w:i", &toggle(italic, "w:i"), RUN_ORDER);
        inner = set_child(&inner, "w:iCs", &toggle(italic, "w:iCs"), RUN_ORDER);
    }
    if let Some(strike) = format.strike {
        inner = set_child(&inner, "w:strike", &toggle(strike, "w:strike"), RUN_ORDER);
    }
    if let Some(color) = &format.color {
        inner = set_child(&inner, "w:color", &format!("<w:color w:val=\"{color}\"/>"), RUN_ORDER);
    }
    if let Some(size) = format.size {
        inner = set_child(&inner, "w:sz", &format!("<w:sz w:val=\"{size}\"/>"), RUN_ORDER);
        inner = set_child(&inner, "w:szCs", &format!("<w:szCs w:val=\"{size}\"/>"), RUN_ORDER);
    }
    if let Some(highlight) = format.highlight {
        inner = set_child(&inner, "w:highlight", &format!("<w:highlight w:val=\"{highlight}\"/>"), RUN_ORDER);
    }
    if let Some(underline) = format.underline {
        let value = if underline { "single" } else { "none" };
        inner = set_child(&inner, "w:u", &format!("<w:u w:val=\"{value}\"/>"), RUN_ORDER);
    }
    if let Some(shade) = &format.shade {
        inner = set_child(&inner, "w:shd", &format!("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{shade}\"/>"), RUN_ORDER);
    }
    if let Some(vert_align) = format.vert_align {
        inner = set_child(&inner, "w:vertAlign", &format!("<w:vertAlign w:val=\"{vert_align}\"/>"), RUN_ORDER);
    }
    inner
}

/// HTML text escaped for `<w:t>`: `&`, `<` and `>` only, without characters XML cannot hold.
fn escape_text(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ if crate::xml::is_xml_char(c) => out.push(c),
            _ => {}
        }
    }
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let name_length = rest[1..].bytes().take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'#').count();
        let decoded = if rest[1 + name_length..].starts_with(';') { decode_entity(&rest[1..1 + name_length]) } else { None };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[name_length + 2..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn decode_entity(name: &str) -> Option<char> {
    if let Some(hex) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
        return u32::from_str_radix(hex, 16).ok().and_then(char::from_u32);
    }
    if let Some(decimal) = name.strip_prefix('#') {
        return decimal.parse().ok().and_then(char::from_u32);
    }
    match name.to_ascii_lowercase().as_str() {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some('\u{A0}'),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_inherited_strings() {
        let text: Arc<str> = "x".repeat(16_384).into();
        let mut styles = StyleStack::new(Format::and);
        styles.push("span", Format { font: Some(text.clone()), link: Some(text.clone()), paragraph_style: Some(text.clone()), ..Format::default() });
        for _ in 0..2_000 { styles.push("span", Format::default()); }

        for element in &styles.elements {
            for value in [&element.inherited.font, &element.inherited.link, &element.inherited.paragraph_style] {
                assert!(Arc::ptr_eq(value.as_ref().unwrap(), &text));
            }
        }
    }

    #[test]
    fn decodes_entities_and_leaves_unknown_ones() {
        assert_eq!(decode_entities("a &amp; b &#65;&#x42; &bogus; &"), "a & b AB &bogus; &");
    }

    #[test]
    fn ignores_core_html_attributes() {
        let blocks = read_html("<p class=\"x\">t</p>", false, SpaceEdges::default(), "", &[]);
        assert_eq!(blocks[0].runs[0].text.as_deref(), Some("t"));
    }

    #[test]
    fn numbers_list_items_in_letters_and_roman_numerals() {
        assert_eq!([1, 26, 27, 52].map(write_letters), ["A", "Z", "AA", "AZ"]);
        assert_eq!([1, 4, 9, 14, 1994].map(write_roman), ["I", "IV", "IX", "XIV", "MCMXCIV"]);
    }
}
