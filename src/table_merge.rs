//! `{:merge-cells-col 1,2}` and `{:merge-cells-row}`: merges neighbouring cells of the tag's table
//! that print the same text. Built from the WordprocessingML spec: a vertical merge is `<w:vMerge>`
//! on each cell of a column run, a horizontal one a `<w:gridSpan>` on its first cell.

use crate::cell_styling::find_element_start;
use crate::properties::{CELL_ORDER, edit_properties, find_element_end, read_child, set_child};
use crate::xml::{ElementKind, Token, read_attribute, tokenize, unescape};

const TABLE: &str = "w:tbl";
const ROW: &str = "w:tr";
const CELL: &str = "w:tc";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Merge {
    /// Merges each listed column's (1-based) runs of equal cells down the table. With `is_rtl`, a
    /// cell merges only when the listed column before it merged in the same row.
    Columns { columns: Vec<usize>, is_rtl: bool },
    /// Merges runs of equal cells across each listed row (1-based, negative from the end), or
    /// every row.
    Rows(Option<Vec<i64>>),
}

/// The merge a tag's text after its prefix asks for, such as `1,2`, `rtl 1,2` or `-1`.
pub(crate) fn read_merge(is_columns: bool, text: &str) -> Merge {
    let text = text.trim();
    let (is_rtl, list) = match text.strip_prefix("rtl") {
        Some(rest) if is_columns => (true, rest.trim()),
        _ => (false, text),
    };
    let numbers: Vec<i64> = list.split(',').filter_map(|each| each.trim().parse().ok()).collect();
    if is_columns {
        let columns = numbers.into_iter().filter(|number| *number > 0).map(|number| number as usize).collect();
        return Merge::Columns { columns, is_rtl };
    }
    Merge::Rows((!numbers.is_empty()).then_some(numbers))
}

struct Cell {
    start: usize,
    end: usize,
    /// The grid column it starts in, from 0.
    column: usize,
    span: usize,
    text: String,
}

/// `table`, a whole `<w:tbl>`, with each merge applied in turn.
pub(crate) fn merge_table(table: &str, merges: &[Merge]) -> String {
    let mut table = table.to_string();
    for merge in merges {
        let rows = read_rows(&table);
        let edits = match merge {
            Merge::Columns { columns, is_rtl } => merge_columns(&rows, columns, *is_rtl, &table),
            Merge::Rows(only) => merge_rows(&rows, only.as_deref(), &table),
        };
        table = apply(&table, edits);
    }
    table
}

/// The table's own rows and cells, not those of tables nested in it.
fn read_rows(table: &str) -> Vec<Vec<Cell>> {
    let Ok(tokens) = tokenize(table) else { return Vec::new() };
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    let mut tables = 0usize;
    let mut properties = 0usize;
    let mut at = 0;
    let mut cell: Option<(usize, String, usize)> = None;
    let mut is_in_text = false;
    for token in tokens {
        let length = match token {
            Token::Element { raw, .. } => raw.len(),
            Token::Text(text) => text.len(),
        };
        if tables == 1 && let Token::Element { kind, name: "w:tcPr", .. } = token {
            match kind {
                ElementKind::Open => properties += 1,
                ElementKind::Close => properties = properties.saturating_sub(1),
                _ => {}
            }
        }
        match token {
            Token::Element { kind: ElementKind::Open, name: TABLE, .. } => tables += 1,
            Token::Element { kind: ElementKind::Close, name: TABLE, .. } => tables -= 1,
            Token::Element { kind: ElementKind::Open, name: ROW, .. } if tables == 1 => rows.push(Vec::new()),
            Token::Element { kind: ElementKind::Open, name: CELL, .. } if tables == 1 => cell = Some((at, String::new(), 1)),
            Token::Element { kind: ElementKind::Close, name: CELL, .. } if tables == 1 => {
                if let (Some((start, text, span)), Some(row)) = (cell.take(), rows.last_mut()) {
                    let column = row.last().map_or(0, |last| last.column + last.span);
                    row.push(Cell { start, end: at + length, column, span, text });
                }
            }
            Token::Element { kind: ElementKind::Open | ElementKind::SelfClosing, name: "w:gridSpan", raw } if tables == 1 && properties == 1 => {
                if let Some(cell) = &mut cell {
                    cell.2 = read_attribute(raw, "w:val").and_then(|span| span.parse().ok()).unwrap_or(1).max(1);
                }
            }
            Token::Element { kind, name: "w:t", .. } => is_in_text = kind == ElementKind::Open,
            Token::Text(text) if is_in_text => {
                if let Some(cell) = &mut cell {
                    cell.1.push_str(&unescape(text));
                }
            }
            _ => {}
        }
        at += length;
    }
    rows
}

/// A cell's replacement, or `None` to remove it.
type Edit = (usize, usize, Option<String>);

fn merge_columns(rows: &[Vec<Cell>], columns: &[usize], is_rtl: bool, table: &str) -> Vec<Edit> {
    let mut edits = Vec::new();
    // Rows whose cell merged into the one above, in the last column handled.
    let mut merged_before: Option<Vec<bool>> = None;
    for &column in columns {
        let mut merged = vec![false; rows.len()];
        let at = |row: &[Cell]| row.iter().position(|cell| cell.column == column - 1);
        let mut first = 0;
        while first < rows.len() {
            let Some(top) = at(&rows[first]) else {
                first += 1;
                continue;
            };
            let top_cell = &rows[first][top];
            let mut last = first;
            while let Some(next) = rows.get(last + 1).and_then(|row| at(row).map(|index| &row[index])) {
                let is_allowed = !is_rtl || merged_before.as_ref().is_none_or(|before| before[last + 1]);
                if top_cell.text.is_empty() || next.text != top_cell.text || next.span != top_cell.span || !is_allowed {
                    break;
                }
                last += 1;
            }
            if last > first {
                let cell = &table[top_cell.start..top_cell.end];
                edits.push((top_cell.start, top_cell.end, Some(set_vertical_merge(cell, true))));
                for row in first + 1..=last {
                    let below = &rows[row][at(&rows[row]).expect("checked above")];
                    let cell = set_vertical_merge(&table[below.start..below.end], false);
                    edits.push((below.start, below.end, Some(empty_cell(&cell))));
                    merged[row] = true;
                }
            }
            first = last + 1;
        }
        merged_before = Some(merged);
    }
    edits
}

fn merge_rows(rows: &[Vec<Cell>], only: Option<&[i64]>, table: &str) -> Vec<Edit> {
    let count = rows.len() as i64;
    let is_listed = |index: usize| {
        only.is_none_or(|listed| listed.iter().any(|&number| if number < 0 { count + number == index as i64 } else { number - 1 == index as i64 }))
    };
    let mut edits = Vec::new();
    for (_, row) in rows.iter().enumerate().filter(|(index, _)| is_listed(*index)) {
        let mut first = 0;
        while first < row.len() {
            let mut last = first;
            while row.get(last + 1).is_some_and(|next| !row[first].text.is_empty() && next.text == row[first].text) {
                last += 1;
            }
            if last > first {
                let cells = &row[first..=last];
                let span: usize = cells.iter().map(|cell| cell.span).sum();
                let widths: Option<u64> = cells.iter().map(|cell| read_dxa_width(&table[cell.start..cell.end])).sum();
                let cell = &table[cells[0].start..cells[0].end];
                edits.push((cells[0].start, cells[0].end, Some(set_span(cell, span, widths))));
                edits.extend(cells[1..].iter().map(|cell| (cell.start, cell.end, None)));
            }
            first = last + 1;
        }
    }
    edits
}

fn set_vertical_merge(cell: &str, is_restart: bool) -> String {
    let merge = if is_restart { "<w:vMerge w:val=\"restart\"/>" } else { "<w:vMerge/>" };
    edit_properties(cell, "w:tcPr", |inner| set_child(inner, "w:vMerge", merge, CELL_ORDER))
}

fn set_span(cell: &str, span: usize, width: Option<u64>) -> String {
    edit_properties(cell, "w:tcPr", |inner| {
        let inner = set_child(inner, "w:gridSpan", &format!("<w:gridSpan w:val=\"{span}\"/>"), CELL_ORDER);
        match width {
            Some(width) => set_child(&inner, "w:tcW", &format!("<w:tcW w:w=\"{width}\" w:type=\"dxa\"/>"), CELL_ORDER),
            None => inner,
        }
    })
}

/// The cell's `<w:tcW>` in twips, when it gives one.
fn read_dxa_width(cell: &str) -> Option<u64> {
    let open_end = cell.find('>')? + 1;
    let properties = read_child(&cell[open_end..], "w:tcPr")?;
    let mut depth = 0usize;
    for token in tokenize(properties).ok()? {
        let Token::Element { kind, name, raw } = token else { continue };
        if depth == 0 && name == "w:tcW" && matches!(kind, ElementKind::Open | ElementKind::SelfClosing) {
            return (read_attribute(raw, "w:type") == Some("dxa")).then(|| read_attribute(raw, "w:w")?.parse().ok()).flatten();
        }
        match kind {
            ElementKind::Open => depth += 1,
            ElementKind::Close => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    None
}

/// A merged-away cell: its properties and one empty paragraph, which a cell must hold.
fn empty_cell(cell: &str) -> String {
    let Some(start) = find_element_start(cell, "<w:tcPr") else { return cell.to_string() };
    let Some(end) = find_element_end(cell, start, "w:tcPr") else { return cell.to_string() };
    format!("{}<w:p/></w:tc>", &cell[..end])
}

fn apply(table: &str, mut edits: Vec<Edit>) -> String {
    edits.sort_by_key(|edit| edit.0);
    let mut out = String::with_capacity(table.len());
    let mut at = 0;
    for (start, end, replacement) in edits {
        if start < at {
            continue;
        }
        out.push_str(&table[at..start]);
        if let Some(replacement) = replacement {
            out.push_str(&replacement);
        }
        at = end;
    }
    out.push_str(&table[at..]);
    out
}
