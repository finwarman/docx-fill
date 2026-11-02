//! A vertical range spanning rows repeats its columns in each row separately.

use super::{Block, BlockKind, CELL, FillError, LoopPageBreaks, Part, ROW, Tag, VERTICAL_START, find_enclosing};
use crate::xml::{ElementKind, read_attribute};

const TABLE: &str = "w:tbl";

struct Cell {
    start: usize,
    end: usize,
    column: usize,
    span: usize,
}

pub(super) fn column_blocks(parts: &[Part], tags: &[Tag], opening: usize, closing: usize) -> Result<Option<Vec<Block>>, FillError> {
    let (open, close) = (&tags[opening], &tags[closing]);
    if !open.text.starts_with(VERTICAL_START) || find_enclosing(parts, open.part, ROW) == find_enclosing(parts, close.part, ROW) {
        return Ok(None);
    }
    let invalid = || FillError::new(format!("The vertical loop \"{}\" must select matching columns of one table", open.name));
    let table = find_enclosing(parts, open.part, TABLE).ok_or_else(invalid)?;
    if find_enclosing(parts, close.part, TABLE) != Some(table) {
        return Err(invalid());
    }

    let rows = read_cells(parts, table);
    let contains = |cell: &Cell, at| cell.start < at && at < cell.end;
    let first = rows.iter().position(|row| row.iter().any(|cell| contains(cell, open.part))).ok_or_else(invalid)?;
    let last = rows.iter().position(|row| row.iter().any(|cell| contains(cell, close.part))).ok_or_else(invalid)?;
    let from = rows[first].iter().find(|cell| contains(cell, open.part)).ok_or_else(invalid)?.column;
    let last_cell = rows[last].iter().find(|cell| contains(cell, close.part)).ok_or_else(invalid)?;
    let to = last_cell.column.saturating_add(last_cell.span);
    if first > last || from >= to {
        return Err(invalid());
    }

    let mut blocks = Vec::new();
    for row in &rows[first..=last] {
        let start = row.iter().find(|cell| cell.column == from).ok_or_else(invalid)?.start;
        let end = row.iter().find(|cell| cell.column.saturating_add(cell.span) == to).ok_or_else(invalid)?.end;
        blocks.push(Block {
            kind: BlockKind::Loop { name: open.name.clone(), is_inverted: false, page_breaks: LoopPageBreaks::default() },
            start,
            end,
            body_start: start,
            body_end: end,
            children: Vec::new(),
            open_tag: opening,
        });
    }
    Ok(Some(blocks))
}

/// Read only this table's cells. Nested tables belong to their containing cell.
fn read_cells(parts: &[Part], table: (usize, usize)) -> Vec<Vec<Cell>> {
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    let mut depth = 0;
    let mut column = 0usize;
    let mut cell: Option<Cell> = None;
    for (at, part) in parts.iter().enumerate().take(table.1 + 1).skip(table.0) {
        let Part::Element { kind, name, raw } = part else { continue };
        if name == TABLE {
            match kind {
                ElementKind::Open => depth += 1,
                ElementKind::Close => depth -= 1,
                _ => {}
            }
            continue;
        }
        if depth != 1 {
            continue;
        }
        match (name.as_str(), kind) {
            (ROW, ElementKind::Open) => {
                rows.push(Vec::new());
                column = 0;
            }
            (CELL, ElementKind::Open) => cell = Some(Cell { start: at, end: 0, column, span: 1 }),
            (CELL, ElementKind::Close) => {
                if let (Some(mut current), Some(row)) = (cell.take(), rows.last_mut()) {
                    current.end = at + 1;
                    column = current.column.saturating_add(current.span);
                    row.push(current);
                }
            }
            ("w:gridSpan", ElementKind::Open | ElementKind::SelfClosing) => {
                if let Some(cell) = &mut cell {
                    cell.span = read_attribute(raw, "w:val").and_then(|value| value.parse().ok()).unwrap_or(1).max(1);
                }
            }
            ("w:gridBefore", ElementKind::Open | ElementKind::SelfClosing) => {
                column = read_attribute(raw, "w:val").and_then(|value| value.parse().ok()).unwrap_or(0);
            }
            _ => {}
        }
    }
    rows
}
