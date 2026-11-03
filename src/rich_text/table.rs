//! HTML table dimensions and spacing mapped to Word's preferred table/cell widths.

use super::*;

const DEFAULT_PADDING_TWIPS: [i64; 4] = [55, 45, 55, 55];
const DEFAULT_BORDER_EIGHTHS: u32 = 2;
const DEFAULT_WIDTH_TWIPS: u32 = 8640;
const EIGHTHS_PER_POINT: f64 = 8.0;
const TWIPS_PER_CSS_PIXEL: f64 = TWIPS_PER_POINT * POINTS_PER_CSS_PIXEL;
const BORDER_SIDES: [&str; 6] = ["top", "left", "bottom", "right", "insideH", "insideV"];

#[derive(Debug, Clone, Copy)]
enum Length { Twips(u32), Percent(f64) }

impl Length {
    fn resolve(self, width: u32) -> u32 {
        match self {
            Self::Twips(value) => value,
            Self::Percent(percent) => (f64::from(width) * percent / 100.0).round().clamp(1.0, f64::from(i32::MAX)) as u32,
        }
    }
}

#[derive(Debug, Clone)]
struct Border { style: &'static str, size: u32, color: String }

impl Default for Border {
    fn default() -> Self { Self { style: "single", size: DEFAULT_BORDER_EIGHTHS, color: "auto".into() } }
}

#[derive(Debug, Clone, Copy, Default)]
enum Layout { #[default] Auto, Fixed }

#[derive(Debug, Clone)]
pub(super) struct TableLayout {
    width: Option<Length>,
    columns: Vec<Option<Length>>,
    margins: [i64; 4],
    padding: [i64; 4],
    borders: [Border; 6],
    align: &'static str,
    layout: Layout,
}

impl Default for TableLayout {
    fn default() -> Self {
        Self { width: None, columns: Vec::new(), margins: [0; 4], padding: DEFAULT_PADDING_TWIPS, borders: std::array::from_fn(|_| Border::default()), align: "left", layout: Layout::Auto }
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct CellLayout {
    width: Option<Length>,
    margins: [Option<i64>; 4],
    borders: [Option<Border>; 4],
    vertical: Option<&'static str>,
}

fn read_length(value: &str) -> Option<Length> {
    if let Some(percent) = value.trim().strip_suffix('%') {
        let percent = percent.trim().parse::<f64>().ok()?;
        return (percent.is_finite() && percent > 0.0).then_some(Length::Percent(percent));
    }
    length_twips(value, DEFAULT_FONT_POINTS).filter(|value| *value > 0).map(|value| Length::Twips(value as u32))
}

fn length_twips(value: &str, font: f64) -> Option<i64> {
    if let Some(length) = css_twips(value, font) { return Some(length); }
    let value = value.trim();
    let units = [("in", 1440.0), ("cm", 1440.0 / 2.54), ("mm", 1440.0 / 25.4), ("pc", 240.0)];
    units.into_iter().find_map(|(unit, factor)| {
        let number = value.strip_suffix(unit)?.trim().parse::<f64>().ok()? * factor;
        (number.is_finite() && number.abs() <= f64::from(i32::MAX)).then(|| number.round() as i64)
    })
}

fn read_width(attributes: &[(String, String)]) -> Option<Length> {
    if let Some(value) = read_style(attributes, "width") { return read_length(value); }
    let value = read_attribute(attributes, "width")?;
    read_length(value).or_else(|| value.parse::<f64>().ok().filter(|value| value.is_finite() && *value > 0.0)
        .map(|value| Length::Twips((value * TWIPS_PER_CSS_PIXEL).round().clamp(1.0, f64::from(i32::MAX)) as u32)))
}

fn read_box(attributes: &[(String, String)], property: &str) -> [Option<i64>; 4] {
    let font = read_style(attributes, "font-size").and_then(read_font_size).map_or(DEFAULT_FONT_POINTS, |size| f64::from(size) / 2.0);
    let short: Vec<_> = read_style(attributes, property).into_iter().flat_map(str::split_whitespace).map(|value| length_twips(value, font)).collect();
    let mut sides = match short.as_slice() {
        [a] => [*a; 4], [a, b] => [*a, *b, *a, *b], [a, b, c] => [*a, *b, *c, *b], [a, b, c, d] => [*a, *d, *c, *b], _ => [None; 4],
    };
    for (index, side) in CELL_SIDES.iter().enumerate() {
        if let Some(value) = read_style(attributes, &format!("{property}-{side}")) {
            sides[index] = length_twips(value, font);
        }
    }
    sides
}

fn border_style(value: &str) -> Option<&'static str> {
    match value {
        "none" | "hidden" => Some("nil"), "solid" => Some("single"), "dashed" => Some("dashed"), "dotted" => Some("dotted"),
        "double" => Some("double"), "inset" => Some("inset"), "outset" => Some("outset"), _ => None,
    }
}

fn border_width(value: &str) -> Option<u32> {
    let value = match value { "thin" => 1.0, "medium" => 3.0, "thick" => 5.0, _ => return css_twips(value, DEFAULT_FONT_POINTS).map(|value| ((value.max(0) as f64 / TWIPS_PER_POINT) * EIGHTHS_PER_POINT).round().clamp(0.0, 96.0) as u32) };
    Some((value * POINTS_PER_CSS_PIXEL * EIGHTHS_PER_POINT) as u32)
}

fn read_border(value: &str, mut border: Border) -> Border {
    for token in value.split_whitespace() {
        if let Some(style) = border_style(token) { border.style = style; }
        else if let Some(size) = border_width(token) { border.size = size; }
        else if let Some(color) = read_color(token) { border.color = color; }
    }
    if border.size == 0 { border.style = "nil"; }
    border
}

fn base_border(attributes: &[(String, String)], base: &Border) -> Option<Border> {
    let mut border = read_style(attributes, "border").map(|value| read_border(value, base.clone()));
    for property in ["style", "width", "color"] {
        if let Some(value) = read_style(attributes, &format!("border-{property}")) {
            border = Some(read_border(value, border.unwrap_or_else(|| base.clone())));
        }
    }
    border
}

fn read_borders(attributes: &[(String, String)], base: Border) -> [Option<Border>; 4] {
    let border = base_border(attributes, &base);
    std::array::from_fn(|index| {
        let side = CELL_SIDES[index];
        let mut result = border.clone();
        if let Some(value) = read_style(attributes, &format!("border-{side}")) {
            result = Some(read_border(value, result.unwrap_or_else(|| base.clone())));
        }
        for property in ["style", "width", "color"] {
            if let Some(value) = read_style(attributes, &format!("border-{side}-{property}")) {
                result = Some(read_border(value, result.unwrap_or_else(|| base.clone())));
            }
        }
        result
    })
}

pub(super) fn read_table(attributes: &[(String, String)]) -> TableLayout {
    let mut table = TableLayout { width: read_width(attributes), ..TableLayout::default() };
    table.margins = read_box(attributes, "margin").map(|value| value.unwrap_or(0).max(0));
    if let Some(padding) = read_attribute(attributes, "cellpadding").and_then(|value| value.parse::<f64>().ok()).filter(|value| value.is_finite() && *value >= 0.0) {
        table.padding.fill((padding * TWIPS_PER_CSS_PIXEL).round().min(f64::from(i32::MAX)) as i64);
    }
    let mut base = Border::default();
    if let Some(value) = read_attribute(attributes, "border")
        && let Ok(width) = value.parse::<u32>() {
        base.size = (width.saturating_mul(6)).min(96);
        if width == 0 { base.style = "nil"; }
    }
    if let Some(color) = read_attribute(attributes, "bordercolor").and_then(read_color) { base.color = color; }
    let general = base_border(attributes, &base).unwrap_or_else(|| base.clone());
    table.borders = std::array::from_fn(|_| general.clone());
    for (index, border) in read_borders(attributes, base).into_iter().enumerate() {
        if let Some(border) = border {
            table.borders[index] = border;
        }
    }
    table.align = read_attribute(attributes, "align").and_then(read_align).filter(|align| matches!(*align, "left" | "center" | "right")).unwrap_or("left");
    if read_style(attributes, "margin-left") == Some("auto") {
        table.align = if read_style(attributes, "margin-right") == Some("auto") { "center" } else { "right" };
    }
    if read_style(attributes, "table-layout") == Some("fixed") { table.layout = Layout::Fixed; }
    table
}

pub(super) fn read_cell(attributes: &[(String, String)]) -> CellLayout {
    CellLayout {
        width: read_width(attributes), margins: read_box(attributes, "padding").map(|value| value.map(|value| value.max(0))),
        borders: read_borders(attributes, Border::default()),
        vertical: read_style(attributes, "vertical-align").and_then(|value| match value { "top" => Some("top"), "middle" | "center" => Some("center"), "bottom" => Some("bottom"), _ => None }),
    }
}

pub(super) fn read_column(layout: &mut TableLayout, attributes: &[(String, String)]) {
    let span = read_attribute(attributes, "span").and_then(|value| value.parse::<usize>().ok()).unwrap_or(1).clamp(1, MAX_COLUMN_SPAN);
    let width = read_width(attributes);
    layout.columns.extend(std::iter::repeat_n(width, span.min(MAX_COLUMN_SPAN.saturating_sub(layout.columns.len()))));
}

fn column_lengths(rows: &[Vec<PlacedCell<'_>>], layout: &TableLayout, columns: usize) -> Vec<Option<Length>> {
    let mut lengths = vec![None; columns];
    for (target, value) in lengths.iter_mut().zip(&layout.columns) { *target = *value; }
    for row in rows {
        let mut column = 0;
        for cell in row {
            if let Some(value) = cell.layout.width {
                let value = match value {
                    Length::Twips(value) => Length::Twips((value / cell.span as u32).max(1)),
                    Length::Percent(value) => Length::Percent(value / cell.span as f64),
                };
                for target in lengths.iter_mut().skip(column).take(cell.span) { target.get_or_insert(value); }
            }
            column += cell.span;
        }
    }
    lengths
}

fn grid_widths(lengths: &[Option<Length>], width: u32) -> Vec<u32> {
    let columns = lengths.len();
    let widths: Vec<_> = lengths.iter().map(|value| value.map(|value| value.resolve(width))).collect();
    let fixed: u64 = widths.iter().flatten().map(|value| u64::from(*value)).sum();
    let free = widths.iter().filter(|value| value.is_none()).count();
    let remainder = u64::from(width).saturating_sub(fixed);
    let fallback = if free > 0 { (remainder / free as u64).max(1) } else { 1 };
    let weights: Vec<_> = widths.into_iter().map(|value| value.map(u64::from).unwrap_or(fallback)).collect();
    let sum: u128 = weights.iter().map(|value| u128::from(*value)).sum();
    let width = width.max(columns.min(i32::MAX as usize) as u32);
    let mut before = 0;
    let mut accumulated = 0;
    weights.into_iter().map(|weight| {
        accumulated += u128::from(weight);
        let after = (accumulated * u128::from(width - columns as u32) / sum) as u32;
        let value = 1 + after - before;
        before = after;
        value
    }).collect()
}

fn write_border(out: &mut String, side: &str, border: &Border) {
    if border.style == "nil" { out.push_str(&format!("<w:{side} w:val=\"nil\"/>")); return; }
    out.push_str(&format!("<w:{side} w:val=\"{}\" w:sz=\"{}\" w:space=\"0\" w:color=\"{}\"/>", border.style, border.size.max(2), border.color));
}

pub(super) fn write(out: &mut String, rows: &[Vec<Cell>], layout: &TableLayout, writer: &mut BlockWriter) {
    let rows = place_cells(rows);
    let columns = rows.iter().map(|row| row.iter().map(|cell| cell.span).sum::<usize>()).max().unwrap_or(1).max(1);
    let lengths = column_lengths(&rows, layout, columns);
    let absolute: Option<Vec<_>> = lengths.iter().map(|length| match length { Some(Length::Twips(value)) => Some(u64::from(*value)), _ => None }).collect();
    let natural = absolute.map(|values| values.into_iter().sum::<u64>().min(i32::MAX as u64) as u32);
    let width = layout.width.map(|width| width.resolve(writer.width)).or(natural)
        .unwrap_or_else(|| writer.width.saturating_sub((layout.margins[1] + layout.margins[3]).min(i64::from(i32::MAX)) as u32).min(DEFAULT_WIDTH_TWIPS)).max(columns.min(i32::MAX as usize) as u32);
    let widths = grid_widths(&lengths, width);
    let first_padding = rows.first().and_then(|row| row.first()).and_then(|cell| cell.layout.margins[1]).unwrap_or(layout.padding[1]);
    // Word aligns the first cell's text at the table indent; compensate its padding for a border aligned to the text area.
    let indent = layout.margins[1].saturating_add(first_padding);
    out.push_str(&format!("<w:tbl><w:tblPr><w:tblW w:w=\"{width}\" w:type=\"dxa\"/><w:jc w:val=\"{}\"/><w:tblInd w:w=\"{indent}\" w:type=\"dxa\"/><w:tblBorders>", layout.align));
    for (side, border) in BORDER_SIDES.iter().zip(&layout.borders) { write_border(out, side, border); }
    out.push_str("</w:tblBorders>");
    if matches!(layout.layout, Layout::Fixed) { out.push_str("<w:tblLayout w:type=\"fixed\"/>"); }
    out.push_str("<w:tblCellMar>");
    for (side, value) in CELL_SIDES.iter().zip(layout.padding) { out.push_str(&format!("<w:{side} w:w=\"{value}\" w:type=\"dxa\"/>")); }
    out.push_str("</w:tblCellMar></w:tblPr><w:tblGrid>");
    for width in &widths { out.push_str(&format!("<w:gridCol w:w=\"{width}\"/>")); }
    out.push_str("</w:tblGrid>");
    for row in rows {
        out.push_str("<w:tr>");
        let mut used = 0;
        for cell in row {
            let width = widths.iter().skip(used).take(cell.span).sum();
            write_cell(out, &cell, layout, width, writer);
            used += cell.span;
        }
        for width in widths.iter().skip(used) {
            write_cell(out, &PlacedCell { span: 1, merge: CellMerge::None, blocks: &[], layout: CellLayout::default() }, layout, *width, writer);
        }
        out.push_str("</w:tr>");
    }
    out.push_str("</w:tbl>");
}

fn write_cell(out: &mut String, cell: &PlacedCell, table: &TableLayout, width: u32, writer: &mut BlockWriter) {
    out.push_str(&format!("<w:tc><w:tcPr><w:tcW w:w=\"{width}\" w:type=\"dxa\"/>"));
    if cell.span > 1 { out.push_str(&format!("<w:gridSpan w:val=\"{}\"/>", cell.span)); }
    match cell.merge { CellMerge::Start => out.push_str("<w:vMerge w:val=\"restart\"/>"), CellMerge::Continue => out.push_str("<w:vMerge/>"), CellMerge::None => {} }
    if cell.layout.borders.iter().any(Option::is_some) {
        out.push_str("<w:tcBorders>");
        for (side, border) in CELL_SIDES.iter().zip(&cell.layout.borders) { if let Some(border) = border { write_border(out, side, border); } }
        out.push_str("</w:tcBorders>");
    }
    if cell.layout.margins.iter().any(Option::is_some) {
        out.push_str("<w:tcMar>");
        for (side, value) in CELL_SIDES.iter().zip(cell.layout.margins) { if let Some(value) = value { out.push_str(&format!("<w:{side} w:w=\"{value}\" w:type=\"dxa\"/>")); } }
        out.push_str("</w:tcMar>");
    }
    if let Some(vertical) = cell.layout.vertical { out.push_str(&format!("<w:vAlign w:val=\"{vertical}\"/>")); }
    out.push_str("</w:tcPr>");
    let padding = cell.layout.margins[1].unwrap_or(table.padding[1]).saturating_add(cell.layout.margins[3].unwrap_or(table.padding[3]));
    let outer_width = writer.width;
    writer.width = width.saturating_sub(padding.min(i64::from(i32::MAX)) as u32).max(1);
    for (index, block) in cell.blocks.iter().enumerate() {
        if follows_table(cell.blocks, index) { out.push_str("<w:p/>"); }
        write_block(out, "<w:p>", "", "", block, writer);
    }
    writer.width = outer_width;
    if cell.blocks.last().is_none_or(|block| block.table.is_some()) { out.push_str("<w:p/>"); }
    out.push_str("</w:tc>");
}
