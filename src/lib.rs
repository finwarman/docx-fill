//! Fills DOCX templates written in docxtemplater's public tag syntax, with `{ }` delimiters:
//!
//! - `{name}` prints a value; a missing or null value prints nothing.
//! - `{#name}…{/name}` repeats once per item of a list, once for an object or any other shown
//!   value, and not at all for a missing, false, zero, empty or null one. `{^name}…{/name}` prints
//!   only when `{#name}` would not.
//! - `{-w:tr name}…{/name}` repeats whole elements (`w:tr`, `w:tc`, `w:p`, …).
//! - `{@name}` replaces its paragraph with the value as raw XML.
//!
//! Names are looked up whole (`table:abc.value` is one key), from the innermost loop item out to
//! the root, read as expressions with [`FillOptions::expressions`], or read your own way with
//! [`FillHooks::tag_value`]. Tags are found in each paragraph's `<w:t>` text joined, since Word
//! splits a tag across runs; a tag's value prints in the run its `{` is in, keeping that run's
//! formatting.
//!
//! Built from the WordprocessingML spec and docxtemplater's documented behaviour.

mod cell_styling;
mod comments;
mod document_edits;
mod empty_last_page;
mod expressions;
mod footnotes;
mod full_table;
mod images;
mod includes;
mod json;
mod part_kinds;
mod placeholder_lines;
mod properties;
mod sections;
mod protection;
mod render;
mod rich_text;
mod structure;
mod styling;
mod table_grid;
mod table_merge;
mod tag_list;
mod template;
mod text;
mod unique_ids;
mod xml;

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde_json::Value;

pub use document_edits::{DocumentEdits, Margins, Watermark, WatermarkLayout};
pub use protection::{HashAlgorithm, Protection, ProtectionKind};
pub use empty_last_page::drop_empty_last_page;
pub use comments::CommentDefaults;
pub use placeholder_lines::{ParagraphPlaceholderOptions, SoftBreakScope};
pub use footnotes::{FootnoteContent, FootnoteOptions};
pub use full_table::{TableOptions, TableResize, TableValidation};
pub use images::{Image, ImageAlignment, ImageBorder, ImageCaption, ImageFloat, ImageLayer, ImageLength, ImageProperties, ImageRelative, ImageSize, ImageWrap};
pub use includes::{IncludeMode, IncludeStyles, Subtemplate};
pub use json::read_include_options;
pub use render::{FillHooks, MissingKind, MissingValue, ModuleTag, NoHooks};
pub use tag_list::{TagType, TemplateTag};
pub use template::Template;
pub use text::{Bookmark, CommentedText, ContentBoundary, FilledDocument, PageBreak, TextPosition, TextRange};

use images::MediaUse;
use part_kinds::{PartKind, PartKinds};
use render::FillState;
use template::TextLayout;

pub(crate) const DOCUMENT_PART: &str = "word/document.xml";
pub(crate) const STYLES_PART: &str = "word/styles.xml";
pub(crate) const CONTENT_TYPES_PART: &str = "[Content_Types].xml";
const IMAGE_RELATIONSHIP: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";
const HYPERLINK_RELATIONSHIP: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink";
const RELATIONSHIPS_NAMESPACE: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
// Images fill their container less 5% for cell padding.
pub(crate) const CONTAINER_FILL: f64 = 0.95;

/// The strings around a tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delimiters {
    pub start: String,
    pub end: String,
}

impl Default for Delimiters {
    fn default() -> Self {
        Delimiters { start: "{".to_string(), end: "}".to_string() }
    }
}

/// Prefixes that classify standard and extension tags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagPrefixes {
    pub loop_start: String,
    pub loop_end: String,
    pub inverted_loop: String,
    pub raw: String,
    pub image: String,
    pub centered_image: String,
    pub html: String,
    pub block_html: String,
    pub word_run: String,
}

impl Default for TagPrefixes {
    fn default() -> Self {
        Self { loop_start: "#".into(), loop_end: "/".into(), inverted_loop: "^".into(), raw: "@".into(), image: "%".into(), centered_image: "%%".into(), html: "~".into(), block_html: "~~".into(), word_run: "r@".into() }
    }
}

impl TagPrefixes {
    pub(crate) fn validate(&self) -> Result<(), FillError> {
        let values = [&self.loop_start, &self.loop_end, &self.inverted_loop, &self.raw, &self.image, &self.centered_image, &self.html, &self.block_html, &self.word_run];
        let mut seen = BTreeSet::new();
        for (index, value) in values.into_iter().enumerate() {
            if (index < 4 && value.is_empty()) || !seen.insert(value) {
                return Err(FillError::new("Core tag prefixes must be nonempty; all tag prefixes must be distinct"));
            }
        }
        Ok(())
    }
}

/// Tag kinds beyond the core syntax, each on or off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extensions {
    /// `{:vt#cols}` … `{:vt/cols}`: repeats table cells, as `{-w:tc cols}` … `{/cols}` does.
    pub vertical_tables: bool,
    /// `{:stylecell name}`: a value of `{ "cellBackground": "#rrggbb" }` fills the tag's cell.
    pub cell_styling: bool,
    /// `{~~name}` and `{~name}`: HTML printed as Word paragraphs and runs.
    pub rich_text: bool,
    /// `{%name}` and `{%%name}`: an image from [`FillHooks::image`].
    pub images: bool,
    /// `{:merge-cells-col 1,2}`, `{:merge-cells-col rtl 1,2}` and `{:merge-cells-row 1,-1}`: merges
    /// neighbouring cells of the tag's table that print the same, non-empty text.
    pub table_merge: bool,
    /// `{:stylepar name}`, `{:stylerow name}`, `{:stylerun name}`, `{:stylebullets name}` and
    /// `{:styleshape name}`: a style value on the tag's paragraph, row, run, list bullet or
    /// shape. `{:stylestart name}` … `{:/style}` styles the runs between. `{::stylepar name}` and
    /// `{::stylerun name}` restyle the definition of the tag paragraph's or run's style.
    pub styling: bool,
    /// `{r@name}`: replaces the tag's run with the value as raw `<w:r>` XML.
    pub word_run: bool,
    /// `{?name}`: drops the tag's paragraph for a value a loop skips, repeats it per item of a list
    /// and keeps it once otherwise. A list item or text value prints in the tag's place.
    pub paragraph_placeholder: bool,
    /// `{:footnote name}`: a footnote holding the value, in the body only.
    pub footnotes: bool,
    /// `{:table name}` builds a table from data in place of its paragraph; `{:#grid name}` …
    /// `{:/grid}` prints the paragraphs between them once per item, in a grid of cells.
    pub full_tables: bool,
    /// Rich text also reads the HTML module's further elements and inline styles: links, tables,
    /// strike, sup/sub, mark, code, pre, hr, blockquote, alignment, colours and sizes. Off, it
    /// reads only p, br, h1-h6, b/strong, i/em, u and lists, without attributes.
    pub html_extras: bool,
    /// `{%%name}`, alone in its paragraph, centres the paragraph. Off, it draws as `{%name}` does.
    pub centered_images: bool,
    /// `{:include name}` prints the bodies of the subtemplates the value names (see
    /// [`PackageTemplate::add_include`]) in place of its paragraph. `{:segment name}` …
    /// `{:segment/}` defines a segment, printed only where `{:includesegment name}` or
    /// `{:includesegment* expression}` names it; `{:inlinesegment name}` …
    /// `{:inlinesegment/}` and `{:includeinlinesegment name}` do the same within a paragraph.
    pub subtemplates: bool,
    /// A value tag whose value is `{ "text": …, "comment": … }` or `{ "text": …, "comments":
    /// [{ "comment": …, "author": …, "date": … }] }` prints the text with Word comments on it, in
    /// the body. [`FillOptions::comments`] gives the author and date a value leaves out.
    pub comments: bool,
    /// In the body, `{:subsection name}` ends the section before its paragraph, and the rest of
    /// the section takes the headers and footers of the subtemplate the value names;
    /// `{:replacesection name}` gives them to the whole section. `{:page props}` sets the
    /// section's `orientation` (`portrait`, `landscape`), `size` (`A4`, `Letter`, … or
    /// `[width, height]` in twips) and `margins` (`top`, `left`, … in twips).
    pub sections: bool,
}

impl Extensions {
    pub const ALL: Extensions = Extensions {
        vertical_tables: true,
        cell_styling: true,
        rich_text: true,
        images: true,
        table_merge: true,
        styling: true,
        word_run: true,
        paragraph_placeholder: true,
        footnotes: true,
        full_tables: true,
        html_extras: true,
        centered_images: true,
        subtemplates: true,
        comments: true,
        sections: true,
    };
    pub const NONE: Extensions = Extensions {
        vertical_tables: false,
        cell_styling: false,
        rich_text: false,
        images: false,
        table_merge: false,
        styling: false,
        word_run: false,
        paragraph_placeholder: false,
        footnotes: false,
        full_tables: false,
        html_extras: false,
        centered_images: false,
        subtemplates: false,
        comments: false,
        sections: false,
    };
}

/// How a template is read and filled.
#[derive(Debug, Clone)]
pub struct FillOptions {
    /// A loop whose tags are each alone in their own paragraph repeats the paragraphs between
    /// them, and its tags' paragraphs go.
    pub paragraph_loop: bool,
    /// A `\n` in a value prints as a line break.
    pub linebreaks: bool,
    pub delimiters: Delimiters,
    pub tag_prefixes: TagPrefixes,
    /// What starts and ends a tag that sets new delimiters for the rest of the part, `=` for
    /// `{=[[ ]]=}`. `None` reads such tags as values.
    pub change_delimiter_prefix: Option<String>,
    /// A close delimiter with no tag open prints as text instead of being an error.
    pub allow_unopened_tag: bool,
    /// An open delimiter never closed, or followed by another, prints as text instead of being
    /// an error.
    pub allow_unclosed_tag: bool,
    pub extensions: Extensions,
    /// When filling a package, drops an empty last page from the body (see
    /// [`drop_empty_last_page`]).
    pub drop_empty_last_page: bool,
    /// Tag prefixes handed to [`FillHooks::module_tag`] instead of printed. They are checked
    /// before the extensions'.
    pub module_prefixes: Vec<String>,
    /// Edits to the whole package, applied when filling one.
    pub document: DocumentEdits,
    /// Drops the paragraphs holding `{::stylepar}` tags.
    pub drop_style_paragraphs: bool,
    pub comments: CommentDefaults,
    pub paragraph_placeholder: ParagraphPlaceholderOptions,
    /// Content mode and formatting of inserted footnotes.
    pub footnotes: FootnoteOptions,
    pub table: TableOptions,
    /// A bookmark prefix such as `tag_`. When rich text turns a tag's paragraph into several, and
    /// the tag's paragraph has a bookmark named this prefix and a number (`tag_3`), each later
    /// paragraph gets its own bookmark `tag_3_1`, `tag_3_2`, …, so a reader can find them all.
    /// A loop's repeats of these bookmarks keep their names; only their ids change.
    pub continued_bookmarks: Option<String>,
    /// Fills tags in the core, app and custom document properties, such as the title.
    pub fill_properties: bool,
    /// Reads tag names as expressions: `{user.name}`, `{items.length > 1}`, `{price * 1.2}`,
    /// `{name | upper}`, `{$index + 1}`. Names such as `table:abc.value` are then errors, and keys
    /// holding dots are not found. Off, a name is looked up whole.
    pub expressions: bool,
    /// The filters expressions can use, which [`FillHooks::filter`] applies. Any other is an error
    /// when parsing.
    pub filters: Vec<String>,
    /// CSS applied to HTML values before their inline styles.
    pub html_style_sheet: String,
}

impl Default for FillOptions {
    /// Paragraph loops, line breaks, every extension and empty-last-page dropping on, with `{ }`
    /// delimiters.
    fn default() -> Self {
        FillOptions {
            paragraph_loop: true,
            linebreaks: true,
            delimiters: Delimiters::default(),
            tag_prefixes: TagPrefixes::default(),
            change_delimiter_prefix: Some("=".to_string()),
            allow_unopened_tag: false,
            allow_unclosed_tag: false,
            extensions: Extensions::ALL,
            drop_empty_last_page: true,
            module_prefixes: Vec::new(),
            document: DocumentEdits::default(),
            drop_style_paragraphs: false,
            comments: CommentDefaults::default(),
            paragraph_placeholder: ParagraphPlaceholderOptions::default(),
            footnotes: FootnoteOptions::default(),
            table: TableOptions::default(),
            continued_bookmarks: None,
            fill_properties: true,
            expressions: false,
            filters: Vec::new(),
            html_style_sheet: String::new(),
        }
    }
}

impl FillOptions {
    /// Core tags only: no extensions, and the last page kept.
    pub fn core() -> FillOptions {
        FillOptions { extensions: Extensions::NONE, drop_empty_last_page: false, ..FillOptions::default() }
    }
}

/// Why a template could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FillError {
    /// The package part, when known.
    pub part: Option<String>,
    pub message: String,
}

impl FillError {
    pub(crate) fn new(message: impl Into<String>) -> FillError {
        FillError { part: None, message: message.into() }
    }
}

impl fmt::Display for FillError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.part {
            Some(part) => write!(f, "{part}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for FillError {}

/// Whether a package part holds tags: the body, a header or a footer.
pub fn is_template_part(name: &str) -> bool {
    if name == DOCUMENT_PART {
        return true;
    }
    let Some(file) = name.strip_prefix("word/").and_then(|file| file.strip_suffix(".xml")) else { return false };
    let Some(number) = file.strip_prefix("header").or_else(|| file.strip_prefix("footer")) else { return false };
    number.bytes().all(|byte| byte.is_ascii_digit())
}

/// Fills one part's XML.
pub fn fill_part(xml: &str, data: &Value, options: &FillOptions) -> Result<String, FillError> {
    Ok(Template::parse(xml, options)?.render(data))
}

/// A parsed DOCX package: its template parts parsed, every other part kept as it is.
#[derive(Debug)]
pub struct PackageTemplate {
    parts: Vec<(String, PackagePart)>,
    theme_colors: BTreeMap<String, String>,
    /// The main document part, `word/document.xml` unless the content types name another.
    main_part: String,
    drop_empty_last_page: bool,
    document: DocumentEdits,
    max_drawing_id: u32,
    max_bookmark_id: u32,
    kept_bookmarks: Option<String>,
    max_footnote_id: u32,
    max_comment_id: Option<u32>,
    body: Option<(f64, f64)>,
    includes: BTreeMap<String, Subtemplate>,
}

#[derive(Debug)]
enum PackagePart {
    Template(Box<Template>),
    Bytes(Vec<u8>),
}

impl PackageTemplate {
    /// Parses the template parts among `parts`, given by name without a leading `/`: the main
    /// document, headers, footers, footnotes and comments by their content types, and with
    /// [`FillOptions::fill_properties`] the core, app and custom document properties.
    pub fn parse(parts: impl IntoIterator<Item = (String, Vec<u8>)>, options: &FillOptions) -> Result<PackageTemplate, FillError> {
        options.document.check()?;
        let parts: Vec<(String, Vec<u8>)> = parts.into_iter().collect();
        let content_types = parts.iter().find(|(name, _)| name == CONTENT_TYPES_PART).map(|(_, bytes)| String::from_utf8_lossy(bytes));
        let kinds = PartKinds::read(content_types.as_deref());
        let main_part = parts.iter().map(|(name, _)| name).find(|name| kinds.kind(name) == PartKind::Main).cloned().unwrap_or_else(|| DOCUMENT_PART.to_string());
        let mut max_footnote_id = None;
        let mut max_comment_id = None;
        let mut parsed = Vec::new();
        for (name, bytes) in parts {
            if name == footnotes::FOOTNOTES_PART {
                max_footnote_id = Some(footnotes::read_max_id(&String::from_utf8_lossy(&bytes)));
            }
            if name == comments::COMMENTS_PART {
                max_comment_id = Some(comments::read_max_id(&String::from_utf8_lossy(&bytes)));
            }
            let layout = match kinds.kind(&name) {
                PartKind::Main | PartKind::Story => TextLayout::Paragraphs,
                PartKind::Properties if options.fill_properties => TextLayout::Plain,
                PartKind::Properties | PartKind::Other => {
                    parsed.push((name, PackagePart::Bytes(bytes)));
                    continue;
                }
            };

            let in_part = |mut error: FillError| {
                error.part = Some(name.clone());
                error
            };
            let xml = String::from_utf8(bytes).map_err(|_| in_part(FillError::new("The part is not UTF-8")))?;
            let template = Template::parse_text(&xml, options, layout).map_err(in_part)?;
            parsed.push((name, PackagePart::Template(Box::new(template))));
        }

        let templates = || parsed.iter().filter_map(|(name, part)| match part {
            PackagePart::Template(template) => Some((name, template)),
            PackagePart::Bytes(_) => None,
        });
        let max_drawing_id = templates().map(|(_, template)| template.max_drawing_id).max().unwrap_or(0);
        let max_bookmark_id = templates().map(|(_, template)| template.max_bookmark_id).max().unwrap_or(0);
        let body = templates().find(|(name, _)| **name == main_part).and_then(|(_, template)| template.body);
        Ok(PackageTemplate {
            main_part,
            theme_colors: BTreeMap::new(),
            max_comment_id,
            drop_empty_last_page: options.drop_empty_last_page,
            document: options.document.clone(),
            max_drawing_id,
            max_bookmark_id,
            kept_bookmarks: options.continued_bookmarks.clone(),
            max_footnote_id: max_footnote_id.unwrap_or(0),
            body,
            parts: parsed,
            includes: BTreeMap::new(),
        })
    }

    /// Replaces standard theme color slots in `word/theme/theme1.xml` during rendering.
    ///
    /// Keys are DrawingML names such as `a:dk1` and values are three- or six-digit hex colors.
    /// A leading `#` is optional.
    pub fn set_theme_colors(&mut self, colors: BTreeMap<String, String>) -> Result<(), FillError> {
        styling::validate_theme_colors(&colors)?;
        self.theme_colors = colors;
        Ok(())
    }

    /// Names a subtemplate `{:include}` tags print where their value is `name`, or a list
    /// holding it. A later subtemplate with the name replaces it.
    pub fn add_include(&mut self, name: impl Into<String>, include: Subtemplate) {
        self.includes.insert(name.into(), include);
    }

    /// Each template part's name with its tags, as [`Template::tags`] lists them.
    pub fn tags(&self) -> Vec<(&str, Vec<TemplateTag>)> {
        self.parts.iter().filter_map(|(name, part)| match part {
            PackagePart::Template(template) => Some((name.as_str(), template.tags())),
            PackagePart::Bytes(_) => None,
        }).collect()
    }

    /// Every part, in the order given, with the template parts filled with `data`.
    pub fn render(&self, data: &Value) -> Vec<(String, Cow<'_, [u8]>)> {
        self.render_with(data, &mut NoHooks)
    }

    /// [`PackageTemplate::render`], asking `hooks` for module tags and images. Images add their
    /// media parts, relationships and content types.
    pub fn render_with(&self, data: &Value, hooks: &mut dyn FillHooks) -> Vec<(String, Cow<'_, [u8]>)> {
        let mut state = FillState {
            assignments: expressions::Assignments::default(),
            next_drawing_id: self.max_drawing_id + 1,
            next_caption: 1,
            body: self.body,
            container_fill: CONTAINER_FILL,
            next_footnote_id: self.max_footnote_id + 1,
            next_comment_id: self.max_comment_id.map_or(0, |id| id + 1),
            next_bookmark_id: self.max_bookmark_id + 1,
            main_part: self.main_part.clone(),
            numbering: Some(rich_text::numbering::Numberings::read(&self.main_part, self.parts.iter().filter_map(|(name, part)| match part {
                PackagePart::Bytes(bytes) => Some((name.as_str(), bytes.as_slice())),
                PackagePart::Template(_) => None,
            }))),
        };
        let mut comments = Vec::new();
        let mut out: Vec<(String, Cow<[u8]>)> = Vec::with_capacity(self.parts.len());
        let mut media: Vec<(String, MediaUse)> = Vec::new();
        let mut notes = Vec::new();
        let mut links: Vec<(String, Vec<String>)> = Vec::new();
        let mut definitions = Vec::new();
        let mut includes = Vec::new();
        let mut sections = Vec::new();
        for (name, part) in &self.parts {
            let bytes = match part {
                PackagePart::Bytes(bytes) => Cow::Borrowed(bytes.as_slice()),
                PackagePart::Template(template) => {
                    let filled = template.fill(data, name, hooks, &mut state);
                    media.extend(filled.media.into_iter().map(|used| (name.clone(), used)));
                    if *name == self.main_part {
                        notes = filled.footnotes;
                        comments = filled.comments;
                        sections = filled.sections;
                    }
                    definitions.extend(filled.style_definitions);
                    if !filled.includes.is_empty() {
                        includes.push((name.clone(), filled.includes));
                    }
                    if !filled.links.is_empty() {
                        links.push((name.clone(), filled.links));
                    }
                    Cow::Owned(filled.xml.into_bytes())
                }
            };
            out.push((name.clone(), bytes));
        }
        add_media(&mut out, &media);
        add_links(&mut out, &links);
        restyle_definitions(&mut out, &definitions);
        footnotes::add_footnotes(&mut out, &notes);
        comments::add_comments(&mut out, &comments);
        if let Some(numbering) = state.numbering {
            numbering.write(&mut out, &self.main_part);
        }
        let count = includes::print_includes(&mut out, &includes, &self.includes, hooks);
        sections::apply_sections(&mut out, &sections, &self.includes, hooks, count);
        self.finish_template_parts(&mut out);
        apply_theme_colors(&mut out, &self.theme_colors);
        self.document.apply(&mut out);
        out
    }

    /// Renumbers repeated ids and repairs each template part's structure, now that subtemplates
    /// have printed, then drops an empty last page from the body.
    fn finish_template_parts(&self, parts: &mut [(String, Cow<[u8]>)]) {
        let is_template = |name: &str| self.parts.iter().any(|(each, part)| each == name && matches!(part, PackagePart::Template(_)));
        for (name, bytes) in parts.iter_mut().filter(|(name, _)| is_template(name)) {
            let Ok(written) = std::str::from_utf8(bytes) else { continue };
            let mut finished = unique_ids::make_ids_unique(written, self.kept_bookmarks.as_deref());
            if let Some(repaired) = structure::repair_structure(finished.as_deref().unwrap_or(written)) {
                finished = Some(repaired);
            }
            if self.drop_empty_last_page && *name == self.main_part {
                finished = Some(drop_empty_last_page(finished.as_deref().unwrap_or(written)));
            }
            if let Some(finished) = finished {
                *bytes = Cow::Owned(finished.into_bytes());
            }
        }
    }
}

fn apply_theme_colors(parts: &mut [(String, Cow<[u8]>)], colors: &BTreeMap<String, String>) {
    if colors.is_empty() {
        return;
    }
    let Some((_, bytes)) = parts.iter_mut().find(|(name, _)| name == "word/theme/theme1.xml") else { return };
    let Ok(xml) = std::str::from_utf8(bytes) else { return };
    *bytes = Cow::Owned(styling::set_theme_colors(xml, colors).into_bytes());
}

/// Adds each image's media part, the relationship its part shows it by, and its content type.
fn add_media(parts: &mut Vec<(String, Cow<[u8]>)>, media: &[(String, MediaUse)]) {
    let mut written = BTreeSet::new();
    for (part, used) in media {
        let folder = part.rfind('/').map_or("", |at| &part[..at + 1]);
        let path = format!("{folder}{}", used.target);
        if written.insert(path.clone()) {
            parts.push((path, Cow::Owned(used.image.bytes.to_vec())));
        }

        let relationship = format!("<Relationship Id=\"{}\" Type=\"{IMAGE_RELATIONSHIP}\" Target=\"{}\"/>", used.relationship_id, used.target);
        add_relationship(parts, part, &used.relationship_id, &relationship);

        let Some(types) = parts.iter_mut().find(|(name, _)| name == CONTENT_TYPES_PART) else { continue };
        let xml = String::from_utf8_lossy(&types.1).into_owned();
        let extension = used.image.extension.to_ascii_lowercase();
        if !xml.to_ascii_lowercase().contains(&format!("extension=\"{extension}\"")) {
            let default = format!("<Default Extension=\"{extension}\" ContentType=\"{}\"/>", image_content_type(&extension));
            types.1 = Cow::Owned(insert_before_last(&xml, "</Types>", &default).into_bytes());
        }
    }
}

/// Applies `{::stylepar}` and `{::stylerun}` changes to the styles part.
fn restyle_definitions(parts: &mut [(String, Cow<[u8]>)], definitions: &[(styling::StyleKind, Option<String>, styling::Style)]) {
    if definitions.is_empty() {
        return;
    }
    let Some(styles) = parts.iter_mut().find(|(name, _)| name == STYLES_PART) else { return };
    let mut xml = String::from_utf8_lossy(&styles.1).into_owned();
    for (kind, id, style) in definitions {
        xml = styling::style_definition(&xml, *kind, id.as_deref(), style);
    }
    styles.1 = Cow::Owned(xml.into_bytes());
}

/// Each part's external links, as relationships its hyperlinks name.
fn add_links(parts: &mut Vec<(String, Cow<[u8]>)>, links: &[(String, Vec<String>)]) {
    for (part, addresses) in links {
        for (index, address) in addresses.iter().enumerate() {
            let id = rich_text::link_relationship_id(index);
            let mut target = String::new();
            xml::escape(address, &mut target);
            let relationship = format!("<Relationship Id=\"{id}\" Type=\"{HYPERLINK_RELATIONSHIP}\" Target=\"{target}\" TargetMode=\"External\"/>");
            add_relationship(parts, part, &id, &relationship);
        }
    }
}

/// Adds `relationship` to `part`'s relationships part, creating it when missing, unless one
/// with `id` is there.
pub(crate) fn add_relationship(parts: &mut Vec<(String, Cow<[u8]>)>, part: &str, id: &str, relationship: &str) {
    let folder = part.rfind('/').map_or("", |at| &part[..at + 1]);
    let rels_path = format!("{folder}_rels/{}.rels", &part[folder.len()..]);
    let rels = match parts.iter().position(|(name, _)| *name == rels_path) {
        Some(index) => &mut parts[index].1,
        None => {
            let empty = format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"{RELATIONSHIPS_NAMESPACE}\"></Relationships>");
            parts.push((rels_path, Cow::Owned(empty.into_bytes())));
            &mut parts.last_mut().expect("just pushed").1
        }
    };
    let xml = String::from_utf8_lossy(rels).into_owned();
    if !xml.contains(&format!("Id=\"{id}\"")) {
        *rels = Cow::Owned(insert_before_last(&xml, "</Relationships>", relationship).into_bytes());
    }
}

pub(crate) fn insert_before_last(xml: &str, close: &str, added: &str) -> String {
    match xml.rfind(close) {
        Some(at) => format!("{}{added}{}", &xml[..at], &xml[at..]),
        None => xml.to_string(),
    }
}

pub(crate) fn image_content_type(extension: &str) -> String {
    match extension {
        "jpg" | "jpeg" => "image/jpeg".to_string(),
        "svg" => "image/svg+xml".to_string(),
        "tif" | "tiff" => "image/tiff".to_string(),
        other => format!("image/{other}"),
    }
}
