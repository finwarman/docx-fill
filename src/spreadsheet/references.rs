//! Spreadsheet references whose row numbers change when a template row repeats.

use crate::properties::set_attribute;
use crate::xml::{ElementKind, Token, tokenize};
use crate::{FillError, FillErrorId};

pub(super) type RowMap = std::collections::BTreeMap<u32, Vec<u32>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CellCoordinate {
    pub source_row: u32,
    pub source_column: u32,
    pub target_row: u32,
    pub target_column: u32,
}

#[derive(Debug, Clone)]
pub(super) struct WorksheetMap {
    pub rows: RowMap,
    pub cells: Vec<CellCoordinate>,
    pub original_rows: Vec<u32>,
}

type CellColumnMap = std::collections::BTreeMap<(u32, u32), Vec<u32>>;

/// Reusable column indexes for formulas rewritten across many package parts.
pub(super) struct QualifiedFormulaIndex {
    sheets: std::collections::BTreeMap<String, IndexedWorksheet>,
}

struct IndexedWorksheet {
    rows: RowMap,
    original_rows: Vec<u32>,
    columns: CellColumnMap,
}

impl QualifiedFormulaIndex {
    pub(super) fn new(maps: &std::collections::BTreeMap<String, WorksheetMap>) -> Self {
        let sheets = maps
            .iter()
            .map(|(name, map)| {
                let mut columns = CellColumnMap::new();
                for cell in &map.cells {
                    columns
                        .entry((cell.source_row, cell.source_column))
                        .or_default()
                        .push(cell.target_column);
                }
                for targets in columns.values_mut() {
                    targets.sort_unstable();
                    targets.dedup();
                }
                (
                    name.to_lowercase(),
                    IndexedWorksheet {
                        rows: map.rows.clone(),
                        original_rows: map.original_rows.clone(),
                        columns,
                    },
                )
            })
            .collect();
        Self { sheets }
    }
}

pub(super) fn normalize_shared_formulas(xml: &str) -> Result<String, FillError> {
    let elements = crate::spreadsheet::xmlutil::elements(xml)?;
    let cells = elements
        .iter()
        .filter(|element| local(&element.name) == "c")
        .collect::<Vec<_>>();
    let mut masters = std::collections::BTreeMap::<String, (String, String)>::new();
    for cell in &cells {
        let open = &xml[cell.start..cell.open_end];
        let Some(reference) = crate::xml::read_attribute(open, "r") else {
            continue;
        };
        for formula in elements.iter().filter(|formula| {
            local(&formula.name) == "f" && formula.start >= cell.open_end && formula.end <= cell.end
        }) {
            let raw = &xml[formula.start..formula.open_end];
            if crate::xml::read_attribute(raw, "t") != Some("shared") {
                continue;
            }
            let Some(si) = crate::xml::read_attribute(raw, "si") else {
                continue;
            };
            let body = crate::xml::unescape(&xml[formula.open_end..formula.close_start]);
            if !body.is_empty() {
                masters.insert(si.to_string(), (reference.to_string(), body));
            }
        }
    }
    let mut replacements = Vec::new();
    for cell in cells {
        let open = &xml[cell.start..cell.open_end];
        let Some(target) = crate::xml::read_attribute(open, "r") else {
            continue;
        };
        for formula in elements.iter().filter(|formula| {
            local(&formula.name) == "f" && formula.start >= cell.open_end && formula.end <= cell.end
        }) {
            let raw = &xml[formula.start..formula.open_end];
            if crate::xml::read_attribute(raw, "t") != Some("shared") {
                continue;
            }
            let Some(si) = crate::xml::read_attribute(raw, "si") else {
                continue;
            };
            let Some((source, master)) = masters.get(si) else {
                return Err(reference_error("Shared formula has no master formula"));
            };
            let translated = translate_formula_copy(master, source, target)?;
            let open = remove_attribute(raw, "t");
            let open = remove_attribute(&open, "si");
            let open = remove_attribute(&open, "ref");
            let open = open.trim_end_matches("/>");
            let open = format!("{}>", open.trim_end_matches('>'));
            let replacement = format!(
                "{}{}{}</{}>",
                open,
                escape_formula(&translated),
                "",
                formula.name
            );
            replacements.push((formula.start, formula.end, replacement));
        }
    }
    apply_replacements(xml, replacements)
}

fn translate_formula_copy(formula: &str, source: &str, target: &str) -> Result<String, FillError> {
    let (source_col, source_row) = split_cell(source)
        .and_then(|(col, row)| column_number(col).map(|col| (col, row)))
        .ok_or_else(|| reference_error("Invalid shared formula master cell"))?;
    let (target_col, target_row) = split_cell(target)
        .and_then(|(col, row)| column_number(col).map(|col| (col, row)))
        .ok_or_else(|| reference_error("Invalid shared formula target cell"))?;
    let rows = update_formula_with_rows(formula, source_row, target_row, &RowMap::new(), &[])?;
    Ok(shift_formula_columns(
        &rows,
        i64::from(target_col) - i64::from(source_col),
    ))
}

fn remove_attribute(raw: &str, name: &str) -> String {
    let Some((value_start, value_end, _)) = attribute_range(raw, name) else {
        return raw.to_string();
    };
    let mut start = value_start;
    while start > 0
        && !raw.as_bytes()[start - 1].is_ascii_whitespace()
        && raw.as_bytes()[start - 1] != b'<'
    {
        start -= 1;
    }
    while start > 0 && raw.as_bytes()[start - 1].is_ascii_whitespace() {
        start -= 1;
    }
    let end = value_end + 1;
    let mut output = raw.to_string();
    output.replace_range(start..end.min(raw.len()), "");
    output
}

fn validate_array_formulas(xml: &str, map: &WorksheetMap) -> Result<(), FillError> {
    let elements = crate::spreadsheet::xmlutil::elements(xml)?;
    for formula in elements.iter().filter(|item| local(&item.name) == "f") {
        let open = &xml[formula.start..formula.open_end];
        if crate::xml::read_attribute(open, "t") != Some("array") {
            continue;
        }
        let Some(reference) = crate::xml::read_attribute(open, "ref") else {
            return Err(reference_error("Array formula has no reference range"));
        };
        let (left, right) = reference.split_once(':').unwrap_or((reference, reference));
        let (Some((left_col, first)), Some((right_col, last))) =
            (split_cell(left), split_cell(right))
        else {
            return Err(reference_error("Array formula has an invalid range"));
        };
        let low = first.min(last);
        let high = first.max(last);
        for (source, targets) in map.rows.range(low..=high) {
            if targets.len() != 1 {
                return Err(reference_error(
                    "An array formula range crosses a repeated row",
                ));
            }
            let _ = source;
        }
        if map
            .original_rows
            .iter()
            .any(|row| (low..=high).contains(row) && !map.rows.contains_key(row))
        {
            return Err(reference_error(
                "An array formula range crosses a removed row",
            ));
        }
        let first_out = map_row(low, 0, 0, false, false, 0, &map.rows, &map.original_rows)
            .ok_or_else(|| reference_error("Array formula range has no surviving rows"))?;
        let last_out = map_row(high, 0, 0, false, true, 0, &map.rows, &map.original_rows)
            .ok_or_else(|| reference_error("Array formula range has no surviving rows"))?;
        if last_out < first_out || last_out - first_out != high - low {
            return Err(reference_error(
                "Array formula range is split by a structural edit",
            ));
        }
        let c1 = column_number(left_col)
            .ok_or_else(|| reference_error("Array formula has an invalid column"))?;
        let c2 = column_number(right_col)
            .ok_or_else(|| reference_error("Array formula has an invalid column"))?;
        if map.cells.iter().any(|cell| {
            (low..=high).contains(&cell.source_row)
                && (c1..=c2).contains(&cell.source_column)
                && cell.source_column != cell.target_column
        }) {
            return Err(reference_error(
                "An array formula range crosses a horizontal column edit",
            ));
        }
    }
    Ok(())
}

impl WorksheetMap {
    pub(super) fn new(
        row_map: &[(u32, u32)],
        cells: Vec<CellCoordinate>,
        original_rows: Vec<u32>,
    ) -> Self {
        let mut rows = RowMap::new();
        for &(source, target) in row_map {
            rows.entry(source).or_default().push(target);
        }
        for targets in rows.values_mut() {
            targets.sort_unstable();
            targets.dedup();
        }
        Self {
            rows,
            cells,
            original_rows,
        }
    }
}

pub(super) fn update_sheet_with_map(xml: &str, map: &WorksheetMap) -> Result<String, FillError> {
    validate_array_formulas(xml, map)?;
    let row_pairs = map
        .rows
        .iter()
        .flat_map(|(source, targets)| targets.iter().map(move |target| (*source, *target)))
        .collect::<Vec<_>>();
    let columns = rewrite_column_geometry(xml, map)?;
    let updated =
        update_sheet_with_rows_mode(&columns, &row_pairs, &map.original_rows, true, true)?;
    rewrite_other_formulas(&updated, map)
}

fn rewrite_other_formulas(xml: &str, map: &WorksheetMap) -> Result<String, FillError> {
    let tokens = tokenize(xml).map_err(reference_error)?;
    let mut replacements = Vec::new();
    let mut offset = 0usize;
    let mut start = None;
    for token in tokens {
        match token {
            Token::Text(text) => offset += text.len(),
            Token::Element { kind, name, raw } => {
                let tag = local(name);
                if kind == ElementKind::Open && matches!(tag, "formula" | "formula1" | "formula2") {
                    start = Some((offset + raw.len(), tag.to_string()));
                } else if kind == ElementKind::Close
                    && matches!(tag, "formula" | "formula1" | "formula2")
                    && let Some((begin, _)) = start.take()
                {
                    let formula = crate::xml::unescape(&xml[begin..offset]);
                    let changed = update_formula_with_rows_mode(
                        &formula,
                        0,
                        0,
                        &map.rows,
                        &map.original_rows,
                        true,
                    )?;
                    let changed = escape_formula(&changed);
                    if changed != xml[begin..offset] {
                        replacements.push((begin, offset, changed));
                    }
                }
                offset += raw.len();
            }
        }
    }
    apply_replacements(xml, replacements)
}

pub(super) fn update_table_part(xml: &str, map: &WorksheetMap) -> Result<String, FillError> {
    let columns = rewrite_column_geometry(xml, map)?;
    let row_pairs = map
        .rows
        .iter()
        .flat_map(|(source, targets)| targets.iter().map(move |target| (*source, *target)))
        .collect::<Vec<_>>();
    update_sheet_with_rows_mode(&columns, &row_pairs, &map.original_rows, true, false)
}

pub(super) fn update_table_part_with_sheet(
    xml: &str,
    map: &WorksheetMap,
    sheet_xml: &str,
) -> Result<String, FillError> {
    let mut updated = update_table_part(xml, map)?;
    let root = tokenize(xml).map_err(reference_error)?;
    let table_ref = root.iter().find_map(|token| match token {
        Token::Element { name, raw, .. } if local(name) == "table" => {
            attribute_range(raw, "ref").map(|(_, _, value)| value.to_string())
        }
        _ => None,
    });
    let Some(table_ref) = table_ref else {
        return Ok(updated);
    };
    let (left, _) = table_ref
        .split_once(':')
        .unwrap_or((&table_ref, &table_ref));
    let Some((first_col, header_row)) = split_cell(left) else {
        return Ok(updated);
    };
    let first_col =
        column_number(first_col).ok_or_else(|| reference_error("Invalid table column"))?;
    let headers = header_values(sheet_xml)?;
    let target_header_row = map
        .rows
        .get(&header_row)
        .and_then(|rows| rows.first())
        .copied()
        .unwrap_or(header_row);
    let columns: std::collections::BTreeMap<(u32, u32), Vec<u32>> = {
        let mut values = std::collections::BTreeMap::new();
        for cell in &map.cells {
            values
                .entry((cell.source_row, cell.source_column))
                .or_insert_with(Vec::new)
                .push(cell.target_column);
        }
        for targets in values.values_mut() {
            targets.sort_unstable();
            targets.dedup();
        }
        values
    };
    let ranges = table_columns(xml)?;
    let Some((container_start, container_end, open_end, items)) = ranges else {
        return Ok(updated);
    };
    let mut expanded = Vec::<String>::new();
    let mut used = std::collections::BTreeSet::<String>::new();
    for (source_index, item_xml) in items.iter().enumerate() {
        let source_col = first_col + source_index as u32;
        let mut targets = columns
            .get(&(header_row, source_col))
            .cloned()
            .unwrap_or_else(|| vec![source_col]);
        for target_col in targets.drain(..) {
            let mut item = item_xml.clone();
            let id = expanded.len() + 1;
            item = with_value(&item, "id", &id.to_string());
            if let Some((_, _, name)) = attribute_range(&item, "name") {
                let base = headers
                    .get(&(target_col, target_header_row))
                    .cloned()
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| name.to_string());
                let mut unique = base.clone();
                let mut suffix = 2;
                while !used.insert(unique.to_lowercase()) {
                    unique = format!("{base}_{suffix}");
                    suffix += 1;
                }
                item = with_value(&item, "name", &escape_attribute(&unique));
            }
            expanded.push(item);
            let _ = target_col;
        }
    }
    let count_open = &updated[container_start..open_end];
    let count_open = with_value(count_open, "count", &expanded.len().to_string());
    let replacement = format!(
        "{}{}{}",
        count_open,
        expanded.join(""),
        &updated[container_end..]
    );
    updated.replace_range(container_start..container_end, &replacement);
    Ok(updated)
}

fn header_values(xml: &str) -> Result<std::collections::BTreeMap<(u32, u32), String>, FillError> {
    let elements = crate::spreadsheet::xmlutil::elements(xml)?;
    let mut headers = std::collections::BTreeMap::new();
    for cell in elements
        .iter()
        .filter(|element| local(&element.name) == "c")
    {
        let open = &xml[cell.start..cell.open_end];
        let Some(reference) = crate::xml::read_attribute(open, "r") else {
            continue;
        };
        let Some((column, row)) = split_cell(reference) else {
            continue;
        };
        let col = column_number(column).unwrap_or(0);
        let kind = crate::xml::read_attribute(open, "t").unwrap_or_default();
        if kind == "s" {
            continue;
        }
        let children = crate::spreadsheet::xmlutil::elements(&xml[cell.start..cell.end])?;
        let texts = children
            .iter()
            .filter(|child| local(&child.name) == "t")
            .collect::<Vec<_>>();
        let mut text = String::new();
        if !texts.is_empty() {
            for child in texts {
                text.push_str(&crate::xml::unescape(
                    &xml[cell.start + child.open_end..cell.start + child.close_start],
                ));
            }
        } else if let Some(value) = children.iter().find(|child| local(&child.name) == "v") {
            text = crate::xml::unescape(
                &xml[cell.start + value.open_end..cell.start + value.close_start],
            );
        }
        if !text.is_empty() {
            headers.insert((col, row), text);
        }
    }
    Ok(headers)
}

fn escape_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

type TableColumnsParts = (usize, usize, usize, Vec<String>);

fn table_columns(xml: &str) -> Result<Option<TableColumnsParts>, FillError> {
    let tokens = tokenize(xml).map_err(reference_error)?;
    let mut at = 0usize;
    let mut depth = 0usize;
    let mut container = None;
    let mut close = None;
    let mut items = Vec::new();
    let mut item_start = None;
    for token in tokens {
        let (len, kind, name) = match token {
            Token::Text(text) => (text.len(), None, ""),
            Token::Element { kind, name, raw } => (raw.len(), Some(kind), name),
        };
        match kind {
            Some(ElementKind::Open) if local(name) == "tableColumns" => {
                container = Some((at, at + len));
                depth = 1;
            }
            Some(ElementKind::SelfClosing) if local(name) == "tableColumns" => {
                return Ok(Some((at, at + len, at + len, Vec::new())));
            }
            Some(ElementKind::Open) if depth > 0 => {
                depth += 1;
                if depth == 2 && local(name) == "tableColumn" {
                    item_start = Some(at);
                }
            }
            Some(ElementKind::SelfClosing) if depth == 1 && local(name) == "tableColumn" => {
                items.push(xml[at..at + len].to_string())
            }
            Some(ElementKind::Close) if depth > 0 => {
                if depth == 2
                    && local(name) == "tableColumn"
                    && let Some(start) = item_start.take()
                {
                    items.push(xml[start..at + len].to_string());
                }
                depth -= 1;
                if depth == 0 && local(name) == "tableColumns" {
                    close = Some(at + len);
                }
            }
            _ => {}
        }
        at += len;
    }
    Ok(container
        .zip(close)
        .map(|((start, open_end), end)| (start, end, open_end, items)))
}

pub(super) fn transform_layout(xml: &str, map: &WorksheetMap) -> Result<String, FillError> {
    let tokens = tokenize(xml).map_err(reference_error)?;
    let mut replacements = Vec::new();
    let mut offset = 0usize;
    for token in tokens {
        let (name, raw) = match token {
            Token::Element { name, raw, .. } => (name, raw),
            Token::Text(text) => {
                offset += text.len();
                continue;
            }
        };
        if local(name) == "brk"
            && let Some((_, _, id)) = attribute_range(raw, "id")
            && let Ok(row) = id.parse::<u32>()
            && let Some(targets) = map.rows.get(&row)
        {
            let updated = with_value(raw, "id", &targets[0].to_string());
            if updated != raw {
                replacements.push((offset, offset + raw.len(), updated));
            }
        }
        offset += raw.len();
    }
    apply_replacements(xml, replacements)
}

/// Move drawing anchors with their worksheet cells. Coordinates use DrawingML's zero-based
/// column and row numbering. Repeated source cells use their first emitted position.
pub(super) fn transform_anchors(
    xml: &str,
    map: &WorksheetMap,
    cell_map: &[CellCoordinate],
) -> Result<String, FillError> {
    let cloned = clone_repeated_anchors(xml, map, cell_map)?;
    let mapped = remap_anchors(&cloned, map, cell_map)?;
    Ok(mapped.replace(" data-docx-fill-mapped=\"1\"", ""))
}

fn remap_anchors(
    xml: &str,
    map: &WorksheetMap,
    cell_map: &[CellCoordinate],
) -> Result<String, FillError> {
    let tokens = tokenize(xml).map_err(reference_error)?;
    let mut replacements = Vec::new();
    let mut offset = 0usize;
    let cell_columns: std::collections::BTreeMap<(u32, u32), u32> = cell_map
        .iter()
        .map(|cell| ((cell.source_row, cell.source_column), cell.target_column))
        .collect();
    type PointSpan = (usize, usize, u32);
    let mut anchor: Option<(Option<PointSpan>, Option<PointSpan>)> = None;
    let mut value: Option<(usize, bool)> = None;
    let mut mapped_anchor = false;
    for token in tokens {
        let (length, kind, name, raw) = match token {
            Token::Text(text) => (text.len(), None, "", text),
            Token::Element { kind, name, raw } => (raw.len(), Some(kind), name, raw),
        };
        let Some(kind) = kind else {
            offset += length;
            continue;
        };
        let local = local(name);
        if kind == ElementKind::Open
            && matches!(local, "twoCellAnchor" | "oneCellAnchor" | "absoluteAnchor")
        {
            mapped_anchor = raw.contains("data-docx-fill-mapped");
        }
        if !mapped_anchor && kind == ElementKind::Open && matches!(local, "from" | "to") {
            anchor = Some((None, None));
        }
        if !mapped_anchor
            && kind == ElementKind::Open
            && anchor.is_some()
            && matches!(local, "col" | "row")
        {
            value = Some((offset + length, local == "row"));
        }
        if !mapped_anchor
            && kind == ElementKind::Close
            && anchor.is_some()
            && matches!(local, "col" | "row")
            && let Some((start, is_row)) = value.take()
            && let Ok(n) = xml[start..offset].parse::<u32>()
            && let Some((col, row)) = anchor.as_mut()
        {
            if is_row {
                *row = Some((start, offset, n));
            } else {
                *col = Some((start, offset, n));
            }
        }
        if !mapped_anchor
            && kind == ElementKind::Close
            && matches!(local, "from" | "to")
            && let Some((Some((cs, ce, col)), Some((rs, re, row)))) = anchor.take()
        {
            let source_row = row + 1;
            let source_col = col + 1;
            let target_row = map
                .rows
                .get(&source_row)
                .and_then(|targets| targets.first())
                .copied()
                .unwrap_or(source_row);
            let target_col = cell_columns
                .get(&(source_row, source_col))
                .copied()
                .unwrap_or(source_col);
            if target_row != source_row {
                replacements.push((rs, re, (target_row - 1).to_string()));
            }
            if target_col != source_col {
                replacements.push((cs, ce, (target_col - 1).to_string()));
            }
        }
        if kind == ElementKind::Close
            && matches!(local, "twoCellAnchor" | "oneCellAnchor" | "absoluteAnchor")
        {
            mapped_anchor = false;
        }
        let _ = raw;
        offset += length;
    }
    apply_replacements(xml, replacements)
}

fn clone_repeated_anchors(
    xml: &str,
    map: &WorksheetMap,
    cells: &[CellCoordinate],
) -> Result<String, FillError> {
    let elements = crate::spreadsheet::xmlutil::elements(xml)?;
    let anchors = elements
        .iter()
        .filter(|element| {
            matches!(
                local(&element.name),
                "twoCellAnchor" | "oneCellAnchor" | "absoluteAnchor"
            )
        })
        .collect::<Vec<_>>();
    if anchors.is_empty() {
        return Ok(xml.to_string());
    }
    let mut max_id = 0u32;
    for element in elements
        .iter()
        .filter(|element| local(&element.name) == "cNvPr")
    {
        let raw = &xml[element.start..element.open_end];
        max_id = max_id.max(
            crate::xml::read_attribute(raw, "id")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
        );
    }
    let mut replacements = Vec::new();
    for anchor in anchors {
        let part = &xml[anchor.start..anchor.end];
        let Some((from_col, from_row)) = anchor_point(part, "from") else {
            continue;
        };
        let source_row = from_row + 1;
        let source_col = from_col + 1;
        let mut candidates = cells
            .iter()
            .filter(|cell| cell.source_row == source_row && cell.source_column == source_col)
            .collect::<Vec<_>>();
        candidates.sort_by_key(|cell| (cell.target_row, cell.target_column));
        candidates.dedup_by_key(|cell| (cell.target_row, cell.target_column));
        if candidates.len() < 2 {
            continue;
        }
        let mut copies = String::new();
        for (index, from) in candidates.iter().enumerate() {
            let mut copy = part.to_string();
            let to_point = anchor_point(&copy, "to");
            let targets = to_point.map(|(to_col, to_row)| (to_col + 1, to_row + 1));
            copy = set_anchor_point(&copy, "from", (from.target_column - 1, from.target_row - 1));
            if let Some((to_source_col, to_source_row)) = targets {
                let mapped_row = if candidates.len() > 1 {
                    if to_source_row >= source_row {
                        from.target_row.saturating_add(to_source_row - source_row)
                    } else {
                        from.target_row.saturating_sub(source_row - to_source_row)
                    }
                } else if to_source_row == source_row {
                    from.target_row
                } else {
                    map.rows
                        .get(&to_source_row)
                        .and_then(|rows| rows.first())
                        .copied()
                        .unwrap_or(to_source_row)
                };
                let mapped_col = cells
                    .iter()
                    .filter(|cell| {
                        cell.source_row == to_source_row && cell.source_column == to_source_col
                    })
                    .nth(index)
                    .or_else(|| {
                        cells.iter().find(|cell| {
                            cell.source_row == to_source_row && cell.source_column == to_source_col
                        })
                    })
                    .map_or(to_source_col, |cell| cell.target_column);
                copy = set_anchor_point(&copy, "to", (mapped_col - 1, mapped_row - 1));
            }
            copy = mark_mapped_anchor(&copy)?;
            if index > 0 {
                let (renumbered, next) = renumber_shape_ids(&copy, max_id.saturating_add(1))?;
                copy = renumbered;
                max_id = next;
            }
            copies.push_str(&copy);
        }
        replacements.push((anchor.start, anchor.end, copies));
    }
    apply_replacements(xml, replacements)
}

fn mark_mapped_anchor(xml: &str) -> Result<String, FillError> {
    let tokens = tokenize(xml).map_err(reference_error)?;
    let Some(Token::Element { raw, .. }) = tokens.first() else {
        return Ok(xml.to_string());
    };
    let changed = crate::properties::set_attribute(raw, "data-docx-fill-mapped", "1");
    Ok(format!("{}{}", changed, &xml[raw.len()..]))
}

fn anchor_point(xml: &str, which: &str) -> Option<(u32, u32)> {
    let tokens = tokenize(xml).ok()?;
    let mut offset = 0;
    let mut active = false;
    let mut field = None;
    let mut col = None;
    let mut row = None;
    for token in tokens {
        match token {
            Token::Text(text) => offset += text.len(),
            Token::Element { kind, name, raw } => {
                let tag = local(name);
                if kind == ElementKind::Open && tag == which {
                    active = true;
                } else if kind == ElementKind::Close && tag == which {
                    active = false;
                } else if active && kind == ElementKind::Open && matches!(tag, "col" | "row") {
                    field = Some((offset + raw.len(), tag == "row"));
                } else if active && kind == ElementKind::Close && matches!(tag, "col" | "row")
                    && let Some((start, is_row)) = field.take() {
                        let value = xml[start..offset].parse().ok()?;
                        if is_row {
                            row = Some(value)
                        } else {
                            col = Some(value)
                        }
                    }
                offset += raw.len();
            }
        }
    }
    Some((col?, row?))
}

fn set_anchor_point(xml: &str, which: &str, point: (u32, u32)) -> String {
    let Ok(tokens) = tokenize(xml) else {
        return xml.to_string();
    };
    let mut offset = 0;
    let mut active = false;
    let mut field = None;
    let mut replacements = Vec::new();
    for token in tokens {
        match token {
            Token::Text(text) => offset += text.len(),
            Token::Element { kind, name, raw } => {
                let tag = local(name);
                if kind == ElementKind::Open && tag == which {
                    active = true;
                } else if kind == ElementKind::Close && tag == which {
                    active = false;
                } else if active && kind == ElementKind::Open && matches!(tag, "col" | "row") {
                    field = Some((offset + raw.len(), tag == "row"));
                } else if active
                    && kind == ElementKind::Close
                    && matches!(tag, "col" | "row")
                    && let Some((start, is_row)) = field.take()
                {
                    replacements.push((
                        start,
                        offset,
                        if is_row {
                            point.1.to_string()
                        } else {
                            point.0.to_string()
                        },
                    ));
                }
                offset += raw.len();
            }
        }
    }
    apply_replacements(xml, replacements).unwrap_or_else(|_| xml.to_string())
}

fn renumber_shape_ids(xml: &str, start: u32) -> Result<(String, u32), FillError> {
    let elements = crate::spreadsheet::xmlutil::elements(xml)?;
    let mut replacements = Vec::new();
    let mut id = start;
    for element in elements
        .iter()
        .filter(|element| local(&element.name) == "cNvPr")
    {
        let raw = &xml[element.start..element.open_end];
        if let Some((_, _, _)) = attribute_range(raw, "id") {
            let updated = with_value(raw, "id", &id.to_string());
            replacements.push((element.start, element.open_end, updated));
            id = id.saturating_add(1);
        }
    }
    Ok((apply_replacements(xml, replacements)?, id.saturating_sub(1)))
}

fn apply_replacements(
    xml: &str,
    mut replacements: Vec<(usize, usize, String)>,
) -> Result<String, FillError> {
    replacements.sort_by_key(|(start, ..)| *start);
    let mut output = String::with_capacity(xml.len());
    let mut at = 0;
    for (start, end, value) in replacements {
        if start < at {
            return Err(reference_error("Overlapping spreadsheet XML edits"));
        }
        output.push_str(&xml[at..start]);
        output.push_str(&value);
        at = end;
    }
    output.push_str(&xml[at..]);
    Ok(output)
}

fn rewrite_column_geometry(xml: &str, map: &WorksheetMap) -> Result<String, FillError> {
    // Column-only changes affect ranges such as validations, hyperlinks, merges and table refs.
    let mut cell_targets = std::collections::BTreeMap::<(u32, u32), Vec<u32>>::new();
    for cell in &map.cells {
        cell_targets
            .entry((cell.source_row, cell.source_column))
            .or_default()
            .push(cell.target_column);
    }
    for targets in cell_targets.values_mut() {
        targets.sort_unstable();
        targets.dedup();
    }
    let tokens = tokenize(xml).map_err(reference_error)?;
    let mut replacements = Vec::new();
    let mut offset = 0;
    for token in tokens {
        let (length, kind, name, raw) = match token {
            Token::Text(text) => (text.len(), None, "", text),
            Token::Element { kind, name, raw } => (raw.len(), Some(kind), name, raw),
        };
        if kind.is_some_and(|kind| kind != ElementKind::Close)
            && local(name) == "mergeCell"
            && let Some(clones) = duplicate_horizontal_merge(raw, &cell_targets)
        {
            replacements.push((offset, offset + length, clones));
            offset += length;
            continue;
        }
        if kind.is_some_and(|kind| kind != ElementKind::Close)
            && ["ref", "sqref"]
                .iter()
                .any(|attribute| attribute_range(raw, attribute).is_some())
        {
            let mut changed = raw.to_string();
            for attribute in ["ref", "sqref"] {
                if let Some((_, _, value)) = attribute_range(&changed, attribute)
                    && let Some(next) = remap_columns(value, &cell_targets)?
                {
                    changed = with_value(&changed, attribute, &next);
                }
            }
            if changed != raw {
                replacements.push((offset, offset + length, changed));
            }
        }
        let _ = name;
        offset += length;
    }
    apply_replacements(xml, replacements)
}

fn duplicate_horizontal_merge(
    raw: &str,
    cells: &std::collections::BTreeMap<(u32, u32), Vec<u32>>,
) -> Option<String> {
    let (_, _, reference) = attribute_range(raw, "ref")?;
    let (first, last) = reference.split_once(':')?;
    let ((first_col, row), (last_col, last_row)) = (split_cell(first)?, split_cell(last)?);
    if row != last_row {
        return None;
    }
    let first_col = column_number(first_col)?;
    let last_col = column_number(last_col)?;
    let starts = cells
        .get(&(row, first_col))
        .cloned()
        .unwrap_or_else(|| vec![first_col]);
    let ends = cells
        .get(&(row, last_col))
        .cloned()
        .unwrap_or_else(|| vec![last_col]);
    if starts.len() <= 1 && ends.len() <= 1 {
        return None;
    }
    Some(
        starts
            .iter()
            .zip(&ends)
            .map(|(start, end)| {
                with_value(
                    raw,
                    "ref",
                    &format!(
                        "{}{}:{}{}",
                        column_name_ref(*start),
                        row,
                        column_name_ref(*end),
                        row
                    ),
                )
            })
            .collect::<String>(),
    )
}

fn remap_columns(
    value: &str,
    cells: &std::collections::BTreeMap<(u32, u32), Vec<u32>>,
) -> Result<Option<String>, FillError> {
    let mut output = Vec::new();
    let mut changed = false;
    for token in value.split_ascii_whitespace() {
        let endpoints = token.split_once(':');
        let mut next = Vec::new();
        let parts = endpoints.map_or_else(|| vec![token], |(left, right)| vec![left, right]);
        for (endpoint, endpoint_index) in parts.iter().zip(0..) {
            let Some((column, row)) = split_cell(endpoint) else {
                next.push(endpoint.to_string());
                continue;
            };
            let col = column_number(column)
                .ok_or_else(|| reference_error("Invalid spreadsheet column"))?;
            let new_col = cells
                .get(&(row, col))
                .and_then(|targets| {
                    if endpoint_index == 0 {
                        targets.first()
                    } else {
                        targets.last()
                    }
                })
                .copied()
                .unwrap_or(col);
            if new_col != col {
                changed = true;
            }
            next.push(format!("{}{}", column_name_ref(new_col), row));
        }
        if next.len() == 2 {
            output.push(format!("{}:{}", next[0], next[1]));
        } else {
            output.push(next[0].clone());
        }
    }
    Ok(changed.then(|| output.join(" ")))
}

/// Transform a formula when its containing cell moves through a rendered worksheet map.
#[expect(
    dead_code,
    reason = "Kept as the per-cell formula transform contract for workbook integrations"
)]
pub(super) fn transform_formula_with_map(
    formula: &str,
    source_cell: &str,
    target_cell: &str,
    map: &WorksheetMap,
) -> Result<String, FillError> {
    let (_, source_row) =
        split_cell(source_cell).ok_or_else(|| reference_error("Invalid source cell reference"))?;
    let (_, target_row) =
        split_cell(target_cell).ok_or_else(|| reference_error("Invalid target cell reference"))?;
    let normalized = update_formula_with_rows(
        formula,
        source_row,
        target_row,
        &map.rows,
        &map.original_rows,
    )?;
    // Column copy semantics are handled separately from row geometry because horizontal loops
    // shift cells within a row. Apply the containing-cell delta only to relative columns.
    let (source_column, _) = split_cell(source_cell)
        .and_then(|(col, row)| column_number(col).map(|number| (number, row)))
        .ok_or_else(|| reference_error("Invalid source cell reference"))?;
    let (target_column, _) = split_cell(target_cell)
        .and_then(|(col, row)| column_number(col).map(|number| (number, row)))
        .ok_or_else(|| reference_error("Invalid target cell reference"))?;
    Ok(shift_formula_columns(
        &normalized,
        i64::from(target_column) - i64::from(source_column),
    ))
}

pub(super) fn shift_formula_in_cell(xml: &str, delta: i64) -> Result<String, FillError> {
    if delta == 0 {
        return Ok(xml.to_string());
    }
    let tokens = tokenize(xml).map_err(reference_error)?;
    let mut replacements = Vec::new();
    let mut offset = 0usize;
    let mut start = None;
    for token in tokens {
        match token {
            Token::Text(text) => offset += text.len(),
            Token::Element { kind, name, raw } => {
                if local(name) == "f" && kind == ElementKind::Open {
                    start = Some(offset + raw.len());
                } else if local(name) == "f" && kind == ElementKind::Close
                    && let Some(begin) = start.take() {
                        let formula = crate::xml::unescape(&xml[begin..offset]);
                        let shifted = shift_formula_columns(&formula, delta);
                        let escaped = escape_formula(&shifted);
                        if escaped != xml[begin..offset] {
                            replacements.push((begin, offset, escaped));
                        }
                    }
                offset += raw.len();
            }
        }
    }
    apply_replacements(xml, replacements)
}

/// Apply structural row changes to explicitly qualified references using the referenced
/// worksheet's map. The workbook layer supplies display names before any sheet renames.
#[cfg(test)]
pub(super) fn transform_qualified_formula_refs(
    formula: &str,
    maps: &std::collections::BTreeMap<String, WorksheetMap>,
) -> Result<String, FillError> {
    let index = QualifiedFormulaIndex::new(maps);
    transform_qualified_formula_refs_with_index(formula, &index)
}

pub(super) fn transform_qualified_formula_refs_with_index(
    formula: &str,
    index: &QualifiedFormulaIndex,
) -> Result<String, FillError> {
    let mut replacements = Vec::new();
    let mut at = 0usize;
    let mut in_string = false;
    while at < formula.len() {
        let ch = formula[at..].chars().next().expect("valid boundary");
        if ch == '"' {
            in_string = !in_string;
            at += 1;
            continue;
        }
        if in_string {
            at += ch.len_utf8();
            continue;
        }
        let Some((first, last)) = read_reference(formula, at) else {
            at += ch.len_utf8();
            continue;
        };
        let end = last.as_ref().map_or(first.end, |last| last.end);
        if let Some(sheets) = qualified_sheets_before(formula, at) {
            let mut transformed_forms = Vec::new();
            let transform_for_map = |map: &IndexedWorksheet| -> Result<String, FillError> {
                let columns = remap_columns(&formula[at..end], &map.columns)?
                    .unwrap_or_else(|| formula[at..end].to_string());
                update_formula_with_rows(
                    &columns,
                    0,
                    0,
                    &map.rows,
                    &map.original_rows,
                )
            };
            for sheet in &sheets {
                if let Some(map) = index.sheets.get(&sheet.to_lowercase()) {
                    transformed_forms.push(transform_for_map(map)?);
                } else {
                    transformed_forms.push(formula[at..end].to_string());
                }
            }
            if sheets.len() > 1 {
                for map in index.sheets.values() {
                    transformed_forms.push(transform_for_map(map)?);
                }
            }
            if transformed_forms.windows(2).any(|pair| pair[0] != pair[1]) {
                return Err(reference_error(
                    "A 3D formula reference crosses worksheets with different structural row maps",
                ));
            }
            if let Some(transformed) = transformed_forms.first()
                && transformed != &formula[at..end]
            {
                replacements.push((at, end, transformed.clone()));
            }
            at = end;
        } else {
            at = end;
        }
    }
    apply_replacements(formula, replacements)
}

fn qualified_sheets_before(formula: &str, at: usize) -> Option<Vec<String>> {
    if !formula[..at].ends_with('!') {
        return None;
    }
    let end = at - 1;
    if formula[..end].ends_with(']') {
        return None;
    }
    if formula[..end].ends_with('\'') {
        let mut cursor = end.saturating_sub(1);
        let bytes = formula.as_bytes();
        while cursor > 0 {
            cursor -= 1;
            if bytes[cursor] == b'\'' {
                if cursor > 0 && bytes.get(cursor - 1) == Some(&b'\'') {
                    cursor -= 1;
                    continue;
                }
                let raw = formula.get(cursor + 1..end - 1)?;
                return Some(
                    raw.replace("''", "'")
                        .split(':')
                        .map(str::to_string)
                        .collect(),
                );
            }
        }
        return None;
    }
    let start = formula[..end]
        .rfind(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':')))
        .map_or(0, |i| i + 1);
    if start > 0 && formula.as_bytes().get(start - 1) == Some(&b']') {
        return None;
    }
    (start < end).then(|| formula[start..end].split(':').map(str::to_string).collect())
}

fn shift_formula_columns(formula: &str, delta: i64) -> String {
    if delta == 0 {
        return formula.to_string();
    }
    let bytes = formula.as_bytes();
    let mut out = String::with_capacity(formula.len());
    let mut at = 0usize;
    let mut in_string = false;
    let mut in_sheet_quote = false;
    while at < bytes.len() {
        if bytes[at] == b'"' {
            in_string = !in_string;
            out.push('"');
            at += 1;
            continue;
        }
        if bytes[at] == b'\'' && !in_string {
            if in_sheet_quote && bytes.get(at + 1) == Some(&b'\'') {
                out.push_str("''");
                at += 2;
                continue;
            }
            in_sheet_quote = !in_sheet_quote;
            out.push('\'');
            at += 1;
            continue;
        }
        if !in_string && !in_sheet_quote {
            let start = at;
            if bytes[at] == b'$' {
                at += 1;
            }
            let letters = at;
            while bytes.get(at).is_some_and(u8::is_ascii_alphabetic) {
                at += 1;
            }
            if at > letters
                && is_formula_boundary_before(bytes, start)
                && let Some(column) = column_number(&formula[letters..at])
            {
                let column_end = at;
                let digit_start = at + usize::from(bytes.get(at) == Some(&b'$'));
                let mut digit_end = digit_start;
                while bytes.get(digit_end).is_some_and(u8::is_ascii_digit) {
                    digit_end += 1;
                }
                if digit_end > digit_start {
                    let followed_by_sheet = bytes.get(digit_end) == Some(&b'!')
                        || (bytes.get(digit_end) == Some(&b'\'')
                            && bytes.get(digit_end + 1) == Some(&b'!'));
                    let followed_by_function = bytes.get(digit_end) == Some(&b'(');
                    let identifier_tail = bytes.get(digit_end).is_some_and(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.')
                    });
                    if !followed_by_sheet
                        && !followed_by_function
                        && !identifier_tail
                        && !has_external_workbook_qualifier(formula, start)
                        && let Ok(row) = formula[digit_start..digit_end].parse::<u32>()
                    {
                        let absolute = bytes.get(start) == Some(&b'$');
                        let shifted = i64::from(column) + delta;
                        if !absolute && (1..=16_384).contains(&shifted) {
                            out.push_str(&formula[start..letters]);
                            out.push_str(&column_name_ref(shifted as u32));
                            out.push_str(&formula[column_end..digit_start]);
                            out.push_str(&row.to_string());
                            at = digit_end;
                            continue;
                        }
                    }
                } else if bytes.get(column_end) == Some(&b':') {
                    let mut right_start = column_end + 1;
                    if bytes.get(right_start) == Some(&b'$') {
                        right_start += 1;
                    }
                    let mut right_end = right_start;
                    while bytes.get(right_end).is_some_and(u8::is_ascii_alphabetic) {
                        right_end += 1;
                    }
                    if right_end > right_start
                        && !bytes.get(right_end).is_some_and(u8::is_ascii_alphanumeric)
                        && !bytes.get(right_end).is_some_and(|b| matches!(b, b'_' | b'.' | b'!'))
                        && !has_external_workbook_qualifier(formula, start)
                    {
                        let right_column = column_number(&formula[right_start..right_end]);
                        if let Some(right_column) = right_column {
                            let left_absolute = bytes.get(start) == Some(&b'$');
                            let right_absolute = bytes.get(column_end + 1) == Some(&b'$');
                            let left_shifted = i64::from(column) + delta;
                            let right_shifted = i64::from(right_column) + delta;
                            let shift_left = !left_absolute && (1..=16_384).contains(&left_shifted);
                            let shift_right =
                                !right_absolute && (1..=16_384).contains(&right_shifted);
                            if shift_left || shift_right {
                                out.push_str(&formula[start..letters]);
                                if shift_left {
                                    out.push_str(&column_name_ref(left_shifted as u32));
                                } else {
                                    out.push_str(&formula[letters..column_end]);
                                }
                                out.push(':');
                                let right_letters = column_end + 1 + usize::from(right_absolute);
                                if right_absolute {
                                    out.push('$');
                                }
                                if shift_right {
                                    out.push_str(&column_name_ref(right_shifted as u32));
                                } else {
                                    out.push_str(&formula[right_letters..right_end]);
                                }
                                at = right_end;
                                continue;
                            }
                        }
                    }
                }
            }
            at = start;
        }
        let ch = formula[at..].chars().next().expect("valid boundary");
        out.push(ch);
        at += ch.len_utf8();
    }
    out
}

fn is_formula_boundary_before(bytes: &[u8], start: usize) -> bool {
    start == 0
        || bytes.get(start - 1).is_none_or(|byte| {
            !byte.is_ascii_alphanumeric() && !matches!(byte, b'_' | b'.' | b']' | b'$')
        })
}

fn has_external_workbook_qualifier(formula: &str, reference_start: usize) -> bool {
    let Some(exclamation) = formula[..reference_start].rfind('!') else {
        return false;
    };
    let gap = &formula[exclamation + 1..reference_start];
    if !gap
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'$' | b':'))
    {
        return false;
    }
    let qualifier = &formula[..exclamation];
    let start = qualifier
        .rfind(['+', '-', '*', '/', '(', ',', ';', '='])
        .map_or(0, |index| index + 1);
    let qualifier = &qualifier[start..];
    qualifier.contains('[') && qualifier.contains(']')
}

fn column_name_ref(mut number: u32) -> String {
    let mut name = String::new();
    while number > 0 {
        let remainder = (number - 1) % 26;
        name.insert(0, (b'A' + remainder as u8) as char);
        number = (number - 1) / 26;
    }
    name
}

/// Update row-relative references in a formula copied from `source_row` to `target_row`.
/// Absolute row references follow Excel's copy semantics and stay fixed.
#[cfg(test)]
pub(super) fn update_formula(
    formula: &str,
    source_row: u32,
    target_row: u32,
    row_map: &[(u32, u32)],
) -> Result<String, FillError> {
    update_formula_with_rows(formula, source_row, target_row, &row_mapping(row_map), &[])
}

#[cfg(test)]
fn row_mapping(row_map: &[(u32, u32)]) -> RowMap {
    let mut mapping = RowMap::new();
    for &(source, target) in row_map {
        mapping.entry(source).or_default().push(target);
    }
    for targets in mapping.values_mut() {
        targets.sort_unstable();
        targets.dedup();
    }
    mapping
}

/// True when a formula names another sheet. String literals and error values such as
/// `#REF!` contain `!` without referring to a sheet.
fn update_formula_with_rows(
    formula: &str,
    source_row: u32,
    target_row: u32,
    mapping: &RowMap,
    original_rows: &[u32],
) -> Result<String, FillError> {
    update_formula_with_rows_mode(
        formula,
        source_row,
        target_row,
        mapping,
        original_rows,
        false,
    )
}

fn update_formula_with_rows_mode(
    formula: &str,
    source_row: u32,
    target_row: u32,
    mapping: &RowMap,
    original_rows: &[u32],
    skip_qualified: bool,
) -> Result<String, FillError> {
    let copy_delta = if let Some(targets) = mapping.get(&source_row) {
        i64::from(target_row) - i64::from(targets[0])
    } else {
        i64::from(target_row) - i64::from(source_row)
    };
    let map = |row, absolute, range_end| {
        map_row(
            row,
            source_row,
            target_row,
            absolute,
            range_end,
            copy_delta,
            mapping,
            original_rows,
        )
    };
    let removed =
        |row: u32| !mapping.contains_key(&row) && original_rows.binary_search(&row).is_ok();
    // A range endpoint on a removed row moves toward the other endpoint, as Excel shrinks a
    // range when rows are deleted. None means every row of the range was removed.
    let shrink = |end: &RowRef, toward: u32, range_end: bool| {
        let mut row = end.row;
        loop {
            if !removed(row) {
                return map(row, end.absolute, range_end);
            }
            if row == toward {
                return None;
            }
            row = if toward > row { row + 1 } else { row - 1 };
        }
    };
    let mut out = String::with_capacity(formula.len());
    let mut at = 0;
    let mut quoted = false;
    while at < formula.len() {
        let rest = &formula[at..];
        let ch = rest.chars().next().expect("at is within formula");
        if ch == '"' {
            out.push(ch);
            at += 1;
            if quoted && formula[at..].starts_with('"') {
                out.push('"');
                at += 1;
            } else {
                quoted = !quoted;
            }
            continue;
        }
        if quoted {
            out.push(ch);
            at += ch.len_utf8();
            continue;
        }
        if let Some((first, last)) = read_reference(formula, at) {
            let end = last.as_ref().map_or(first.end, |last| last.end);
            let before = formula[..at].chars().next_back();
            let after = formula[end..].chars().next();
            let boundary_before = before
                .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_' && c != '.' && c != '[');
            let boundary_after = after
                .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_' && c != '(' && c != ']');
            if boundary_before && boundary_after {
                if skip_qualified && formula[..at].trim_end().ends_with('!') {
                    out.push_str(&formula[at..end]);
                    at = end;
                    continue;
                }
                let rows = match &last {
                    None => map(first.row, first.absolute, before == Some(':'))
                        .map(|row| vec![(&first, row)]),
                    Some(last) => shrink(&first, last.row, first.row > last.row)
                        .zip(shrink(last, first.row, last.row >= first.row))
                        .map(|(low, high)| vec![(&first, low), (last, high)]),
                };
                let Some(rows) = rows else {
                    out.push_str("#REF!");
                    at = end;
                    continue;
                };
                for (reference, row) in rows {
                    if !(1..=1_048_576).contains(&row) {
                        return Err(reference_error(
                            "A copied formula refers outside Excel's row limits",
                        ));
                    }
                    out.push_str(&formula[at..reference.row_start]);
                    out.push_str(&row.to_string());
                    at = reference.end;
                }
                continue;
            }
        }
        out.push(ch);
        at += ch.len_utf8();
    }
    if quoted {
        return Err(reference_error("A formula has an unclosed string literal"));
    }
    Ok(out)
}

/// The row part of one reference endpoint. `end` is where the endpoint ends in the formula.
struct RowRef {
    end: usize,
    row_start: usize,
    row: u32,
    absolute: bool,
}

/// Reads a cell reference, a cell range or a whole-row range such as `$2:$3`.
/// Whole-column ranges such as `A:A` have no rows and are not read.
fn read_reference(text: &str, start: usize) -> Option<(RowRef, Option<RowRef>)> {
    let colon = |end: usize| text[end..].starts_with(':');
    if let Some(first) = read_cell_ref(text, start) {
        let last = if colon(first.end) {
            read_cell_ref(text, first.end + 1)
        } else {
            None
        };
        return Some((first, last));
    }
    let first = read_row(text, start)?;
    if !colon(first.end) {
        return None;
    }
    let last = read_row(text, first.end + 1)?;
    Some((first, Some(last)))
}

fn read_cell_ref(text: &str, start: usize) -> Option<RowRef> {
    let bytes = text.as_bytes();
    let mut at = start;
    if bytes.get(at) == Some(&b'$') {
        at += 1;
    }
    let col_start = at;
    while bytes.get(at).is_some_and(u8::is_ascii_alphabetic) {
        at += 1;
    }
    if at == col_start {
        return None;
    }
    column_number(&text[col_start..at])?;
    read_row(text, at)
}

fn read_row(text: &str, start: usize) -> Option<RowRef> {
    let bytes = text.as_bytes();
    let mut at = start;
    let absolute = bytes.get(at) == Some(&b'$');
    if absolute {
        at += 1;
    }
    let row_start = at;
    while bytes.get(at).is_some_and(u8::is_ascii_digit) {
        at += 1;
    }
    if at == row_start {
        return None;
    }
    let row = text[row_start..at].parse::<u32>().ok()?;
    if row == 0 || row > 1_048_576 {
        return None;
    }
    Some(RowRef {
        end: at,
        row_start,
        row,
        absolute,
    })
}

fn column_number(text: &str) -> Option<u32> {
    let mut number = 0u32;
    for byte in text.bytes() {
        number = number
            .checked_mul(26)?
            .checked_add(u32::from(byte.to_ascii_uppercase() - b'A' + 1))?;
        if number > 16_384 {
            return None;
        }
    }
    Some(number)
}

/// Adjust worksheet geometry and formulas after rows have been cloned or removed.
/// `row_map` lists each emitted row as `(original_row, emitted_row)`.
#[cfg(test)]
pub(super) fn update_sheet(xml: &str, row_map: &[(u32, u32)]) -> Result<String, FillError> {
    let mut originals: Vec<u32> = row_map.iter().map(|(source, _)| *source).collect();
    originals.sort_unstable();
    originals.dedup();
    update_sheet_with_rows(xml, row_map, &originals)
}

#[allow(clippy::collapsible_if)]
#[cfg(test)]
pub(super) fn update_sheet_with_rows(
    xml: &str,
    row_map: &[(u32, u32)],
    original_rows: &[u32],
) -> Result<String, FillError> {
    update_sheet_with_rows_mode(xml, row_map, original_rows, false, false)
}

fn update_sheet_with_rows_mode(
    xml: &str,
    row_map: &[(u32, u32)],
    original_rows: &[u32],
    skip_qualified: bool,
    allow_arrays: bool,
) -> Result<String, FillError> {
    let tokens = tokenize(xml).map_err(reference_error)?;
    let mut emitted_to_source = std::collections::BTreeMap::new();
    let mut source_to_emitted = RowMap::new();
    for &(source, emitted) in row_map {
        if emitted == 0 || emitted > 1_048_576 || source == 0 || source > 1_048_576 {
            return Err(reference_error(
                "A worksheet row reference is outside Excel's limits",
            ));
        }
        emitted_to_source.insert(emitted, source);
        source_to_emitted.entry(source).or_default().push(emitted);
    }
    for rows in source_to_emitted.values_mut() {
        rows.sort_unstable();
        rows.dedup();
    }
    let structural = row_map.iter().any(|(source, target)| source != target)
        || source_to_emitted.values().any(|rows| rows.len() > 1)
        || original_rows
            .iter()
            .any(|source| !source_to_emitted.contains_key(source));
    let mut replacements: Vec<(usize, usize, String)> = Vec::new();
    let mut offset = 0usize;
    let mut cell_row = None;
    let mut formula_start: Option<(usize, u32, bool)> = None;
    let mut removing: Option<(usize, &str)> = None;
    for token in tokens {
        let length = match token {
            Token::Element { raw, .. } => raw.len(),
            Token::Text(text) => text.len(),
        };
        if let Some((start, removed)) = removing {
            if let Token::Element {
                kind: ElementKind::Close,
                name,
                ..
            } = token
                && name == removed
            {
                replacements.push((start, offset + length, String::new()));
                removing = None;
            }
            offset += length;
            continue;
        }
        match token {
            Token::Text(_) => {}
            Token::Element { kind, name, raw } => {
                if kind != ElementKind::Close {
                    match rewrite_geometry(raw, name, &source_to_emitted, original_rows)? {
                        Geometry::Keep => {}
                        Geometry::Replace(changed) => {
                            replacements.push((offset, offset + length, changed))
                        }
                        Geometry::Remove if kind == ElementKind::Open => {
                            removing = Some((offset, name));
                            offset += length;
                            continue;
                        }
                        Geometry::Remove => {
                            replacements.push((offset, offset + length, String::new()))
                        }
                    }
                }
                if kind == ElementKind::Open {
                    if local(name) == "c" {
                        cell_row = attr(raw, "r").and_then(cell_ref_row);
                    }
                    if local(name) == "f" && let Some(target) = cell_row {
                            let source = emitted_to_source.get(&target).copied().unwrap_or(target);
                            let formula_type = attr(raw, "t");
                            if formula_type.is_some_and(|kind| {
                                kind == "shared" || (kind == "array" && !allow_arrays)
                            }) && structural
                            {
                                return Err(reference_error(
                                    "Shared and array formulas cannot be copied safely yet",
                                ));
                            }
                            formula_start = Some((
                                offset + raw.len(),
                                source,
                                formula_type.is_some_and(|kind| kind == "array"),
                            ));
                    }
                } else if kind == ElementKind::Close {
                    if local(name) == "f"
                        && let Some((start, source, is_array)) = formula_start.take()
                    {
                            let target = cell_row.unwrap_or(source);
                            let end = offset;
                            let formula = &xml[start..end];
                            let formula = crate::xml::unescape(formula);
                            let (formula_source, formula_target) =
                                if is_array { (0, 0) } else { (source, target) };
                            let replaced = update_formula_with_rows_mode(
                                &formula,
                                formula_source,
                                formula_target,
                                &source_to_emitted,
                                original_rows,
                                skip_qualified,
                            )?;
                            let replaced = escape_formula(&replaced);
                            if replaced != xml[start..end] {
                                replacements.push((start, end, replaced));
                            }
                    }
                    if local(name) == "c" {
                        cell_row = None;
                    }
                } else if kind == ElementKind::SelfClosing && local(name) == "f"
                    && let Some(target) = cell_row
                {
                        let source = emitted_to_source.get(&target).copied().unwrap_or(target);
                        if source != target || (structural && !allow_arrays) {
                            return Err(reference_error(
                                "Shared and array formulas cannot be copied safely yet",
                            ));
                        }
                }
            }
        }
        offset += length;
    }
    replacements.sort_by_key(|(start, ..)| *start);
    let mut out = String::with_capacity(xml.len());
    let mut at = 0usize;
    for (start, end, replacement) in replacements {
        if start < at {
            return Err(reference_error("Overlapping spreadsheet XML edits"));
        }
        out.push_str(&xml[at..start]);
        out.push_str(&replacement);
        at = end;
    }
    out.push_str(&xml[at..]);
    update_counts(&out)
}

#[allow(clippy::too_many_arguments)]
fn map_row(
    row: u32,
    formula_source: u32,
    formula_target: u32,
    absolute: bool,
    range_end: bool,
    copy_delta: i64,
    mapping: &RowMap,
    original_rows: &[u32],
) -> Option<u32> {
    if !absolute && row == formula_source {
        return Some(formula_target);
    }
    let structural = if let Some(targets) = mapping.get(&row) {
        if range_end {
            targets.last().copied()
        } else {
            targets.first().copied()
        }
    } else if original_rows.binary_search(&row).is_ok() {
        None
    } else if let Some((source, targets)) = mapping.range(..row).next_back() {
        targets.last().copied()?.checked_add(row - source)
    } else if let Some((source, targets)) = mapping.first_key_value() {
        targets
            .first()
            .and_then(|target| row.checked_add(target.checked_sub(*source)?))
            .or(Some(row))
    } else {
        Some(row)
    }?;
    if absolute {
        Some(structural)
    } else {
        i64::from(structural)
            .checked_add(copy_delta)
            .and_then(|mapped| u32::try_from(mapped).ok())
    }
}

fn escape_formula(formula: &str) -> String {
    let mut out = String::with_capacity(formula.len());
    for ch in formula.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Sets `count` on collections whose items a loop copied or removed. The schema requires
/// at least one item in each, so a collection left empty is removed.
fn update_counts(xml: &str) -> Result<String, FillError> {
    let mut xml = xml.to_string();
    for (container, item) in [
        ("mergeCells", "mergeCell"),
        ("dataValidations", "dataValidation"),
    ] {
        xml = update_count(&xml, container, item)?;
    }
    Ok(xml)
}

fn update_count(xml: &str, container_name: &str, item: &str) -> Result<String, FillError> {
    let tokens = tokenize(xml).map_err(reference_error)?;
    let mut count = 0usize;
    let mut container = None;
    let mut container_end = None;
    let mut offset = 0usize;
    for token in &tokens {
        let length = match token {
            Token::Element { raw, .. } => raw.len(),
            Token::Text(text) => text.len(),
        };
        if let Token::Element { kind, name, raw } = token {
            match kind {
                ElementKind::SelfClosing | ElementKind::Open if local(name) == item => count += 1,
                ElementKind::SelfClosing if local(name) == container_name => {
                    container = Some((offset, offset + length, *raw));
                    container_end = Some(offset + length);
                }
                ElementKind::Open if local(name) == container_name => {
                    container = Some((offset, offset + length, *raw))
                }
                ElementKind::Close if local(name) == container_name => {
                    container_end = Some(offset + length)
                }
                _ => {}
            }
        }
        offset += length;
    }
    let Some((start, open_end, raw)) = container else {
        return Ok(xml.to_string());
    };
    if count == 0 {
        let end =
            container_end.ok_or_else(|| reference_error("Unclosed spreadsheet collection"))?;
        return Ok(format!("{}{}", &xml[..start], &xml[end..]));
    }
    let updated = set_attribute(raw, "count", &count.to_string());
    Ok(format!("{}{}{}", &xml[..start], updated, &xml[open_end..]))
}

enum Geometry {
    Keep,
    Replace(String),
    /// Drop the whole element, children included.
    Remove,
}

fn rewrite_geometry(
    raw: &str,
    element: &str,
    map: &RowMap,
    original_rows: &[u32],
) -> Result<Geometry, FillError> {
    match local(element) {
        "dimension" => rewrite_dimension(raw, map, original_rows),
        "mergeCell" => rewrite_merge(raw, map, original_rows),
        "selection" => rewrite_selection(raw, map, original_rows),
        "conditionalFormatting" | "dataValidation" => {
            let Some((_, _, value)) = attribute_range(raw, "sqref") else {
                return Ok(Geometry::Keep);
            };
            let Some(next) = remap_reference_list(value, map, original_rows)? else {
                return Ok(Geometry::Keep);
            };
            if next.is_empty() {
                return Ok(Geometry::Remove);
            }
            Ok(Geometry::Replace(with_value(raw, "sqref", &next)))
        }
        "autoFilter" | "hyperlink" | "table" | "f" => {
            let Some((_, _, value)) = attribute_range(raw, "ref") else {
                return Ok(Geometry::Keep);
            };
            let Some(next) = remap_reference_list(value, map, original_rows)? else {
                return Ok(Geometry::Keep);
            };
            if next.is_empty() {
                return Err(reference_error(
                    "Spreadsheet geometry refers only to rows removed by a loop",
                ));
            }
            Ok(Geometry::Replace(with_value(raw, "ref", &next)))
        }
        _ => Ok(Geometry::Keep),
    }
}

fn rewrite_dimension(
    raw: &str,
    map: &RowMap,
    original_rows: &[u32],
) -> Result<Geometry, FillError> {
    if map.is_empty() {
        return Ok(Geometry::Remove);
    }
    let Some((_, _, value)) = attribute_range(raw, "ref") else {
        return Ok(Geometry::Keep);
    };
    let Some((left, right)) = value.split_once(':') else {
        return Ok(Geometry::Keep);
    };
    let Some((left_col, first)) = split_cell(left) else {
        return Ok(Geometry::Keep);
    };
    let Some((right_col, last)) = split_cell(right) else {
        return Ok(Geometry::Keep);
    };
    let (low, high) = (first.min(last), first.max(last));
    let first = map_row(
        first,
        u32::MAX,
        u32::MAX,
        false,
        false,
        0,
        map,
        original_rows,
    )
    .or_else(|| {
        map.range(low..=high)
            .find_map(|(_, rows)| rows.first().copied())
    })
    .ok_or_else(|| reference_error("Worksheet dimension has no surviving rows"))?;
    let last = map_row(last, u32::MAX, u32::MAX, false, true, 0, map, original_rows)
        .or_else(|| {
            map.range(low..=high)
                .next_back()
                .and_then(|(_, rows)| rows.last().copied())
        })
        .ok_or_else(|| reference_error("Worksheet dimension has no surviving rows"))?;
    let updated = format!("{left_col}{first}:{right_col}{last}");
    if updated == value {
        return Ok(Geometry::Keep);
    }
    Ok(Geometry::Replace(with_value(raw, "ref", &updated)))
}

fn rewrite_merge(raw: &str, map: &RowMap, original_rows: &[u32]) -> Result<Geometry, FillError> {
    let Some((_, _, value)) = attribute_range(raw, "ref") else {
        return Ok(Geometry::Keep);
    };
    let Some((left, right)) = value.split_once(':') else {
        return Ok(Geometry::Keep);
    };
    let Some((left_col, first)) = split_cell(left) else {
        return Ok(Geometry::Keep);
    };
    let Some((right_col, last)) = split_cell(right) else {
        return Ok(Geometry::Keep);
    };
    if first == last
        && let Some(rows) = map.get(&first).filter(|rows| rows.len() > 1)
    {
        let copies = rows
            .iter()
            .map(|row| with_value(raw, "ref", &format!("{left_col}{row}:{right_col}{row}")));
        return Ok(Geometry::Replace(copies.collect()));
    }
    if first != last
        && map
            .range(first.min(last)..=first.max(last))
            .any(|(_, rows)| rows.len() > 1)
    {
        return Err(reference_error(
            "A merged range crossing a repeated row cannot be updated safely",
        ));
    }
    let Some(next) = remap_reference_list(value, map, original_rows)? else {
        return Ok(Geometry::Keep);
    };
    // A merge left with one cell or none merges nothing.
    match next.split_once(':') {
        Some((left, right)) if left != right => {
            Ok(Geometry::Replace(with_value(raw, "ref", &next)))
        }
        _ => Ok(Geometry::Remove),
    }
}

/// `activeCell` is one cell, so a repeated row keeps it on the first copy. When its row or
/// every `sqref` range is removed, the selection falls back to a surviving cell, else A1.
fn rewrite_selection(
    raw: &str,
    map: &RowMap,
    original_rows: &[u32],
) -> Result<Geometry, FillError> {
    let sqref = attribute_range(raw, "sqref").map(|(_, _, value)| value);
    let active = attribute_range(raw, "activeCell").map(|(_, _, value)| value);
    let mut next_sqref = match sqref {
        Some(value) => remap_reference_list(value, map, original_rows)?,
        None => None,
    };
    let mut next_active = None;
    let mut active_removed = false;
    if let Some((col, row)) = active.and_then(split_cell) {
        match map.get(&row) {
            Some(rows) if rows[0] != row => next_active = Some(format!("{col}{}", rows[0])),
            Some(_) => {}
            None => active_removed = original_rows.binary_search(&row).is_ok(),
        }
    }
    let sqref_now = next_sqref.as_deref().or(sqref).unwrap_or_default();
    if active_removed {
        let first = sqref_now
            .split_ascii_whitespace()
            .next()
            .and_then(|token| token.split(':').next());
        next_active = Some(first.unwrap_or("A1").to_string());
    }
    if sqref.is_some() && sqref_now.is_empty() {
        let active_now = next_active.as_deref().or(active).unwrap_or("A1");
        next_sqref = Some(active_now.to_string());
    }
    if next_sqref.is_none() && next_active.is_none() {
        return Ok(Geometry::Keep);
    }
    let mut changed = raw.to_string();
    if let Some(value) = &next_active {
        changed = with_value(&changed, "activeCell", value);
    }
    if let Some(value) = &next_sqref {
        changed = with_value(&changed, "sqref", value);
    }
    Ok(Geometry::Replace(changed))
}

/// Replaces an existing attribute value in an opening tag.
fn with_value(raw: &str, name: &str, value: &str) -> String {
    let Some((start, end, _)) = attribute_range(raw, name) else {
        return raw.to_string();
    };
    let mut changed = raw.to_string();
    changed.replace_range(start..end, value);
    changed
}

fn attribute_range<'a>(raw: &'a str, name: &str) -> Option<(usize, usize, &'a str)> {
    let mut from = 0usize;
    while let Some(found) = raw[from..].find(name) {
        let start = from + found;
        let before = raw[..start].chars().next_back();
        let mut at = start + name.len();
        let after = raw[at..].chars().next();
        from = at;
        if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
            || after.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
        {
            continue;
        }
        while raw.as_bytes().get(at).is_some_and(u8::is_ascii_whitespace) {
            at += 1;
        }
        if raw.as_bytes().get(at) != Some(&b'=') {
            continue;
        }
        at += 1;
        while raw.as_bytes().get(at).is_some_and(u8::is_ascii_whitespace) {
            at += 1;
        }
        let quote = *raw.as_bytes().get(at)?;
        if quote != b'"' && quote != b'\'' {
            continue;
        }
        let value_start = at + 1;
        let end = raw[value_start..].find(char::from(quote))? + value_start;
        return Some((value_start, end, &raw[value_start..end]));
    }
    None
}

fn remap_reference_list(
    value: &str,
    map: &RowMap,
    original_rows: &[u32],
) -> Result<Option<String>, FillError> {
    let mut output = Vec::new();
    let mut changed = false;
    for token in value.split_ascii_whitespace() {
        let Some((left, right)) = token.split_once(':') else {
            let Some((col, row)) = split_cell(token) else {
                output.push(token.to_string());
                continue;
            };
            let Some(rows) = map.get(&row) else {
                if original_rows.binary_search(&row).is_ok() {
                    changed = true;
                    continue;
                }
                output.push(token.to_string());
                continue;
            };
            changed = true;
            output.extend(rows.iter().map(|row| format!("{col}{row}")));
            continue;
        };
        let Some((left_col, first)) = split_cell(left) else {
            output.push(token.to_string());
            continue;
        };
        let Some((right_col, last)) = split_cell(right) else {
            output.push(token.to_string());
            continue;
        };
        let low = first.min(last);
        let high = first.max(last);
        let rows: Vec<u32> = map
            .range(low..=high)
            .flat_map(|(_, rows)| rows.iter().copied())
            .collect();
        if rows.is_empty() {
            // A range made only of removed rows is dropped; one that also covers blank rows is not safe to guess.
            let removed = original_rows
                .iter()
                .filter(|row| (low..=high).contains(*row))
                .count();
            if removed as u64 == u64::from(high - low) + 1 {
                changed = true;
                continue;
            }
            if removed > 0 {
                return Err(reference_error(
                    "Spreadsheet geometry refers only to rows removed by a loop",
                ));
            }
            output.push(token.to_string());
            continue;
        }
        if rows.iter().any(|row| *row < low || *row > high)
            || rows.len() != (high - low + 1) as usize
            || rows
                .iter()
                .enumerate()
                .any(|(index, row)| index > 0 && rows[index - 1] + 1 != *row)
        {
            changed = true;
        }
        let first_out = *rows.first().expect("nonempty");
        let last_out = *rows.last().expect("nonempty");
        if first_out != first || last_out != last {
            changed = true;
        }
        output.push(format!("{left_col}{first_out}:{right_col}{last_out}"));
    }
    if changed {
        Ok(Some(output.join(" ")))
    } else {
        Ok(None)
    }
}

fn split_cell(reference: &str) -> Option<(&str, u32)> {
    let mut at = 0;
    while reference
        .as_bytes()
        .get(at)
        .is_some_and(u8::is_ascii_alphabetic)
    {
        at += 1;
    }
    if at == 0 {
        return None;
    }
    let column = &reference[..at];
    column_number(column)?;
    let row = reference[at..].parse::<u32>().ok()?;
    (row > 0 && row <= 1_048_576).then_some((column, row))
}

fn local(name: &str) -> &str {
    name.rsplit_once(':').map_or(name, |(_, local)| local)
}

fn attr<'a>(raw: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = raw;
    while let Some(at) = rest.find(name) {
        let before = rest[..at].chars().next_back();
        let tail = &rest[at + name.len()..];
        rest = tail;
        if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':') {
            continue;
        }
        let value = tail.trim_start().strip_prefix('=')?.trim_start();
        let quote = value.chars().next()?;
        if quote != '"' && quote != '\'' {
            continue;
        }
        return Some(&value[1..value[1..].find(quote)? + 1]);
    }
    None
}

fn cell_ref_row(reference: &str) -> Option<u32> {
    let bytes = reference.as_bytes();
    let mut at = usize::from(bytes.first() == Some(&b'$'));
    while bytes.get(at) == Some(&b'$') || bytes.get(at).is_some_and(u8::is_ascii_alphabetic) {
        at += 1;
    }
    if bytes.get(at) == Some(&b'$') {
        at += 1;
    }
    let row = reference[at..].parse::<u32>().ok()?;
    (row > 0 && row <= 1_048_576).then_some(row)
}

fn reference_error(message: impl Into<String>) -> FillError {
    FillError::new(message).with_id(FillErrorId::SpreadsheetReference)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_relative_rows_but_keeps_absolute_rows_and_strings() {
        assert_eq!(
            update_formula("A2*$B$1+C$4+$D5+\"A2\"", 2, 3, &[]).unwrap(),
            "A3*$B$1+C$4+$D6+\"A2\""
        );
    }

    #[test]
    fn translates_sheet_qualified_and_range_references() {
        assert_eq!(
            update_formula("'Other Sheet'!A2:B9", 2, 4, &[]).unwrap(),
            "'Other Sheet'!A4:B11"
        );
    }

    #[test]
    fn rejects_a_copy_that_moves_references_outside_sheet() {
        assert!(update_formula("A1", 2, 1, &[]).is_err());
        assert!(update_formula("A2", 1, 1_048_576, &[]).is_err());
    }

    #[test]
    fn updates_geometry_and_copied_formula_rows() {
        let xml = r#"<worksheet><dimension ref="A1:B3"/><sheetData><row r="1"><c r="A1"><f>A2*$B$1</f></c></row><row r="2"><c r="A2"><f>A2*$B$1</f></c></row><row r="3"><c r="A3"><f>A2*$B$1</f></c></row><row r="4"><c r="A4"><f>A3*$B$1</f></c></row></sheetData><mergeCells count="1"><mergeCell ref="A2:B2"/></mergeCells></worksheet>"#;
        let actual = update_sheet(xml, &[(1, 1), (2, 2), (2, 3), (3, 4)]).unwrap();
        assert!(actual.contains(r#"<dimension ref="A1:B4"/>"#));
        assert!(actual.contains(r#"<mergeCell ref="A2:B2"/><mergeCell ref="A3:B3"/>"#));
        assert!(actual.contains(r#"<mergeCells count="2">"#));
        assert!(actual.contains("<c r=\"A3\"><f>A3*$B$1</f></c>"));
        assert!(actual.contains("<c r=\"A4\"><f>A4*$B$1</f></c>"));
    }

    #[test]
    fn expands_single_row_references_to_each_copy() {
        let map = std::collections::BTreeMap::from([(2, vec![2, 3])]);
        assert_eq!(
            remap_reference_list("A2 A5", &map, &[]).unwrap().as_deref(),
            Some("A2 A3 A5")
        );
        assert_eq!(
            remap_reference_list("A2:B2", &map, &[]).unwrap().as_deref(),
            Some("A2:B3")
        );
    }

    #[test]
    fn qualified_maps_handle_quoted_operator_sheet_names_structurally() {
        let map = WorksheetMap::new(&[(1, 1), (2, 2), (2, 3), (3, 4)], Vec::new(), vec![1, 2, 3]);
        let maps = std::collections::BTreeMap::from([
            ("A+B".to_string(), map.clone()),
            ("Data".to_string(), map),
        ]);
        assert_eq!(
            transform_qualified_formula_refs("'A+B'!$A$1:$B$2+Data!A2", &maps).unwrap(),
            "'A+B'!$A$1:$B$3+Data!A2"
        );
    }

    #[test]
    fn external_workbook_references_stay_opaque() {
        let map = WorksheetMap::new(&[(1, 1), (2, 3)], Vec::new(), vec![1, 2]);
        let maps = std::collections::BTreeMap::from([("Data".to_string(), map)]);
        assert_eq!(
            transform_qualified_formula_refs(
                "'[Book.xlsx]Data'!A2+[Book.xlsx]Data!A2+Data!A2",
                &maps,
            )
            .unwrap(),
            "'[Book.xlsx]Data'!A2+[Book.xlsx]Data!A2+Data!A3"
        );
        assert_eq!(shift_formula_columns("[Book.xlsx]Data!A2+Data!A2", 1), "[Book.xlsx]Data!A2+Data!B2");
    }

    #[test]
    fn horizontal_formula_shift_respects_lexer_boundaries() {
        assert_eq!(
            shift_formula_columns("'Data2'!A2+A1!A2+LOG10(A2)", 1),
            "'Data2'!B2+A1!B2+LOG10(B2)"
        );
        assert_eq!(
            shift_formula_columns("[Book.xlsx]Data!A2+A2", 1),
            "[Book.xlsx]Data!A2+B2"
        );
        assert_eq!(
            shift_formula_columns("[Book.xlsx]Data!A2:B2+A2", 1),
            "[Book.xlsx]Data!A2:B2+B2"
        );
        assert_eq!(shift_formula_columns("SUM(A:A)", 1), "SUM(B:B)");
        assert_eq!(shift_formula_columns("SUM($A:B)", 1), "SUM($A:C)");
        assert_eq!(shift_formula_columns("SUM(A:$B)", 1), "SUM(B:$B)");
    }

    #[test]
    fn three_dimensional_refs_require_uniform_row_transforms() {
        let uniform = WorksheetMap::new(
            &[(1, 1), (2, 3), (3, 4), (4, 5)],
            Vec::new(),
            vec![1, 2, 3, 4],
        );
        let maps = std::collections::BTreeMap::from([
            ("Sheet1".to_string(), uniform.clone()),
            ("Sheet3".to_string(), uniform),
        ]);
        assert_eq!(
            transform_qualified_formula_refs("SUM(Sheet1:Sheet3!A2:A4)", &maps).unwrap(),
            "SUM(Sheet1:Sheet3!A3:A5)"
        );
        let divergent = WorksheetMap::new(
            &[(1, 1), (2, 4), (3, 5), (4, 6)],
            Vec::new(),
            vec![1, 2, 3, 4],
        );
        let maps = std::collections::BTreeMap::from([
            (
                "Sheet1".to_string(),
                WorksheetMap::new(
                    &[(1, 1), (2, 3), (3, 4), (4, 5)],
                    Vec::new(),
                    vec![1, 2, 3, 4],
                ),
            ),
            ("Sheet3".to_string(), divergent),
        ]);
        assert!(transform_qualified_formula_refs("SUM(Sheet1:Sheet3!A2:A4)", &maps).is_err());

        let middle = WorksheetMap::new(
            &[(1, 1), (2, 6), (3, 7), (4, 8)],
            Vec::new(),
            vec![1, 2, 3, 4],
        );
        let endpoints = WorksheetMap::new(
            &[(1, 1), (2, 3), (3, 4), (4, 5)],
            Vec::new(),
            vec![1, 2, 3, 4],
        );
        let maps = std::collections::BTreeMap::from([
            ("Sheet1".to_string(), endpoints.clone()),
            ("Sheet2".to_string(), middle),
            ("Sheet3".to_string(), endpoints),
        ]);
        assert!(transform_qualified_formula_refs("SUM(Sheet1:Sheet3!A2:A4)", &maps).is_err());
        let columns_one = WorksheetMap::new(
            &[(1, 1), (2, 2)],
            vec![CellCoordinate {
                source_row: 1,
                source_column: 2,
                target_row: 1,
                target_column: 3,
            }],
            vec![1, 2],
        );
        let columns_two = WorksheetMap::new(
            &[(1, 1), (2, 2)],
            vec![CellCoordinate {
                source_row: 1,
                source_column: 2,
                target_row: 1,
                target_column: 4,
            }],
            vec![1, 2],
        );
        let maps = std::collections::BTreeMap::from([
            ("Sheet1".to_string(), columns_one),
            ("Sheet3".to_string(), columns_two),
        ]);
        assert!(transform_qualified_formula_refs("SUM(Sheet1:Sheet3!A1:B1)", &maps).is_err());
    }

    #[test]
    fn horizontal_table_expansion_rebuilds_columns_with_unique_names_and_ids() {
        let cells = vec![
            CellCoordinate {
                source_row: 1,
                source_column: 1,
                target_row: 1,
                target_column: 1,
            },
            CellCoordinate {
                source_row: 1,
                source_column: 2,
                target_row: 1,
                target_column: 2,
            },
            CellCoordinate {
                source_row: 1,
                source_column: 2,
                target_row: 1,
                target_column: 3,
            },
            CellCoordinate {
                source_row: 2,
                source_column: 1,
                target_row: 2,
                target_column: 1,
            },
            CellCoordinate {
                source_row: 2,
                source_column: 2,
                target_row: 2,
                target_column: 2,
            },
            CellCoordinate {
                source_row: 2,
                source_column: 2,
                target_row: 2,
                target_column: 3,
            },
        ];
        let map = WorksheetMap::new(&[(1, 1), (2, 2)], cells, vec![1, 2]);
        let table = r#"<table ref="A1:B2"><tableColumns count="2"><tableColumn id="1" name="Name"/><tableColumn id="2" name="Value"><calculatedColumnFormula>1</calculatedColumnFormula></tableColumn></tableColumns></table>"#;
        let sheet = r#"<worksheet/>"#;
        let updated = update_table_part_with_sheet(table, &map, sheet).unwrap();
        assert!(updated.contains("ref=\"A1:C2\""), "{updated}");
        assert!(updated.contains("count=\"3\""), "{updated}");
        assert!(updated.contains("id=\"3\" name=\"Value_2\""), "{updated}");
        assert_eq!(updated.matches("<calculatedColumnFormula>").count(), 2);
    }

    #[test]
    fn shared_formula_groups_expand_to_relative_standalone_formulas() {
        let xml = r#"<worksheet><sheetData><row r="1"><c r="A1"><f t="shared" si="0" ref="A1:A2">A1+$B$1</f></c></row><row r="2"><c r="A2"><f t="shared" si="0"/></c></row></sheetData></worksheet>"#;
        let normalized = normalize_shared_formulas(xml).unwrap();
        assert!(normalized.contains("<f>A1+$B$1</f>"), "{normalized}");
        assert!(normalized.contains("<f>A2+$B$1</f>"), "{normalized}");
        assert!(!normalized.contains("t=\"shared\""), "{normalized}");
    }

    #[test]
    fn drawing_anchors_clone_for_repeated_cells_with_unique_shape_ids() {
        let xml = r#"<wsDr><twoCellAnchor><from><col>0</col><row>1</row></from><to><col>1</col><row>2</row></to><pic><nvPicPr><cNvPr id="5" name="Picture 1"/></nvPicPr></pic></twoCellAnchor></wsDr>"#;
        let cells = vec![
            CellCoordinate {
                source_row: 2,
                source_column: 1,
                target_row: 2,
                target_column: 1,
            },
            CellCoordinate {
                source_row: 2,
                source_column: 1,
                target_row: 3,
                target_column: 1,
            },
        ];
        let map = WorksheetMap::new(
            &[(1, 1), (2, 3), (2, 4), (3, 5)],
            cells.clone(),
            vec![1, 2, 3],
        );
        let output = transform_anchors(xml, &map, &cells).unwrap();
        assert_eq!(output.matches("<twoCellAnchor").count(), 2, "{output}");
        assert!(output.contains("<row>2</row>"), "{output}");
        assert!(output.contains("<row>3</row>"), "{output}");
        assert!(output.contains("id=\"5\""), "{output}");
        assert!(output.contains("id=\"6\""), "{output}");
    }

    #[test]
    fn array_formula_moves_as_one_range_and_rejects_repeated_spans() {
        let xml = r#"<worksheet><dimension ref="A1:A3"/><sheetData><row r="2"><c r="A2"><f t="array" ref="A2:A3">SUM(A2:A3)</f><v>0</v></c></row><row r="3"><c r="A3"><v>0</v></c></row></sheetData></worksheet>"#;
        let cells = vec![
            CellCoordinate {
                source_row: 2,
                source_column: 1,
                target_row: 3,
                target_column: 1,
            },
            CellCoordinate {
                source_row: 3,
                source_column: 1,
                target_row: 4,
                target_column: 1,
            },
        ];
        let map = WorksheetMap::new(&[(2, 3), (3, 4)], cells.clone(), vec![2, 3]);
        let updated = update_sheet_with_map(xml, &map).unwrap();
        assert!(updated.contains("t=\"array\" ref=\"A3:A4\""), "{updated}");
        assert!(updated.contains("SUM(A3:A4)"), "{updated}");
        let repeated = WorksheetMap::new(
            &[(2, 2), (2, 3), (3, 4)],
            vec![
                CellCoordinate {
                    source_row: 2,
                    source_column: 1,
                    target_row: 2,
                    target_column: 1,
                },
                CellCoordinate {
                    source_row: 2,
                    source_column: 1,
                    target_row: 3,
                    target_column: 1,
                },
            ],
            vec![2, 3],
        );
        assert!(update_sheet_with_map(xml, &repeated).is_err());
    }

    #[test]
    fn expands_footer_ranges_and_remaps_absolute_rows() {
        let map = [(1, 1), (2, 2), (2, 3), (3, 4)];
        assert_eq!(
            update_formula("SUM(B2:B2)", 5, 5, &map).unwrap(),
            "SUM(B2:B3)"
        );
        assert_eq!(
            update_formula("$B$2+$B$3", 2, 3, &map).unwrap(),
            "$B$2+$B$4"
        );
    }

    #[test]
    fn remaps_references_in_static_formulas_after_insertions() {
        let map = [(1, 1), (2, 2), (2, 3), (3, 4)];
        assert_eq!(
            update_formula("A3+SUM(B2:B2)", 1, 1, &map).unwrap(),
            "A4+SUM(B2:B3)"
        );
    }

    #[test]
    fn copied_formula_moves_relative_references_to_other_rows() {
        let map = [(1, 1), (2, 2), (2, 3), (3, 4)];
        assert_eq!(update_formula("A1+$A$1", 2, 3, &map).unwrap(), "A2+$A$1");
    }

    #[test]
    fn deleted_rows_become_ref_errors_and_geometry_is_trimmed() {
        let map = [(1, 1), (3, 2)];
        assert_eq!(
            update_formula_with_rows("A2+A3", 1, 1, &row_mapping(&map), &[1, 2, 3]).unwrap(),
            "#REF!+A2"
        );
        let xml = r#"<worksheet><dimension ref="A1:B3"/><sheetData><row r="1"/><row r="2"/></sheetData><mergeCells count="0"><mergeCell ref="A2:B2"/></mergeCells></worksheet>"#;
        let result = update_sheet_with_rows(xml, &map, &[1, 2, 3]).unwrap();
        assert!(result.contains(r#"<dimension ref="A1:B2"/>"#));
        assert!(!result.contains("mergeCell"), "{result}");
    }

    #[test]
    fn dimension_keeps_blank_tail_rows_after_structural_insertion() {
        let map = [(1, 1), (2, 2), (2, 3), (3, 4)];
        let xml = r#"<worksheet><dimension ref="A1:B100"/><sheetData><row r="1"/><row r="2"/><row r="3"/><row r="4"/></sheetData></worksheet>"#;
        let result = update_sheet_with_rows(xml, &map, &[1, 2, 3]).unwrap();
        assert!(result.contains(r#"<dimension ref="A1:B101"/>"#));
    }
}
