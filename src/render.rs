//! Fills a parsed template with tag values.

use std::borrow::Cow;

use serde_json::Value;

use crate::properties::edit_each;
use crate::styling::{RANGE_END, RANGE_START, RANGE_START_END, Style, StyleKind, read_style, style_cell, style_paragraph, style_ranges, style_row, style_run, style_shape};
use crate::table_merge::{Merge, merge_table};
use crate::full_table::{Grid, write_table};
use crate::images::{Image, MediaUse, body_or_default, drawing_size, drawing_xml};
use crate::images::replacement::{PictureFrame, PictureReplacement, replace_picture};
use crate::rich_text::{self, RichValue};
use crate::template::{Block, BlockKind, LoopPageBreaks, Part, PictureTag, StyleTarget, TagKind, Template};
use crate::cell_styling;
use crate::expressions::{Assignments, Computed, ComputedEnd, Evaluation};
use crate::comments::{Comment, read_commented};
use crate::includes::{INCLUDE_END, INCLUDE_START};
use crate::sections::{SECTION_END, SECTION_START, SectionUse};
use crate::structure::repair_structure;
use crate::xml::{ElementKind, escape, read_attribute};

// A `\n` in a value outside a run: the text continues after a line break.
const LINE_BREAK: &str = "</w:t><w:br/><w:t xml:space=\"preserve\">";
const PARAGRAPH: &str = "w:p";
const CELL: &str = "w:tc";
const ROW: &str = "w:tr";
const TABLE: &str = "w:tbl";
const RUN: &str = "w:r";
const PAGE_BREAK_RUN: &str = "<w:r><w:br w:type=\"page\"/></w:r>";
// Shapes `{:styleshape}` colours: DrawingML and the VML Word writes as a fallback.
const SHAPES: [&str; 5] = ["wps:wsp", "v:shape", "v:rect", "v:roundrect", "v:oval"];
// Segments including themselves stop here.
const MAX_SEGMENT_DEPTH: usize = 32;
const SUPERSCRIPT: &str = "<w:rPr><w:vertAlign w:val=\"superscript\"/></w:rPr>";

/// A tag a module claims, by its prefix in `FillOptions::module_prefixes`.
pub struct ModuleTag<'a> {
    pub module: usize,
    /// The tag after its prefix.
    pub name: &'a str,
    /// The name's value, looked up as a plain tag's is.
    pub value: Option<&'a Value>,
}

/// A value or raw tag whose value is missing or null.
pub struct MissingValue<'a> {
    pub kind: MissingKind,
    /// The name looked up, or the expression with expressions on.
    pub name: &'a str,
    /// The tag as written between its delimiters, such as `name` or `@xml`.
    pub written: &'a str,
}

/// Which kind of tag a [`MissingValue`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingKind {
    /// `{name}`: the hook's text prints escaped.
    Value,
    /// `{@name}` or `{r@name}`: the hook's text prints as raw XML.
    Raw,
}

/// What a fill asks of its caller.
pub trait FillHooks {
    fn html_custom_tag(&mut self, _element: &crate::HtmlElement<'_>) -> Option<crate::HtmlFragment> { None }
    fn html_customize(&mut self, _element: &crate::HtmlElement<'_>) -> crate::HtmlCustomization { crate::HtmlCustomization::default() }
    fn html_style_transform(&mut self, _element: &crate::HtmlElement<'_>) -> Option<String> { None }
    /// Escaped XML text printed for a module tag in `part`. Prints nothing by default.
    fn module_tag(&mut self, _part: &str, _tag: ModuleTag) -> String {
        String::new()
    }

    /// The image an image tag in `part` shows for `value`, which is never missing, null, false,
    /// zero or empty text. `None` prints nothing, the default.
    fn image(&mut self, _part: &str, _value: &Value) -> Option<Image> {
        None
    }

    /// Exact display dimensions for an image. `None` keeps container fitting.
    /// Called after `image`; the same media can have different display sizes.
    fn image_size(&mut self, _part: &str, _value: &Value, _image: &Image) -> Option<crate::ImageSize> {
        None
    }

    /// A PNG preview for an SVG image, used by readers without SVG support.
    /// The `svg-preview` feature rasterizes by default. An override can return `None` to keep SVG only.
    fn svg_fallback(&mut self, _part: &str, _value: &Value, _image: &Image) -> Option<std::sync::Arc<[u8]>> {
        #[cfg(feature = "svg-preview")]
        { crate::images::svg::rasterize(_image, &self.svg_fonts(_part, _value)) }
        #[cfg(not(feature = "svg-preview"))]
        { None }
    }

    /// Additional font files for automatic SVG previews. Noto Sans is included by default.
    /// Font data stays in memory; the rasterizer does not read fonts or images from disk.
    fn svg_fonts(&mut self, _part: &str, _value: &Value) -> Vec<std::sync::Arc<[u8]>> { Vec::new() }

    fn image_properties(&mut self, _part: &str, _value: &Value, _image: &Image) -> crate::ImageProperties {
        crate::ImageProperties::default()
    }

    /// The result of `{input | name:arguments}`, with a filter named in `FillOptions::filters`. A
    /// missing input or argument is null. Returns the input by default.
    fn filter(&mut self, _name: &str, input: &Value, _arguments: &[Value]) -> Value {
        input.clone()
    }

    /// The value of a tag read in the caller's own syntax, as docxtemplater's `parser` gives.
    /// `name` is the tag after its prefix, as written. `scopes` are the data and then each loop's
    /// item, innermost last. A null is a missing value. `None`, the default, reads the tag as usual.
    fn tag_value(&mut self, _name: &str, _scopes: &[&Value]) -> Option<Value> {
        None
    }

    /// The text a value or raw tag in `part` prints when its value is missing or null, as
    /// docxtemplater's `nullGetter` gives. `None` prints nothing, the default.
    fn missing_value(&mut self, _part: &str, _tag: MissingValue) -> Option<String> {
        None
    }
}

/// Hooks that print nothing for module and image tags.
pub struct NoHooks;

impl FillHooks for NoHooks {}

/// Shared by the parts of one package fill.
pub(crate) struct FillState {
    pub assignments: Assignments,
    pub next_drawing_id: u32,
    pub next_caption: u32,
    /// Page width and height less margins, in pixels, when the part does not give its own.
    pub body: Option<(f64, f64)>,
    /// Images `{%name}` sizes to fill, as a share of their container's width.
    pub container_fill: f64,
    pub next_footnote_id: u32,
    pub next_comment_id: u32,
    pub next_bookmark_id: u32,
    /// The package's main document part, whose body takes sections, footnotes and comments.
    pub main_part: String,
    pub numbering: Option<rich_text::numbering::Numberings>,
}

/// A filled part and the media its drawings show.
pub(crate) struct FilledPart {
    pub xml: String,
    pub charts: Vec<crate::charts::ChartUse>,
    pub html_errors: Vec<crate::FillError>,
    pub media: Vec<MediaUse>,
    /// Footnote ids and their text, referenced from the part.
    pub footnotes: Vec<(u32, String)>,
    /// External link addresses, by relationship index.
    pub links: Vec<String>,
    /// Style definitions to change: their kind, id (`None` for the default) and style.
    pub style_definitions: Vec<(StyleKind, Option<String>, Style)>,
    /// Subtemplates to print at the include marks, by mark index: the name and the scope.
    pub includes: Vec<(String, Value)>,
    pub comments: Vec<Comment>,
    /// Section tags to apply at the section marks, by mark index.
    pub sections: Vec<SectionUse>,
}

/// A paragraph open in the output.
struct ParagraphFrame {
    start: usize,
    has_rich_text: bool,
    style: Option<Style>,
    /// Held a `{::stylepar}` tag, with such paragraphs dropped.
    is_dropped: bool,
    captions: Vec<String>,
}

struct Render<'a, 'h> {
    template: &'a Template,
    part: &'a str,
    hooks: &'h mut dyn FillHooks,
    state: &'h mut FillState,
    out: String,
    paragraphs: Vec<ParagraphFrame>,
    /// Output offsets of the elements open now, with the style or merges each was given.
    cells: Vec<(usize, Option<Style>)>,
    rows: Vec<(usize, Option<Style>)>,
    runs: Vec<(usize, Option<Style>)>,
    tables: Vec<(usize, Vec<Merge>)>,
    shapes: Vec<(usize, Option<Style>)>,
    /// `{:stylestart}` ranges' styles, by the index their marks carry.
    ranges: Vec<Style>,
    style_definitions: Vec<(StyleKind, Option<String>, Style)>,
    includes: Vec<(String, Value)>,
    comments: Vec<Comment>,
    sections: Vec<SectionUse>,
    segment_depth: usize,
    rich_values: Vec<RichValue>,
    html_errors: Vec<crate::FillError>,
    media: Vec<MediaUse>,
    footnotes: Vec<(u32, String)>,
    links: Vec<String>,
    /// Expressions' results, which live as long as the data does.
    computed: ComputedEnd<'a>,
    /// Each array loop item's index, innermost last.
    loop_indexes: Vec<usize>,
    pictures: Vec<PictureFrame>,
    charts: Vec<crate::charts::ChartUse>,
    /// Each open placeholder block's names and values, innermost last, so its tags print them
    /// without evaluating assignments again.
    placeholders: Vec<(&'a [String], Vec<Option<&'a Value>>)>,
}

impl Template {
    /// The part filled with `data`. Module and image tags print nothing.
    pub fn render(&self, data: &Value) -> String {
        self.render_with(data, &mut NoHooks)
    }

    /// The part filled with `data`, asking `hooks` for module tags and images. An image's media
    /// part and relationship are not added: fill a [`crate::PackageTemplate`] for that.
    pub fn render_with(&self, data: &Value, hooks: &mut dyn FillHooks) -> String {
        self.render_result(data, hooks).0
    }

    /// Renders once and returns HTML validation errors instead of partial output.
    pub fn try_render(&self, data: &Value) -> Result<String, crate::FillError> { self.try_render_with(data, &mut NoHooks) }

    /// Checked rendering with callbacks, each invoked once per element.
    pub fn try_render_with(&self, data: &Value, hooks: &mut dyn FillHooks) -> Result<String, crate::FillError> {
        let (xml, errors) = self.render_result(data, hooks);
        crate::check_html_errors(errors)?;
        Ok(xml)
    }

    fn render_result(&self, data: &Value, hooks: &mut dyn FillHooks) -> (String, Vec<crate::FillError>) {
        let mut state = FillState { assignments: Assignments::default(), next_drawing_id: self.max_drawing_id.wrapping_add(1), next_caption: 1, body: None, container_fill: crate::CONTAINER_FILL, next_footnote_id: 1, next_comment_id: 0, next_bookmark_id: self.max_bookmark_id + 1, main_part: crate::DOCUMENT_PART.to_string(), numbering: None };
        let filled = self.fill(data, "", hooks, &mut state);
        let errors = filled.html_errors;
        // Only a package prints subtemplates.
        let xml = if filled.includes.is_empty() {
            filled.xml
        } else {
            let mut xml = String::with_capacity(filled.xml.len());
            let mut is_in_mark = false;
            for c in filled.xml.chars() {
                match c {
                    INCLUDE_START => is_in_mark = true,
                    INCLUDE_END => is_in_mark = false,
                    _ if !is_in_mark => xml.push(c),
                    _ => {}
                }
            }
            xml
        };
        let xml = crate::unique_ids::make_ids_unique(&xml, self.continued_bookmarks.as_deref()).unwrap_or(xml);
        (repair_structure(&xml).unwrap_or(xml), errors)
    }

    pub(crate) fn fill(&self, data: &Value, part: &str, hooks: &mut dyn FillHooks, state: &mut FillState) -> FilledPart {
        let computed = Computed::default();
        let mut render = Render {
            template: self,
            part,
            hooks,
            state,
            out: String::with_capacity(self.parts.len() * 16),
            paragraphs: Vec::new(),
            cells: Vec::new(),
            rows: Vec::new(),
            runs: Vec::new(),
            tables: Vec::new(),
            shapes: Vec::new(),
            ranges: Vec::new(),
            style_definitions: Vec::new(),
            includes: Vec::new(),
            comments: Vec::new(),
            sections: Vec::new(),
            segment_depth: 0,
            rich_values: Vec::new(),
            html_errors: Vec::new(),
            media: Vec::new(),
            footnotes: Vec::new(),
            links: Vec::new(),
            computed: computed.end(),
            loop_indexes: Vec::new(),
            pictures: Vec::new(),
            charts: Vec::new(),
            placeholders: Vec::new(),
        };
        let mut scopes = vec![data];
        render.render_range(0, self.parts.len(), &self.top, &mut scopes);
        let xml = if render.ranges.is_empty() { render.out } else { style_ranges(&render.out, &render.ranges) };
        let xml = if self.soft_placeholders { crate::placeholder_lines::collapse(&xml) } else { xml };
        let xml = if self.has_cell_loops { crate::table_grid::fit_table_grids(&xml, self.table_resize).unwrap_or(xml) } else { xml };
        let filled = FilledPart { xml, charts: render.charts, html_errors: render.html_errors, media: render.media, footnotes: render.footnotes, links: render.links, style_definitions: render.style_definitions, includes: render.includes, comments: render.comments, sections: render.sections };
        state.assignments.forget(&computed);
        filled
    }
}

impl<'a> Render<'a, '_> {
    fn current_scope(&self, scopes: &[&Value]) -> Value {
        let updated: Vec<Value> = scopes.iter().map(|scope| self.state.assignments.scope(scope)).collect();
        merge_scopes(&updated.iter().collect::<Vec<_>>())
    }

    /// The value a tag's name gives: the hook's if it reads the tag, else its expression's result
    /// with expressions on, else the name's value in the innermost scope that has one.
    fn look_up(&mut self, scopes: &[&'a Value], name: &str) -> Option<&'a Value> {
        if let Some(value) = self.hooks.tag_value(name, scopes) {
            return if value.is_null() { None } else { Some(self.computed.keep(value)) };
        }
        let template = self.template;
        let Some(expression) = template.expressions.get(name) else { return look_up(scopes, name) };
        let mut evaluation = Evaluation { scopes, loop_indexes: &self.loop_indexes, hooks: &mut *self.hooks, assignments: &mut self.state.assignments };
        match evaluation.evaluate(expression)? {
            Cow::Borrowed(value) => Some(value),
            Cow::Owned(value) => Some(self.computed.keep(value)),
        }
    }

    /// A placeholder tag's value, as its block read it.
    fn placeholder_value(&self, kind: &TagKind, name: &str) -> Option<Option<&'a Value>> {
        if !matches!(kind, TagKind::Placeholder) { return None; }
        let (names, values) = self.placeholders.last()?;
        names.iter().position(|each| each == name).map(|index| values[index])
    }

    fn render_range(&mut self, from: usize, to: usize, blocks: &[usize], scopes: &mut Vec<&'a Value>) {
        let template = self.template;
        let mut at = from;
        for &index in blocks {
            let block = &template.blocks[index];
            if block.start < at || block.end > to {
                continue;
            }
            self.render_parts(at, block.start, scopes);
            at = block.end;

            match &block.kind {
                // Only a string is XML; anything else prints nothing.
                BlockKind::Raw { name, written } => match self.look_up(scopes, name) {
                    Some(Value::String(xml)) => self.out.push_str(xml),
                    None | Some(Value::Null) => {
                        let missing = MissingValue { kind: MissingKind::Raw, name, written };
                        if let Some(xml) = self.hooks.missing_value(self.part, missing) {
                            self.out.push_str(&xml);
                        }
                    }
                    Some(_) => {}
                },
                BlockKind::Segment { .. } => {}
                BlockKind::Section { name, tag } => {
                    // Sections belong to the body; elsewhere the paragraph goes.
                    let Some(value) = self.look_up(scopes, name).filter(|_| self.part == self.state.main_part) else { continue };
                    self.out.push(SECTION_START);
                    self.out.push_str(&self.sections.len().to_string());
                    self.out.push(SECTION_END);
                    self.sections.push(SectionUse { tag: *tag, value: value.clone(), scope: self.current_scope(scopes) });
                }
                BlockKind::IncludeSegment { name, is_dynamic } => {
                    let wanted = if *is_dynamic { self.look_up(scopes, name).and_then(Value::as_str) } else { Some(name.as_str()) };
                    let segment = wanted.and_then(|wanted| template.blocks.iter().find(|each| matches!(&each.kind, BlockKind::Segment { name } if name == wanted)));
                    let Some(segment) = segment.filter(|_| self.segment_depth < MAX_SEGMENT_DEPTH) else { continue };
                    self.segment_depth += 1;
                    self.render_range(segment.body_start, segment.body_end, &segment.children, scopes);
                    self.segment_depth -= 1;
                }
                BlockKind::Include { name } => {
                    let names: Vec<&str> = match self.look_up(scopes, name) {
                        Some(Value::String(name)) => vec![name.as_str()],
                        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect(),
                        _ => Vec::new(),
                    };
                    if names.is_empty() {
                        continue;
                    }
                    let scope = self.current_scope(scopes);
                    for name in names {
                        self.out.push(INCLUDE_START);
                        self.out.push_str(&self.includes.len().to_string());
                        self.out.push(INCLUDE_END);
                        self.includes.push((name.to_string(), scope.clone()));
                    }
                }
                BlockKind::Chart { name } => {
                    let value = self.look_up(scopes, name);
                    crate::charts::queue(&self.out, &mut self.charts, value);
                }
                BlockKind::FullTable { name, is_in_cell } => {
                    if let Some(value) = self.look_up(scopes, name) {
                        let body = body_or_default(self.template.body.or(self.state.body));
                        self.out.push_str(&write_table(value, body, *is_in_cell));
                    }
                }
                BlockKind::Grid { name } => {
                    let body = body_or_default(self.template.body.or(self.state.body));
                    let Some(grid) = self.look_up(scopes, name).and_then(|value| Grid::read(value, body)) else { continue };
                    let columns = grid.columns();
                    for (index, items) in grid.items.chunks(grid.rows.saturating_mul(columns)).enumerate() {
                        grid.write_table_start(&mut self.out, index);
                        for row in items.chunks(columns) {
                            grid.write_row_start(&mut self.out);
                            for column in 0..columns {
                                grid.write_cell_start(&mut self.out, column);
                                let before = self.out.len();
                                if let Some(item) = row.get(column) {
                                    scopes.push(item);
                                    self.render_range(block.body_start, block.body_end, &block.children, scopes);
                                    scopes.pop();
                                }
                                // A cell must hold a paragraph.
                                if !self.out[before..].contains("<w:p") {
                                    self.out.push_str("<w:p/>");
                                }
                                self.out.push_str("</w:tc>");
                            }
                            self.out.push_str("</w:tr>");
                        }
                        self.out.push_str("</w:tbl>");
                    }
                    grid.write_end(&mut self.out);
                }
                BlockKind::Placeholder { names } => {
                    let values: Vec<Option<&'a Value>> = names.iter().map(|name| self.look_up(scopes, name)).collect();
                    if !values.iter().all(|value| is_shown(*value)) {
                        continue;
                    }
                    // The first list repeats the paragraph; else the first object is its scope.
                    let body = |render: &mut Self, scopes: &mut Vec<&'a Value>| render.render_range(block.body_start, block.body_end, &block.children, scopes);
                    let scope = values.iter().flatten().find(|value| value.is_array()).or_else(|| values.iter().flatten().find(|value| value.is_object())).copied();
                    self.placeholders.push((names, values));
                    match scope {
                        Some(Value::Array(items)) => {
                            for item in items {
                                scopes.push(item);
                                body(self, scopes);
                                scopes.pop();
                            }
                        }
                        Some(object) => {
                            scopes.push(object);
                            body(self, scopes);
                            scopes.pop();
                        }
                        None => body(self, scopes),
                    }
                    self.placeholders.pop();
                }
                BlockKind::Loop { name, is_inverted, page_breaks } => {
                    let printed_from = self.out.len();
                    let value = self.look_up(scopes, name);
                    match value {
                        _ if *is_inverted && !is_shown(value) => self.render_item(block, *page_breaks, scopes),
                        _ if *is_inverted => {}
                        Some(Value::Array(items)) => {
                            for (index, item) in items.iter().enumerate() {
                                self.loop_indexes.push(index);
                                scopes.push(item);
                                self.render_item(block, *page_breaks, scopes);
                                scopes.pop();
                                self.loop_indexes.pop();
                            }
                        }
                        Some(object @ Value::Object(_)) => {
                            scopes.push(object);
                            self.render_item(block, *page_breaks, scopes);
                            scopes.pop();
                        }
                        // Any other shown value prints the body once in the current scope.
                        _ if is_shown(value) => self.render_item(block, *page_breaks, scopes),
                        _ => {}
                    }
                    if page_breaks.after_last {
                        self.end_with_page_break(printed_from);
                    }
                }
            }
        }
        self.render_parts(at, to, scopes);
    }

    /// One repeat of a loop's body, after a page break paragraph when its opening paragraph had one.
    fn render_item(&mut self, block: &'a Block, page_breaks: LoopPageBreaks, scopes: &mut Vec<&'a Value>) {
        if page_breaks.before_each {
            self.out.push_str(&format!("<w:p>{PAGE_BREAK_RUN}</w:p>"));
        }
        self.render_range(block.body_start, block.body_end, &block.children, scopes);
    }

    /// Ends the last paragraph printed since `from` with a page break, or prints one holding it.
    fn end_with_page_break(&mut self, from: usize) {
        match self.out[from..].rfind("</w:p>") {
            Some(at) => self.out.insert_str(from + at, PAGE_BREAK_RUN),
            None => self.out.push_str(&format!("<w:p>{PAGE_BREAK_RUN}</w:p>")),
        }
    }

    fn render_parts(&mut self, from: usize, to: usize, scopes: &[&'a Value]) {
        let template = self.template;
        for part in &template.parts[from..to] {
            match part {
                Part::Element { kind, name, raw } => {
                    if name == "w:drawing" && *kind == ElementKind::Open {
                        self.pictures.push(PictureFrame {
                            start: self.out.len(), replacement: None,
                            #[cfg(feature = "qr-codes")]
                            source_properties: None,
                        });
                    }
                    if name == "wp:docPr" { self.prepare_picture(raw, scopes); }
                    self.print_element(*kind, name, raw);
                    if name == "w:drawing" && *kind == ElementKind::Close { self.finish_picture(scopes); }
                }
                Part::Text(text) => self.out.push_str(text),
                Part::Tag(index) => {
                    let tag = &template.tags[*index];
                    let value = match self.placeholder_value(&tag.kind, &tag.name) {
                        Some(value) => value,
                        None if tag.kind.prints_value() => self.look_up(scopes, &tag.name),
                        None => None,
                    };
                    match tag.kind {
                        TagKind::Value => {
                            let Some(value) = value.filter(|value| !value.is_null()) else {
                                let missing = MissingValue { kind: MissingKind::Value, name: &tag.name, written: &tag.text };
                                if let Some(text) = self.hooks.missing_value(self.part, missing) {
                                    self.print_text(&text, tag.run_props.as_deref());
                                }
                                continue;
                            };
                            let commented = self.template.comment_defaults.as_ref().and_then(|defaults| read_commented(value, defaults));
                            match commented {
                                Some((text, comments)) => self.print_commented(&text, comments, tag.run_props.as_deref()),
                                None => self.print_text(&print_value(value), tag.run_props.as_deref()),
                            }
                        }
                        TagKind::Chart => crate::charts::queue(&self.out, &mut self.charts, value),
                        TagKind::Module(module) => {
                            let printed = self.hooks.module_tag(self.part, ModuleTag { module, name: &tag.name, value });
                            self.out.push_str(&printed);
                        }
                        TagKind::CellStyle => {
                            self.print_style_text(value, tag.run_props.as_deref());
                            if let (Some(style), Some(cell)) = (read_style(value), self.cells.last_mut()) {
                                cell.1 = Some(style);
                            }
                        }
                        TagKind::Style(StyleTarget::Range) => {
                            self.print_style_text(value, tag.run_props.as_deref());
                            self.out.push(RANGE_START);
                            self.out.push_str(&self.ranges.len().to_string());
                            self.out.push(RANGE_START_END);
                            self.ranges.push(read_style(value).unwrap_or_default());
                        }
                        TagKind::StyleEnd => {
                            if !self.ranges.is_empty() {
                                self.out.push(RANGE_END);
                            }
                        }
                        TagKind::Style(target) => {
                            self.print_style_text(value, tag.run_props.as_deref());
                            let frame = match target {
                                StyleTarget::Paragraph => self.paragraphs.last_mut().map(|frame| &mut frame.style),
                                StyleTarget::Row => self.rows.last_mut().map(|frame| &mut frame.1),
                                StyleTarget::Run => self.runs.last_mut().map(|frame| &mut frame.1),
                                StyleTarget::Shape => self.shapes.last_mut().map(|frame| &mut frame.1),
                                StyleTarget::Range => None,
                            };
                            if let (Some(style), Some(frame)) = (read_style(value), frame) {
                                *frame = Some(frame.take().unwrap_or_default().and(style));
                            }
                        }
                        TagKind::GlobalStyle(kind) => {
                            self.print_style_text(value, tag.run_props.as_deref());
                            self.restyle_definition(kind, read_style(value));
                        }
                        TagKind::TableMerge(ref merge) => {
                            if let Some(table) = self.tables.last_mut() {
                                table.1.push(merge.clone());
                            }
                        }
                        TagKind::Placeholder => {
                            // A list's item prints in its own paragraph.
                            let printed = match value {
                                Some(Value::Array(_)) => scopes.last().filter(|item| !item.is_object() && !item.is_array()).map(|item| print_value(item)),
                                Some(Value::Bool(_) | Value::Object(_)) | None => None,
                                Some(value) => Some(print_value(value)),
                            };
                            if let Some(printed) = printed {
                                escape(&printed, &mut self.out);
                            }
                        }
                        TagKind::Footnote => self.print_footnote(value, tag.run_props.as_deref()),
                        TagKind::RichText { is_block, container_width } => {
                            let html = value.and_then(Value::as_str).unwrap_or_default().to_string();
                            let image_mode = if self.template.html_images { rich_text::ImageMode::Embed } else { rich_text::ImageMode::Ignore };
                            let mut prepared = rich_text::prepare_html(&html, &self.template.html_options, &self.template.html_style_sheet, image_mode, self.hooks);
                            for request in &prepared.images {
                                match self.html_image(request) {
                                    Ok(fragment) => prepared.raw[request.slot] = fragment,
                                    Err(error) => prepared.errors.push(error),
                                }
                            }
                            let mut errors = prepared.errors;
                            if is_block && !tag.is_alone && self.template.html_options.on_invalid_block == crate::HtmlBlockMode::Error {
                                errors.push(crate::FillError::new("HTML block tag must be alone in its paragraph").with_id(crate::FillErrorId::HtmlBlockInvalid));
                            }
                            if !is_block && prepared.has_block && self.template.html_options.on_invalid_block == crate::HtmlBlockMode::Error {
                                errors.push(crate::FillError::new("Inline HTML contains block elements").with_id(crate::FillErrorId::HtmlBlockInvalid));
                            }
                            self.html_errors.extend(errors.into_iter().map(|mut error| {
                                error.part = (!self.part.is_empty()).then(|| self.part.to_string());
                                error.tag = Some(tag.name.clone().into_boxed_str());
                                error.paragraph = Some(tag.paragraph);
                                error
                            }));
                            let is_block = is_block || (prepared.has_block && matches!(self.template.html_options.on_invalid_block, crate::HtmlBlockMode::Block | crate::HtmlBlockMode::Preserve));
                            self.out.push_str(&rich_text::mark(self.rich_values.len()));
                            let container_width = container_width.unwrap_or_else(|| body_or_default(self.template.body.or(self.state.body)).0);
                            let container_width = (container_width / crate::template::PX_PER_TWIP).round().clamp(1.0, i32::MAX as f64) as u32;
                            self.rich_values.push(RichValue { html: prepared.html, raw: prepared.raw, container_width, is_block, block_mode: self.template.html_options.on_invalid_block, has_extras: self.template.html_extras, style_sheet: self.template.html_style_sheet.clone() });
                            if let Some(paragraph) = self.paragraphs.last_mut() {
                                paragraph.has_rich_text = true;
                            }
                        }
                        TagKind::Image { cell_width, is_centered } => self.print_image(value, cell_width, is_centered),
                        TagKind::Raw | TagKind::RawRun | TagKind::Open { .. } | TagKind::Close | TagKind::FullTable | TagKind::GridOpen => {}
                        TagKind::Include | TagKind::SegmentOpen { .. } | TagKind::IncludeSegment { .. } | TagKind::Section(_) => {}
                    }
                }
            }
        }
    }

    fn print_style_text(&mut self, value: Option<&Value>, run_props: Option<&str>) {
        if let Some(text) = value.and_then(|value| value.get("text")).and_then(Value::as_str) {
            self.print_text(text, run_props);
        }
    }

    fn prepare_picture(&mut self, raw: &str, scopes: &[&'a Value]) {
        if self.pictures.is_empty() { return; }
        #[cfg(feature = "qr-codes")]
        { self.pictures.last_mut().expect("checked above").source_properties = Some(raw.to_string()); }
        let tag = ["descr", "name"].iter().filter_map(|name| read_attribute(raw, name)).map(crate::xml::unescape)
            .find_map(|text| self.template.picture_tags.get(&text));
        let Some(tag) = tag else { return };
        let Some(replacement) = self.read_picture(tag, scopes) else { return };
        self.pictures.last_mut().expect("checked above").replacement = Some(replacement);
    }

    fn read_picture(&mut self, tag: &PictureTag, scopes: &[&'a Value]) -> Option<PictureReplacement> {
        let value = self.look_up(scopes, &tag.name).filter(|value| is_truthy(value))?;
        let image = self.hooks.image(self.part, value)?;
        let size = self.hooks.image_size(self.part, value, &image);
        let fallback = if image.extension.eq_ignore_ascii_case("svg") { self.hooks.svg_fallback(self.part, value, &image) } else { None };
        Some(PictureReplacement { image, size, tag: tag.literal.clone(), fallback })
    }

    #[cfg(feature = "qr-codes")]
    fn read_qr_picture(&mut self, properties: &str, start: usize, scopes: &[&'a Value]) -> Option<PictureReplacement> {
        let (_, id) = crate::qr_codes::picture_key(&self.out[start..])?;
        let tag = self.template.qr_tags.get(&(properties.to_string(), id.to_string()))?;
        self.read_picture(tag, scopes)
    }

    fn finish_picture(&mut self, _scopes: &[&'a Value]) {
        let Some(frame) = self.pictures.pop() else { return };
        let PictureFrame { start, replacement, .. } = frame;
        #[cfg(feature = "qr-codes")]
        let replacement = replacement.or_else(|| frame.source_properties.as_deref().and_then(|properties| self.read_qr_picture(properties, start, _scopes)));
        let Some(replacement) = replacement else { return };
        let id = self.state.next_drawing_id;
        self.state.next_drawing_id = id.wrapping_add(1);
        let mut media = MediaUse::for_image(replacement.image);
        media.relationship_id = format!("rIdDocxFillReplacement_{id}");
        media.target = format!("media/replacements/{id}.{}", media.image.extension);
        let fallback = replacement.fallback.map(|bytes| {
            let mut fallback = MediaUse::svg_fallback(&media.image, bytes);
            fallback.relationship_id = format!("rIdDocxFillReplacementFallback_{id}");
            fallback.target = format!("media/replacements/{id}.png");
            fallback
        });
        let displayed = fallback.as_ref().unwrap_or(&media);
        let svg = fallback.as_ref().map(|_| media.relationship_id.as_str());
        if let Some(xml) = replace_picture(&self.out[start..], &displayed.relationship_id, replacement.size, &replacement.tag, svg) {
            self.out.replace_range(start.., &xml);
            self.media.push(media);
            self.media.extend(fallback);
        }
    }

    /// Prints an element, rewriting a paragraph with rich text, or a styled or merged element, as
    /// it closes.
    fn print_element(&mut self, kind: ElementKind, name: &str, raw: &str) {
        let start = self.out.len();
        self.out.push_str(raw);
        match (kind, name) {
            (ElementKind::Open, PARAGRAPH) => self.paragraphs.push(ParagraphFrame { start, has_rich_text: false, style: None, is_dropped: false, captions: Vec::new() }),
            (ElementKind::Open, shape) if SHAPES.contains(&shape) => self.shapes.push((start, None)),
            (ElementKind::Open, CELL) => self.cells.push((start, None)),
            (ElementKind::Open, ROW) => self.rows.push((start, None)),
            (ElementKind::Open, RUN) => self.runs.push((start, None)),
            (ElementKind::Open, TABLE) => self.tables.push((start, Vec::new())),
            (ElementKind::Close, PARAGRAPH) => {
                let Some(ParagraphFrame { start: opened, has_rich_text, style, is_dropped, captions }) = self.paragraphs.pop() else { return };
                if is_dropped {
                    self.out.truncate(opened);
                    return;
                }
                if !has_rich_text && style.is_none() && captions.is_empty() {
                    return;
                }
                let mut written = if has_rich_text { rich_text::write_paragraph(&self.out[opened..], &self.rich_values, &mut self.links, self.template.continued_bookmarks.as_deref().map(|prefix| (prefix, &mut self.state.next_bookmark_id)), self.state.numbering.as_mut()) } else { self.out[opened..].to_string() };
                if let Some(style) = style {
                    written = edit_each(&written, PARAGRAPH, |paragraph| style_paragraph(paragraph, &style, true));
                }
                self.out.replace_range(opened.., &written);
                for caption in captions { self.out.push_str(&caption); }
            }
            (ElementKind::Close, CELL) => {
                let frame = self.cells.pop();
                self.restyle(frame, style_cell);
            }
            (ElementKind::Close, ROW) => {
                let frame = self.rows.pop();
                self.restyle(frame, style_row);
            }
            (ElementKind::Close, RUN) => {
                let frame = self.runs.pop();
                self.restyle(frame, style_run);
            }
            (ElementKind::Close, shape) if SHAPES.contains(&shape) => {
                let frame = self.shapes.pop();
                self.restyle(frame, style_shape);
            }
            (ElementKind::Close, TABLE) => {
                if let Some((opened, merges)) = self.tables.pop().filter(|(_, merges)| !merges.is_empty()) {
                    let merged = merge_table(&self.out[opened..], &merges);
                    self.out.replace_range(opened.., &merged);
                }
            }
            _ => {}
        }
    }

    /// Records a change to the definition of the open paragraph's or run's style, read from its
    /// `<w:pStyle>` or `<w:rStyle>`; without one, the default style's.
    fn restyle_definition(&mut self, kind: StyleKind, style: Option<Style>) {
        let Some(style) = style else { return };
        let (start, properties, name) = match kind {
            StyleKind::Paragraph => (self.paragraphs.last().map(|frame| frame.start), "</w:pPr>", "w:pStyle"),
            StyleKind::Character => (self.runs.last().map(|frame| frame.0), "</w:rPr>", "w:rStyle"),
        };
        let Some(start) = start else { return };
        let written = &self.out[start..];
        let properties = written.find(properties).map_or("", |end| &written[..end]);
        let id = cell_styling::find_element_start(properties, &format!("<{name}")).and_then(|at| {
            let raw = &properties[at..at + properties[at..].find('>')? + 1];
            read_attribute(raw, "w:val").map(str::to_string)
        });
        self.style_definitions.push((kind, id, style));
        if let (StyleKind::Paragraph, true, Some(frame)) = (kind, self.template.drop_style_paragraphs, self.paragraphs.last_mut()) {
            frame.is_dropped = true;
        }
    }

    /// Restyles the element a frame opened, now closed, when its frame has a style.
    fn restyle(&mut self, frame: Option<(usize, Option<Style>)>, styled: fn(&str, &Style) -> String) {
        if let Some((opened, Some(style))) = frame {
            let written = styled(&self.out[opened..], &style);
            self.out.replace_range(opened.., &written);
        }
    }

    /// Ends the tag's run, adds a superscript run referencing a new footnote, and goes on in a
    /// run formatted as the tag's.
    fn print_footnote(&mut self, value: Option<&Value>, run_props: Option<&str>) {
        let Some(text) = value.filter(|value| is_truthy(value)).map(print_value) else { return };
        let Some(props) = run_props else { return };
        // Footnotes belong to the body; a single part filled alone counts as one.
        if !(self.part.is_empty() || self.part == self.state.main_part) {
            return;
        }
        let id = self.state.next_footnote_id;
        self.state.next_footnote_id += 1;
        self.out.push_str(&format!("</w:t></w:r><w:r>{SUPERSCRIPT}<w:footnoteReference w:id=\"{id}\"/></w:r><w:r>{props}<w:t xml:space=\"preserve\">"));
        self.footnotes.push((id, crate::footnotes::write_note(id, &text, &self.template.footnotes)));
    }

    /// The value's text marked with its comments, in the body; elsewhere the text alone.
    fn print_commented(&mut self, text: &str, comments: Vec<Comment>, run_props: Option<&str>) {
        let is_body = self.part.is_empty() || self.part == self.state.main_part;
        let Some(props) = run_props.filter(|_| is_body) else {
            self.print_text(text, run_props);
            return;
        };
        let first = self.state.next_comment_id;
        self.state.next_comment_id += comments.len() as u32;
        let ids = first..self.state.next_comment_id;
        self.out.push_str("</w:t></w:r>");
        for id in ids.clone() {
            self.out.push_str(&format!("<w:commentRangeStart w:id=\"{id}\"/>"));
        }
        self.out.push_str("<w:r>");
        self.out.push_str(props);
        self.out.push_str("<w:t xml:space=\"preserve\">");
        self.print_text(text, Some(props));
        self.out.push_str("</w:t></w:r>");
        for id in ids.clone() {
            self.out.push_str(&format!("<w:commentRangeEnd w:id=\"{id}\"/><w:r><w:rPr><w:rStyle w:val=\"CommentReference\"/></w:rPr><w:commentReference w:id=\"{id}\"/></w:r>"));
        }
        self.out.push_str("<w:r>");
        self.out.push_str(props);
        self.out.push_str("<w:t xml:space=\"preserve\">");
        for (id, comment) in ids.zip(comments) {
            self.comments.push(Comment { id, ..comment });
        }
    }

    fn print_image(&mut self, value: Option<&Value>, cell_width: Option<f64>, is_centered: bool) {
        let Some(value) = value.filter(|value| is_truthy(value)) else { return };
        let Some(image) = self.hooks.image(self.part, value) else { return };
        let properties = self.hooks.image_properties(self.part, value, &image);
        if let (true, Some(paragraph)) = (is_centered, self.paragraphs.last_mut()) {
            paragraph.style.get_or_insert_with(Style::default).align = Some("center");
        }

        let (width, height) = match self.hooks.image_size(self.part, value, &image) {
            Some(size) => size.dimensions(),
            None => {
                let (body_width, body_height) = body_or_default(self.template.body.or(self.state.body));
                let width = cell_width.unwrap_or(body_width) * self.state.container_fill;
                let height = body_height * self.state.container_fill;
                drawing_size(&image, width, cell_width.is_some(), height)
            }
        };
        let xml = self.image_xml(image, value, &properties, (width, height), None);
        // The tag sits inside <w:t>; a drawing belongs beside it.
        self.out.push_str("</w:t>");
        self.out.push_str(&xml);
        self.out.push_str("<w:t xml:space=\"preserve\">");
    }

    fn html_image(&mut self, request: &rich_text::images::HtmlImage) -> Result<crate::HtmlFragment, crate::FillError> {
        let value = Value::String(request.source.clone());
        let image = match self.hooks.image(self.part, &value) {
            Some(image) => image,
            None => request.embedded().map_err(crate::FillError::new)?.ok_or_else(|| crate::FillError::new("HTML image source was not resolved"))?,
        };
        let body = body_or_default(self.template.body.or(self.state.body));
        let size = self.hooks.image_size(self.part, &value, &image).or_else(|| request.size(&image, body.0))
            .ok_or_else(|| crate::FillError::new("Invalid HTML image dimensions"))?;
        let properties = self.hooks.image_properties(self.part, &value, &image);
        let xml = self.image_xml(image, &value, &properties, size.dimensions(), Some(&request.properties()));
        let xml = xml.replacen("<w:drawing>", "<w:drawing xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">", 1);
        Ok(crate::HtmlFragment::inline(format!("<w:r>{xml}</w:r>")))
    }

    fn image_xml(&mut self, image: Image, value: &Value, properties: &crate::ImageProperties, dimensions: (f64, f64), defaults: Option<&crate::ImageProperties>) -> String {
        let (width, height) = dimensions;
        if let (Some(align), Some(paragraph)) = (properties.alignment(), self.paragraphs.last_mut()) {
            paragraph.style.get_or_insert_with(Style::default).align = Some(align);
        }
        let fallback = if image.extension.eq_ignore_ascii_case("svg") {
            self.hooks.svg_fallback(self.part, value, &image).map(|bytes| MediaUse::svg_fallback(&image, bytes))
        } else { None };
        let media = MediaUse::for_image(image);
        let id = self.state.next_drawing_id;
        self.state.next_drawing_id = id.wrapping_add(1);

        let displayed = fallback.as_ref().unwrap_or(&media);
        let svg = fallback.as_ref().map(|_| media.relationship_id.as_str());
        let link = properties.link_target().map(|target| {
            let id = rich_text::link_relationship_id(self.links.len());
            self.links.push(target.to_string());
            id
        });
        let mut xml = drawing_xml(id, &displayed.relationship_id, width, height, svg);
        if let Some(defaults) = defaults { xml = defaults.apply(xml, None); }
        let xml = properties.apply(xml, link.as_deref());
        if let (Some(caption), Some(paragraph)) = (properties.caption_xml(self.state.next_caption), self.paragraphs.last_mut()) {
            paragraph.captions.push(caption);
            self.state.next_caption += 1;
        }
        for media in std::iter::once(media).chain(fallback) {
            if !self.media.iter().any(|used| used.relationship_id == media.relationship_id) {
                self.media.push(media);
            }
        }
        xml
    }

    /// Prints a value. With line breaks on, each `\n` ends the tag's run, adds a run holding a
    /// break and goes on in a run with the tag run's properties.
    fn print_text(&mut self, text: &str, run_props: Option<&str>) {
        if !self.template.linebreaks || !text.contains('\n') {
            escape(text, &mut self.out);
            return;
        }
        for (index, line) in text.split('\n').enumerate() {
            if index > 0 {
                match run_props {
                    Some(props) => {
                        self.out.push_str("</w:t></w:r><w:r><w:br/></w:r><w:r>");
                        self.out.push_str(props);
                        self.out.push_str("<w:t xml:space=\"preserve\">");
                    }
                    None => self.out.push_str(LINE_BREAK),
                }
            }
            escape(line.strip_suffix('\r').unwrap_or(line), &mut self.out);
        }
    }
}

/// The scopes' fields in one object, inner ones over outer ones, nulls left out as `look_up`
/// skips them.
fn merge_scopes(scopes: &[&Value]) -> Value {
    let mut merged = serde_json::Map::new();
    for scope in scopes {
        let Value::Object(fields) = scope else { continue };
        for (name, value) in fields.iter().filter(|(_, value)| !value.is_null()) {
            merged.insert(name.clone(), value.clone());
        }
    }
    Value::Object(merged)
}

/// `name`'s value in the innermost scope that has it, out to the root. A null counts as
/// missing, so an outer scope's value shows through.
fn look_up<'a>(scopes: &[&'a Value], name: &str) -> Option<&'a Value> {
    scopes.iter().rev().find_map(|scope| {
        let value = if name == "." { Some(*scope) } else { scope.get(name) };
        value.filter(|value| !value.is_null())
    })
}

/// Whether a loop over `value` prints: not missing, false, zero, empty text or an empty list.
fn is_shown(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Array(items)) => !items.is_empty(),
        Some(value) => is_truthy(value),
        None => false,
    }
}

/// JavaScript truthiness: not null, false, zero or empty text.
fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(is_true) => *is_true,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// `value` as text, as JavaScript's `String(value)` gives it.
pub(crate) fn print_value(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(is_true) => is_true.to_string(),
        Value::Number(number) => print_number(number),
        Value::String(text) => text.clone(),
        Value::Array(items) => items.iter().map(print_value).collect::<Vec<_>>().join(","),
        Value::Object(_) => "[object Object]".to_string(),
    }
}

fn print_number(number: &serde_json::Number) -> String {
    if number.is_i64() || number.is_u64() {
        return number.to_string();
    }
    let float = number.as_f64().unwrap_or_default();
    if float == 0.0 {
        return "0".to_string();
    }
    // JavaScript writes very large and very small numbers with an exponent, signed: 1e+21, 1e-7.
    if float.abs() >= 1e21 || float.abs() < 1e-6 {
        let written = format!("{float:e}");
        return match written.split_once('e') {
            Some((digits, exponent)) if !exponent.starts_with('-') => format!("{digits}e+{exponent}"),
            _ => written,
        };
    }
    // Whole floats print without a fraction, as in JavaScript.
    if float.fract() == 0.0 {
        return format!("{float:.0}");
    }
    float.to_string()
}
