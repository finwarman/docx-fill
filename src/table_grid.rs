//! Refits the grid of tables whose cell loops changed how many columns their rows have.

use std::collections::BTreeMap;

use crate::properties::set_attribute;
use crate::TableResize;
use crate::xml::{ElementKind, Token, read_attribute, tokenize};

/// `xml` with each table whose widest row no longer matches its `w:tblGrid` given a grid of that
/// many columns, keeping the table's width. Column widths follow the widest row's cell widths,
/// scaled with every cell width in the table. `None` when every grid matches.
pub(crate) fn fit_table_grids(xml: &str, resize: TableResize) -> Option<String> {
    let tokens = tokenize(xml).ok()?;
    let mut tables: Vec<Table> = Vec::new();
    let mut edits = Edits::default();
    for (index, token) in tokens.iter().enumerate() {
        let Token::Element { kind, name, raw } = token else { continue };
        let is_start = matches!(kind, ElementKind::Open | ElementKind::SelfClosing);
        match (*name, is_start) {
            ("w:tbl", true) => tables.push(Table::default()),
            ("w:tbl", false) => {
                if let Some(table) = tables.pop() {
                    table.refit(&mut edits, resize);
                }
            }
            _ => {
                let Some(table) = tables.last_mut() else { continue };
                table.read(index, name, raw, is_start);
            }
        }
    }
    if edits.is_empty() {
        return None;
    }

    let mut out = String::with_capacity(xml.len());
    let mut skip_to = 0;
    for (index, token) in tokens.iter().enumerate() {
        if index < skip_to {
            continue;
        }
        let (Token::Element { raw, .. } | Token::Text(raw)) = token;
        match edits.get(&index) {
            Some(Edit::Replace(written)) => out.push_str(written),
            Some(Edit::ReplaceThrough(last, written)) => {
                out.push_str(written);
                skip_to = last + 1;
            }
            None => out.push_str(raw),
        }
    }
    Some(out)
}

enum Edit {
    Replace(String),
    /// Replaces the tokens from this one through the given one.
    ReplaceThrough(usize, String),
}

type Edits = BTreeMap<usize, Edit>;

#[derive(Default)]
struct Table {
    /// The `<w:tblGrid>` start tag, its index, and the index of the first token after its columns.
    grid: Option<(String, usize, Option<usize>)>,
    /// Inside `<w:tblGridChange>`, whose old grid is kept as written.
    is_in_change: bool,
    widths: Vec<Option<u64>>,
    rows: Vec<Vec<Cell>>,
    table_width: Option<(usize, String)>,
}

struct Cell {
    span: usize,
    /// A width in twips, and its `<w:tcW>` token.
    width: Option<(u64, usize, String)>,
}

impl Table {
    /// Reads one token of this table, outside any table nested in it.
    fn read(&mut self, index: usize, name: &str, raw: &str, is_start: bool) {
        if self.is_in_change {
            self.is_in_change = name != "w:tblGridChange" || is_start;
            return;
        }
        match (name, is_start) {
            ("w:tblW", true) => self.table_width = Some((index, raw.to_string())),
            ("w:tblGrid", true) if !raw.ends_with("/>") => self.grid = Some((raw.to_string(), index, None)),
            ("w:tblGridChange", true) | ("w:tblGrid", false) => {
                self.is_in_change = name == "w:tblGridChange" && !raw.ends_with("/>");
                if let Some(grid) = &mut self.grid {
                    grid.2.get_or_insert(index);
                }
            }
            ("w:gridCol", true) => self.widths.push(read_attribute(raw, "w:w").and_then(|width| width.parse().ok())),
            ("w:tr", true) => self.rows.push(Vec::new()),
            ("w:tc", true) => {
                if let Some(row) = self.rows.last_mut() {
                    row.push(Cell { span: 1, width: None });
                }
            }
            ("w:gridSpan", true) => {
                if let Some(cell) = self.last_cell() {
                    cell.span = read_attribute(raw, "w:val").and_then(|span| span.parse().ok()).unwrap_or(1).max(1);
                }
            }
            ("w:tcW", true) if read_attribute(raw, "w:type").is_none_or(|kind| kind == "dxa") => {
                let width = read_attribute(raw, "w:w").and_then(|width| width.parse().ok());
                if let (Some(cell), Some(width)) = (self.last_cell(), width) {
                    cell.width = Some((width, index, raw.to_string()));
                }
            }
            _ => {}
        }
    }

    fn last_cell(&mut self) -> Option<&mut Cell> {
        self.rows.last_mut()?.last_mut()
    }

    fn refit(self, edits: &mut Edits, resize: TableResize) {
        let Some((open, start, Some(end))) = self.grid else { return };
        let columns = |row: &Vec<Cell>| row.iter().map(|cell| cell.span).sum::<usize>();
        let Some(widest) = self.rows.iter().max_by_key(|row| columns(row)) else { return };
        let count = columns(widest);
        if count == 0 || count == self.widths.len() {
            return;
        }

        // Each grid column takes an equal share of the widest row's cell over it.
        let known: Vec<u64> = self.widths.iter().flatten().copied().collect();
        let fallback = (!known.is_empty()).then(|| known.iter().sum::<u64>() / known.len() as u64);
        let mut widths: Vec<Option<u64>> = Vec::with_capacity(count);
        for cell in widest {
            let share = cell.width.as_ref().map(|(width, ..)| width / cell.span as u64).or(fallback);
            widths.extend(std::iter::repeat_n(share, cell.span));
        }

        // Keep the table's width, scaling cell widths with the grid.
        let table_width: u64 = known.iter().sum();
        let new_width: u64 = widths.iter().flatten().sum();
        let scale = |width: u64| if resize == TableResize::KeepWidth && table_width > 0 && new_width > 0 { width * table_width / new_width } else { width };
        if resize == TableResize::KeepColumns
            && let Some((index, raw)) = self.table_width {
            edits.insert(index, Edit::Replace(set_attribute(&set_attribute(&raw, "w:type", "dxa"), "w:w", &new_width.to_string())));
        }
        let mut grid = open;
        for width in &widths {
            match width {
                Some(width) => grid.push_str(&format!("<w:gridCol w:w=\"{}\"/>", scale(*width))),
                None => grid.push_str("<w:gridCol/>"),
            }
        }
        edits.insert(start, Edit::ReplaceThrough(end - 1, grid));
        for (width, index, raw) in self.rows.iter().flatten().filter_map(|cell| cell.width.as_ref()) {
            edits.insert(*index, Edit::Replace(set_attribute(raw, "w:w", &scale(*width).to_string())));
        }
    }
}
