//! `{:table name}` builds a table from data, and `{:#grid name}` … `{:/grid}` lays items out in a
//! grid of cells. Built from the WordprocessingML spec and the table module's documented options;
//! where the docs are silent (default borders, grid order and overflow) the choice is noted.

use serde_json::Value;

use crate::properties::{RUN_ORDER, set_child};
use crate::render::print_value;
use crate::xml::escape;

// Table widths and heights in pixels are 75 to the inch; Word's twips are 1440.
const TWIPS_PER_TABLE_PX: f64 = 1440.0 / 75.0;
// Page sizes reach the renderer in 96-per-inch pixels.
const TWIPS_PER_PAGE_PX: f64 = 1440.0 / 96.0;
const PAGE_BREAK: &str = "<w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>";
// Word merges two tables with nothing between them, so chunks keep an empty paragraph apart.
const SEPARATOR: &str = "<w:p/>";
const SIDES: [(&str, &str); 6] = [("top", "top"), ("left", "left"), ("bottom", "bottom"), ("right", "right"), ("insideh", "insideH"), ("insidev", "insideV")];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TableValidation { #[default] Error, Ignore }

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TableResize { #[default] KeepWidth, KeepColumns }

/// Parsing and sizing rules shared by table tags.
#[derive(Debug, Clone, Default)]
pub struct TableOptions {
    pub paragraph_validation: TableValidation,
    pub resize: TableResize,
}

/// How a table or grid is sized, bordered and split, from its value's options.
pub(crate) struct Layout {
    /// Column widths in twips.
    columns: Vec<u64>,
    /// Row height in twips, and whether it is exact.
    height: Option<(u64, bool)>,
    borders: Option<String>,
    indent: Option<i64>,
    cell_margin: Option<String>,
    is_fixed: bool,
    align: Option<&'static str>,
    spacing_after: Option<u64>,
    page_break_between: bool,
    final_page_break: bool,
    fixed_columns: Vec<Value>,
}

/// What a table or grid needs from the page: its body width and height in 96-per-inch pixels.
pub(crate) type Body = (f64, f64);

impl Layout {
    /// The layout `value` asks for, with `count` columns and borders by default when
    /// `has_borders`.
    fn read(value: &Value, count: usize, body: Body, has_borders: bool) -> Layout {
        let (body_width, body_height) = (body.0 * TWIPS_PER_PAGE_PX, body.1 * TWIPS_PER_PAGE_PX);
        let length = |value: &Value, whole: f64| -> Option<f64> {
            match value {
                Value::Number(number) => number.as_f64().map(|px| px * TWIPS_PER_TABLE_PX),
                Value::String(text) => text.trim().strip_suffix('%')?.trim().parse::<f64>().ok().map(|percent| whole * percent / 100.0),
                _ => None,
            }
        };

        let count = count.max(1);
        let listed: Option<Vec<u64>> = value.get("widths").and_then(Value::as_array).map(|widths| {
            (0..count).map(|index| widths.get(index).and_then(|width| length(width, body_width)).unwrap_or(body_width / count as f64).round() as u64).collect()
        });
        let total = value.get("totalWidth").and_then(|total| length(total, body_width)).unwrap_or(body_width);
        let columns = listed.unwrap_or_else(|| vec![(total / count as f64).round() as u64; count]);

        let is_exact = value.get("heightRule").and_then(Value::as_str) == Some("exact");
        let height = value.get("height").and_then(|height| length(height, body_height)).map(|twips| (twips.round() as u64, is_exact));

        Layout {
            columns,
            height,
            borders: read_borders(value.get("border"), has_borders),
            indent: value.get("indent").and_then(Value::as_f64).map(|dxa| dxa.round() as i64),
            cell_margin: value.get("cellMargin").and_then(Value::as_object).map(|margins| {
                ["top", "left", "bottom", "right"]
                    .iter()
                    .filter_map(|side| Some(format!("<w:{side} w:w=\"{}\" w:type=\"dxa\"/>", margins.get(*side)?.as_f64()?.round() as i64)))
                    .collect()
            }),
            is_fixed: value.get("layout").and_then(Value::as_str) == Some("fixed"),
            align: value.get("align").and_then(Value::as_str).and_then(|align| match align {
                "center" => Some("center"),
                "left" | "start" => Some("left"),
                "right" | "end" => Some("right"),
                _ => None,
            }),
            spacing_after: value.get("spacingAfter").and_then(Value::as_f64).map(|dxa| dxa.round() as u64),
            page_break_between: value.get("pageBreakBetween").and_then(Value::as_bool).unwrap_or(true),
            final_page_break: value.get("finalPageBreak").and_then(Value::as_bool).unwrap_or(false),
            fixed_columns: value.get("fixedColumns").and_then(Value::as_array).cloned().unwrap_or_default(),
        }
    }

    fn write_start(&self, out: &mut String) {
        let total: u64 = self.columns.iter().sum();
        out.push_str(&format!("<w:tbl><w:tblPr><w:tblW w:w=\"{total}\" w:type=\"dxa\"/>"));
        if let Some(align) = self.align {
            out.push_str(&format!("<w:jc w:val=\"{align}\"/>"));
        }
        if let Some(indent) = self.indent {
            out.push_str(&format!("<w:tblInd w:w=\"{indent}\" w:type=\"dxa\"/>"));
        }
        if let Some(borders) = &self.borders {
            out.push_str(borders);
        }
        if self.is_fixed {
            out.push_str("<w:tblLayout w:type=\"fixed\"/>");
        }
        if let Some(margins) = &self.cell_margin {
            out.push_str(&format!("<w:tblCellMar>{margins}</w:tblCellMar>"));
        }
        out.push_str("</w:tblPr><w:tblGrid>");
        for width in &self.columns {
            out.push_str(&format!("<w:gridCol w:w=\"{width}\"/>"));
        }
        out.push_str("</w:tblGrid>");
    }

    fn write_row_start(&self, out: &mut String, is_header: bool) {
        out.push_str("<w:tr>");
        if self.height.is_none() && !is_header {
            return;
        }
        out.push_str("<w:trPr>");
        if let Some((height, is_exact)) = self.height {
            let rule = if is_exact { "exact" } else { "atLeast" };
            out.push_str(&format!("<w:trHeight w:val=\"{height}\" w:hRule=\"{rule}\"/>"));
        }
        if is_header {
            out.push_str("<w:tblHeader/>");
        }
        out.push_str("</w:trPr>");
    }

    /// The width in twips of `span` columns from `column`.
    fn width(&self, column: usize, span: usize) -> u64 {
        self.columns.iter().skip(column).take(span).sum()
    }

    /// Between two tables of one tag: a page break, or an empty paragraph.
    fn write_between(&self, out: &mut String) {
        out.push_str(if self.page_break_between { PAGE_BREAK } else { SEPARATOR });
    }

    fn write_end(&self, out: &mut String) {
        if self.final_page_break {
            out.push_str(PAGE_BREAK);
        }
    }
}

/// `<w:tblBorders>` for a `border` option: `"none"`, or `{ type, size, color }` with `type` one of
/// `all`, `inner`, `border` or `none`, and per-side `{ val, size, color }` under `left`, `right`,
/// `top`, `bottom`, `insideh` and `insidev`. `None` leaves borders to the table style.
fn read_borders(border: Option<&Value>, has_borders: bool) -> Option<String> {
    let single = |size: u64, color: &str| format!("w:val=\"single\" w:sz=\"{size}\" w:space=\"0\" w:color=\"{color}\"");
    let none = "w:val=\"nil\"".to_string();
    let mut sides: Vec<String> = match border {
        Some(Value::String(text)) if text == "none" => vec![none.clone(); SIDES.len()],
        Some(object @ Value::Object(_)) => {
            let size = object.get("size").and_then(Value::as_u64).unwrap_or(4);
            let color = object.get("color").and_then(Value::as_str).map_or_else(|| "000000".to_string(), read_color);
            let (outer, inner) = match object.get("type").and_then(Value::as_str) {
                Some("none") => (false, false),
                Some("inner") => (false, true),
                Some("border") => (true, false),
                _ => (true, true),
            };
            (0..SIDES.len()).map(|index| if (index < 4 && outer) || (index >= 4 && inner) { single(size, &color) } else { none.clone() }).collect()
        }
        _ if has_borders => vec![single(4, "000000"); SIDES.len()],
        _ => return None,
    };

    if let Some(object) = border.and_then(Value::as_object) {
        for (index, (key, _)) in SIDES.iter().enumerate() {
            let Some(side) = object.get(*key) else { continue };
            sides[index] = match side {
                Value::String(text) if text == "none" => none.clone(),
                Value::Object(_) => {
                    let value = side.get("val").and_then(Value::as_str).unwrap_or("single");
                    let size = side.get("size").and_then(Value::as_u64).unwrap_or(4);
                    let color = side.get("color").and_then(Value::as_str).map_or_else(|| "000000".to_string(), read_color);
                    format!("w:val=\"{}\" w:sz=\"{size}\" w:space=\"0\" w:color=\"{color}\"", attribute(value))
                }
                _ => continue,
            };
        }
    }
    let written: String = SIDES.iter().zip(&sides).map(|((_, name), attributes)| format!("<w:{name} {attributes}/>")).collect();
    Some(format!("<w:tblBorders>{written}</w:tblBorders>"))
}

fn read_color(color: &str) -> String {
    attribute(color.trim().trim_start_matches('#')).to_ascii_uppercase()
}

fn attribute(text: &str) -> String {
    let mut out = String::new();
    escape(text, &mut out);
    out
}

/// A cell's or row kind's look: `shade`, `textAlign`, `verticalAlign` and raw `runProperties`.
#[derive(Clone, Default)]
struct CellStyle {
    shade: Option<String>,
    align: Option<&'static str>,
    vertical_align: Option<&'static str>,
    run_properties: String,
}

impl CellStyle {
    fn read(value: Option<&Value>) -> CellStyle {
        let Some(value) = value else { return CellStyle::default() };
        let text = |key: &str| value.get(key).and_then(Value::as_str);
        CellStyle {
            shade: text("shade").map(read_color),
            align: text("textAlign").and_then(|align| match align {
                "left" | "start" => Some("left"),
                "center" => Some("center"),
                "right" | "end" => Some("right"),
                "justify" | "both" => Some("both"),
                _ => None,
            }),
            vertical_align: text("verticalAlign").and_then(|align| match align {
                "top" => Some("top"),
                "center" => Some("center"),
                "bottom" => Some("bottom"),
                _ => None,
            }),
            run_properties: text("runProperties").unwrap_or_default().to_string(),
        }
    }

    /// This style with `over`'s settings on top.
    fn and(&self, over: &CellStyle) -> CellStyle {
        CellStyle {
            shade: over.shade.clone().or_else(|| self.shade.clone()),
            align: over.align.or(self.align),
            vertical_align: over.vertical_align.or(self.vertical_align),
            run_properties: if over.run_properties.is_empty() { self.run_properties.clone() } else { over.run_properties.clone() },
        }
    }
}

/// Which built-in run look a row kind adds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Header,
    Subheader,
    Normal,
}

#[derive(Clone, Copy)]
enum FixedMerge { None, Start, Continue }

struct TableCell<'a> {
    value: &'a Value,
    span: usize,
    merge: FixedMerge,
}

fn table_cells<'a>(cells: &'a [Value], fixed: &'a [Value], merge: FixedMerge) -> Vec<TableCell<'a>> {
    let mut source = cells.iter();
    let mut next = source.next();
    let mut column = 0;
    let mut out = Vec::new();
    while next.is_some() || column < fixed.len() {
        if let Some(value) = fixed.get(column).filter(|value| !value.is_null()) {
            out.push(TableCell { value: if matches!(merge, FixedMerge::Continue) { &Value::Null } else { value }, span: 1, merge });
            column += 1;
            continue;
        }
        let value = next.unwrap_or(&Value::Null);
        let requested = value.get("colspan").and_then(Value::as_u64).unwrap_or(1).max(1) as usize;
        let until_fixed = fixed.iter().skip(column + 1).position(|value| !value.is_null()).map(|at| at + 1);
        let span = until_fixed.map_or(requested, |gap| requested.min(gap));
        out.push(TableCell { value, span, merge: FixedMerge::None });
        column += span;
        next = source.next();
    }
    out
}

/// Writes a `{:table}` value: `{ data: [[cell]], header?, subheader?, widths?, totalWidth?,
/// height?, heightRule?, chunkSize?, pageBreakBetween?, finalPageBreak?, style?: { header, normal },
/// border?, indent?, cellMargin?, layout?, align?, spacingAfter? }`. A cell is text, a number, or
/// `{ text?, paragraphs?: [{ text, style }], colspan?, style?, border? }`. Prints nothing without
/// rows.
///
/// The header is bold and repeats on each page; the subheader is italic. Borders are single black
/// lines unless `border` says otherwise.
pub(crate) fn write_table(value: &Value, body: Body, in_cell: bool) -> String {
    let Some(rows) = value.get("data").and_then(Value::as_array).filter(|rows| !rows.is_empty()) else { return String::new() };
    let header = value.get("header").and_then(Value::as_array);
    let subheader = value.get("subheader").and_then(Value::as_array);

    let fixed = value.get("fixedColumns").and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
    let row_width = |row: &Value| row.as_array().map_or(0, |cells| table_cells(cells, fixed, FixedMerge::None).iter().map(|cell| cell.span).sum());
    let count = rows.iter().map(row_width).chain(header.map(Vec::len)).chain(subheader.map(Vec::len)).max().unwrap_or(1);
    let layout = Layout::read(value, count, body, true);

    let styles = value.get("style");
    let header_style = CellStyle::read(styles.and_then(|styles| styles.get("header")));
    let normal_style = CellStyle::read(styles.and_then(|styles| styles.get("normal")));
    let chunk = value.get("chunkSize").and_then(Value::as_u64).filter(|size| *size > 0).map_or(rows.len(), |size| size as usize);

    let mut out = String::new();
    for (index, chunk) in rows.chunks(chunk).enumerate() {
        if index > 0 {
            layout.write_between(&mut out);
        }
        layout.write_start(&mut out);
        if let Some(header) = header {
            write_row(&mut out, &layout, header, RowKind::Header, &header_style, FixedMerge::None);
        }
        if let Some(subheader) = subheader {
            write_row(&mut out, &layout, subheader, RowKind::Subheader, &header_style, FixedMerge::None);
        }
        for (index, row) in chunk.iter().enumerate() {
            let merge = if chunk.len() == 1 { FixedMerge::None } else if index == 0 { FixedMerge::Start } else { FixedMerge::Continue };
            write_row(&mut out, &layout, row.as_array().map_or(&[][..], Vec::as_slice), RowKind::Normal, &normal_style, merge);
        }
        out.push_str("</w:tbl>");
    }
    layout.write_end(&mut out);
    // A cell must end with a paragraph.
    if in_cell {
        out.push_str(SEPARATOR);
    }
    out
}

fn write_row(out: &mut String, layout: &Layout, cells: &[Value], kind: RowKind, base: &CellStyle, merge: FixedMerge) {
    layout.write_row_start(out, kind == RowKind::Header);
    let mut column = 0;
    let fixed = if kind == RowKind::Normal { layout.fixed_columns.as_slice() } else { &[] };
    for placed in table_cells(cells, fixed, merge) {
        let (cell, span) = (placed.value, placed.span);
        let style = base.and(&CellStyle::read(cell.get("style")));
        out.push_str(&format!("<w:tc><w:tcPr><w:tcW w:w=\"{}\" w:type=\"dxa\"/>", layout.width(column, span)));
        if span > 1 {
            out.push_str(&format!("<w:gridSpan w:val=\"{span}\"/>"));
        }
        match placed.merge {
            FixedMerge::Start => out.push_str("<w:vMerge w:val=\"restart\"/>"),
            FixedMerge::Continue => out.push_str("<w:vMerge/>"),
            FixedMerge::None => {}
        }
        if let Some(borders) = read_borders(cell.get("border"), false) {
            out.push_str(&borders.replace("w:tblBorders", "w:tcBorders"));
        }
        if let Some(shade) = &style.shade {
            out.push_str(&format!("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{shade}\"/>"));
        }
        if let Some(align) = style.vertical_align {
            out.push_str(&format!("<w:vAlign w:val=\"{align}\"/>"));
        }
        out.push_str("</w:tcPr>");

        match cell.get("paragraphs").and_then(Value::as_array) {
            Some(paragraphs) if !paragraphs.is_empty() => {
                for paragraph in paragraphs {
                    let own = style.and(&CellStyle::read(paragraph.get("style")));
                    write_paragraph(out, layout, &cell_text(paragraph), &own, kind);
                }
            }
            _ => write_paragraph(out, layout, &cell_text(cell), &style, kind),
        }
        out.push_str("</w:tc>");
        column += span;
    }
    // Rows shorter than the table get empty cells, so every row fills the grid.
    while column < layout.columns.len() {
        out.push_str(&format!("<w:tc><w:tcPr><w:tcW w:w=\"{}\" w:type=\"dxa\"/></w:tcPr><w:p/></w:tc>", layout.width(column, 1)));
        column += 1;
    }
    out.push_str("</w:tr>");
}

/// A cell value's text: the value itself, or its `text`.
fn cell_text(cell: &Value) -> String {
    match cell {
        Value::Object(object) => object.get("text").filter(|text| !text.is_null()).map(print_value).unwrap_or_default(),
        Value::Null => String::new(),
        other => print_value(other),
    }
}

fn write_paragraph(out: &mut String, layout: &Layout, text: &str, style: &CellStyle, kind: RowKind) {
    out.push_str("<w:p>");
    if layout.spacing_after.is_some() || style.align.is_some() {
        out.push_str("<w:pPr>");
        if let Some(after) = layout.spacing_after {
            out.push_str(&format!("<w:spacing w:after=\"{after}\"/>"));
        }
        if let Some(align) = style.align {
            out.push_str(&format!("<w:jc w:val=\"{align}\"/>"));
        }
        out.push_str("</w:pPr>");
    }
    if !text.is_empty() {
        let mut properties = style.run_properties.clone();
        match kind {
            RowKind::Header => properties = set_child(&properties, "w:b", "<w:b/>", RUN_ORDER),
            RowKind::Subheader => properties = set_child(&properties, "w:i", "<w:i/>", RUN_ORDER),
            RowKind::Normal => {}
        }
        out.push_str("<w:r>");
        if !properties.is_empty() {
            out.push_str(&format!("<w:rPr>{properties}</w:rPr>"));
        }
        for (index, line) in text.split('\n').enumerate() {
            if index > 0 {
                out.push_str("<w:br/>");
            }
            out.push_str("<w:t xml:space=\"preserve\">");
            escape(line.strip_suffix('\r').unwrap_or(line), out);
            out.push_str("</w:t>");
        }
        out.push_str("</w:r>");
    }
    out.push_str("</w:p>");
}

/// A grid's value: `{ size: [rows, columns], data: [item], widths?, height?, heightRule?, border?,
/// pageBreakBetween?, … }`, read as a table's layout without borders by default.
pub(crate) struct Grid<'a> {
    pub layout: Layout,
    pub rows: usize,
    pub items: &'a [Value],
}

impl<'a> Grid<'a> {
    /// The grid `value` asks for; `None` without a size of two counts and a data list.
    pub(crate) fn read(value: &'a Value, body: Body) -> Option<Grid<'a>> {
        let size = value.get("size")?.as_array().filter(|size| size.len() == 2)?;
        let rows = size[0].as_u64().filter(|rows| *rows > 0)? as usize;
        let columns = size[1].as_u64().filter(|columns| *columns > 0)? as usize;
        let items = value.get("data")?.as_array()?;
        Some(Grid { layout: Layout::read(value, columns, body, false), rows, items })
    }

    pub(crate) fn columns(&self) -> usize {
        self.layout.columns.len()
    }

    /// Opens table `index` (from 0), separated from the one before it.
    pub(crate) fn write_table_start(&self, out: &mut String, index: usize) {
        if index > 0 {
            self.layout.write_between(out);
        }
        self.layout.write_start(out);
    }

    pub(crate) fn write_row_start(&self, out: &mut String) {
        self.layout.write_row_start(out, false);
    }

    pub(crate) fn write_cell_start(&self, out: &mut String, column: usize) {
        out.push_str(&format!("<w:tc><w:tcPr><w:tcW w:w=\"{}\" w:type=\"dxa\"/></w:tcPr>", self.layout.width(column, 1)));
    }

    pub(crate) fn write_end(&self, out: &mut String) {
        self.layout.write_end(out);
    }
}
