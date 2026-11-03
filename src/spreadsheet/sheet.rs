//! Worksheet parsing and row-loop rendering.

use serde_json::Value;
use std::collections::BTreeMap;

use crate::render::FillHooks;
use crate::xml::{ElementKind, Token, read_attribute, tokenize};
use crate::{FillError, FillErrorId, FillOptions};

use super::{
    cells::Cell,
    error,
    references::CellCoordinate,
    text::{self, Context, Kind, Tag},
    xmlutil::{self, Element},
};

const MAX_ROWS: u32 = 1_048_576;
const MAX_COLUMNS: u32 = 16_384;
const MAX_CELLS: usize = 1_000_000;
const MAX_LOOP_DEPTH: usize = 64;
type RenderedRow = (String, Vec<(u32, u32)>, usize);

#[derive(Debug)]
struct Row {
    xml: String,
    number: u32,
    cells: Vec<Cell>,
    cell_spans: Vec<(usize, usize)>,
    columns: Vec<u32>,
}

#[derive(Debug, Clone, Copy)]
struct TagLocation {
    row: usize,
    cell: usize,
    tag: usize,
}

#[derive(Debug)]
struct LoopBlock {
    start: usize,
    end: usize,
    open: TagLocation,
    inverted: bool,
    children: Vec<LoopBlock>,
}

#[derive(Debug, Clone)]
struct HorizontalBlock {
    start_cell: usize,
    end_cell: usize,
    open_tag: usize,
    close_tag: Option<usize>,
    start_column: u32,
    end_column: u32,
    inverted: bool,
}

#[derive(Debug, Clone)]
pub(super) struct InspectedTag {
    pub cell: String,
    pub name: String,
    pub written: String,
    pub kind: Kind,
}

struct PlacedRow {
    source: u32,
    target: u32,
    xml: String,
    cells: Vec<CellCoordinate>,
}

/// A parsed worksheet with cell text tags and row loops.
#[derive(Debug)]
pub(super) struct Sheet {
    xml: String,
    sheet_data: Element,
    rows: Vec<Row>,
    blocks: Vec<LoopBlock>,
    removed_tags: BTreeMap<(usize, usize), Vec<usize>>,
    horizontal: BTreeMap<usize, Vec<HorizontalBlock>>,
}

pub(super) struct RenderedSheet {
    pub xml: String,
    pub row_map: Vec<(u32, u32)>,
    pub cell_map: Vec<CellCoordinate>,
    pub effects: Vec<super::effects::CellEffect>,
}

impl Sheet {
    pub(super) fn parse(
        xml: &str,
        shared: &[String],
        options: &FillOptions,
    ) -> Result<Self, FillError> {
        let sheet_data = xmlutil::elements(xml)?
            .into_iter()
            .find(|element| xmlutil::local_name(&element.name) == "sheetData")
            .ok_or_else(|| error("Worksheet has no sheetData element"))?;
        let row_elements = direct_children(&xml[sheet_data.open_end..sheet_data.close_start])?
            .into_iter()
            .filter(|element| xmlutil::local_name(&element.name) == "row")
            .collect::<Vec<_>>();
        if row_elements.len() > MAX_ROWS as usize {
            return Err(limit_error("Worksheet row count exceeds the Excel limit"));
        }

        let mut rows = Vec::with_capacity(row_elements.len());
        let mut seen_rows = std::collections::BTreeSet::new();
        let mut cell_count = 0usize;
        let mut previous_row = 0;
        for element in row_elements {
            let sheet_data_start = sheet_data.open_end;
            let row_xml =
                xml[sheet_data_start + element.start..sheet_data_start + element.end].to_string();
            let row_open = &row_xml[..element.open_end - element.start];
            let number = read_attribute(row_open, "r")
                .and_then(|raw| raw.parse::<u32>().ok())
                .filter(|number| (1..=MAX_ROWS).contains(number))
                .ok_or_else(|| error("Worksheet row has an invalid row number"))?;
            if !seen_rows.insert(number) {
                return Err(error("Worksheet has duplicate row numbers"));
            }
            if number <= previous_row {
                return Err(error("Worksheet rows are not in ascending order"));
            }
            previous_row = number;
            let row_inner = if element.close_start == element.open_end {
                ""
            } else {
                &row_xml[element.open_end - element.start..element.close_start - element.start]
            };
            let cell_elements = direct_children(row_inner)?
                .into_iter()
                .filter(|cell| xmlutil::local_name(&cell.name) == "c")
                .collect::<Vec<_>>();
            cell_count = cell_count
                .checked_add(cell_elements.len())
                .ok_or_else(|| limit_error("Worksheet cell count overflow"))?;
            if cell_count > MAX_CELLS {
                return Err(limit_error("Worksheet exceeds one million cells"));
            }
            let mut cells = Vec::with_capacity(cell_elements.len());
            let mut spans = Vec::with_capacity(cell_elements.len());
            let mut previous_column = 0;
            let mut columns = Vec::with_capacity(cell_elements.len());
            for cell in cell_elements {
                let start = element.open_end - element.start + cell.start;
                let end = element.open_end - element.start + cell.end;
                let cell_xml = &row_xml[start..end];
                let (column, referenced_row) = validate_cell_reference(cell_xml)?;
                if referenced_row != number {
                    return Err(error("Worksheet cell reference does not match its row"));
                }
                if column <= previous_column {
                    return Err(error(
                        "Worksheet cells are duplicate or out of column order",
                    ));
                }
                previous_column = column;
                columns.push(column);
                let reference = read_attribute(
                    &cell_xml[..cell_xml.find('>').unwrap_or(cell_xml.len())],
                    "r",
                )
                .unwrap_or("?");
                cells.push(
                    Cell::parse(cell_xml, shared, options)
                        .map_err(|failure| annotate_cell_error(failure, reference))?,
                );
                spans.push((start, end));
            }
            rows.push(Row {
                xml: row_xml,
                number,
                cells,
                cell_spans: spans,
                columns,
            });
        }

        // Row loops cross a cell boundary or a row boundary. Paired tags inside one cell stay
        // with the cell renderer, which handles inline loops.
        let mut locations = Vec::new();
        let mut tags = Vec::new();
        for (row_index, row) in rows.iter().enumerate() {
            for (cell_index, cell) in row.cells.iter().enumerate() {
                for (tag_index, tag) in cell.tags.iter().enumerate() {
                    locations.push(TagLocation {
                        row: row_index,
                        cell: cell_index,
                        tag: tag_index,
                    });
                    tags.push(tag);
                }
            }
        }
        let horizontal = pair_horizontal_loops(&tags, &locations, &rows)?;
        let (blocks, row_loop_tags) = pair_row_loops(&tags, &locations)?;
        let structural_loops = !blocks.is_empty() || !horizontal.is_empty();
        if structural_loops && xml.contains("shared") {
            let normalized = super::references::normalize_shared_formulas(xml)?;
            if normalized != xml {
                return Self::parse(&normalized, shared, options);
            }
        }
        let mut removed_tags = BTreeMap::<(usize, usize), Vec<usize>>::new();
        for location in &row_loop_tags {
            removed_tags
                .entry((location.row, location.cell))
                .or_default()
                .push(location.tag);
        }
        for (row, blocks) in &horizontal {
            for block in blocks {
                removed_tags
                    .entry((*row, block.start_cell))
                    .or_default()
                    .push(block.open_tag);
                if let Some(close) = block.close_tag {
                    removed_tags
                        .entry((*row, block.end_cell))
                        .or_default()
                        .push(close);
                }
            }
        }
        Ok(Self {
            xml: xml.to_string(),
            sheet_data,
            rows,
            blocks,
            removed_tags,
            horizontal,
        })
    }

    #[cfg(test)]
    pub(super) fn render(
        &self,
        data: &Value,
        hooks: &mut dyn FillHooks,
        part: &str,
        date_1904: bool,
    ) -> Result<RenderedSheet, FillError> {
        self.render_with_limits(
            data,
            hooks,
            part,
            date_1904,
            &super::WorkbookLimits::default(),
        )
    }

    pub(super) fn render_with_limits(
        &self,
        data: &Value,
        hooks: &mut dyn FillHooks,
        part: &str,
        date_1904: bool,
        limits: &super::WorkbookLimits,
    ) -> Result<RenderedSheet, FillError> {
        let mut context = Context::new(part, hooks, date_1904);
        context.max_loop_depth = limits.max_loop_depth;
        let scopes = vec![data];
        let indexes = Vec::new();
        let mut placed_rows = Vec::<PlacedRow>::new();
        let mut next_number = self.rows.first().map_or(1, |row| row.number);
        let mut prior_source = None;
        let mut rendered_cells = 0usize;
        let mut rendered_bytes = 0usize;
        self.render_range(
            0,
            self.rows.len(),
            &self.blocks,
            &scopes,
            &indexes,
            &mut context,
            &mut placed_rows,
            &mut next_number,
            &mut prior_source,
            &mut rendered_cells,
            &mut rendered_bytes,
            limits,
            0,
        )?;

        let row_map = placed_rows
            .iter()
            .map(|row| (row.source, row.target))
            .collect();
        let cell_map = placed_rows
            .iter()
            .flat_map(|row| row.cells.iter().copied())
            .collect();
        let rows_xml = placed_rows
            .into_iter()
            .map(|row| row.xml)
            .collect::<String>();
        let mut xml = self.xml.clone();
        let start = self.sheet_data.open_end;
        let end = self.sheet_data.close_start;
        let original_inner = &xml[start..end];
        let new_inner = replace_rows(original_inner, &rows_xml)?;
        xml.replace_range(start..end, &new_inner);
        Ok(RenderedSheet {
            xml,
            row_map,
            cell_map,
            effects: std::mem::take(&mut context.effects),
        })
    }

    pub(super) fn inspect(&self) -> Vec<InspectedTag> {
        self.rows
            .iter()
            .flat_map(|row| {
                row.cells
                    .iter()
                    .enumerate()
                    .flat_map(move |(cell_index, cell)| {
                        let reference = cell_reference(
                            &row.xml,
                            row.cell_spans[cell_index].0,
                            row.cell_spans[cell_index].1,
                        );
                        let reference = reference.unwrap_or_default();
                        cell.tags.iter().map(move |tag| InspectedTag {
                            cell: reference.clone(),
                            name: tag.name.clone(),
                            written: tag.written.clone(),
                            kind: tag.kind,
                        })
                    })
            })
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_range(
        &self,
        start: usize,
        end: usize,
        blocks: &[LoopBlock],
        scopes: &[&Value],
        indexes: &[usize],
        context: &mut Context<'_>,
        output: &mut Vec<PlacedRow>,
        next_number: &mut u32,
        prior_source: &mut Option<u32>,
        rendered_cells: &mut usize,
        rendered_bytes: &mut usize,
        limits: &super::WorkbookLimits,
        depth: usize,
    ) -> Result<(), FillError> {
        if depth > limits.max_loop_depth {
            return Err(limit_error(format!(
                "Worksheet loop nesting exceeds {}",
                limits.max_loop_depth
            )));
        }
        let mut row = start;
        while row < end {
            if let Some(block) = blocks
                .iter()
                .find(|block| block.start == row && block.end < end)
            {
                let first_source = self.rows[block.start].number;
                if let Some(prior) = *prior_source
                    && first_source > prior
                {
                    *next_number = next_number
                        .checked_add(first_source - prior - 1)
                        .ok_or_else(|| limit_error("Rendered worksheet row number overflow"))?;
                    // The gap is placed; the block's first row must not add it again.
                    *prior_source = Some(first_source - 1);
                }
                let value = context.value(self.tag(block.open), scopes, indexes);
                match value.as_ref() {
                    Some(Value::Array(items)) if !block.inverted => {
                        for (index, item) in items.iter().enumerate() {
                            let mut child_scopes = scopes.to_vec();
                            child_scopes.push(item);
                            let mut child_indexes = indexes.to_vec();
                            child_indexes.push(index);
                            self.render_range(
                                block.start,
                                block.end + 1,
                                &block.children,
                                &child_scopes,
                                &child_indexes,
                                context,
                                output,
                                next_number,
                                prior_source,
                                rendered_cells,
                                rendered_bytes,
                                limits,
                                depth + 1,
                            )?;
                        }
                    }
                    Some(Value::Array(items)) if block.inverted && items.is_empty() => {
                        self.render_range(
                            block.start,
                            block.end + 1,
                            &block.children,
                            scopes,
                            indexes,
                            context,
                            output,
                            next_number,
                            prior_source,
                            rendered_cells,
                            rendered_bytes,
                            limits,
                            depth + 1,
                        )?;
                    }
                    Some(value) if !block.inverted && text::shown(Some(value)) => {
                        let item = value.clone();
                        let mut child_scopes = scopes.to_vec();
                        child_scopes.push(&item);
                        self.render_range(
                            block.start,
                            block.end + 1,
                            &block.children,
                            &child_scopes,
                            indexes,
                            context,
                            output,
                            next_number,
                            prior_source,
                            rendered_cells,
                            rendered_bytes,
                            limits,
                            depth + 1,
                        )?;
                    }
                    None if block.inverted => {
                        self.render_range(
                            block.start,
                            block.end + 1,
                            &block.children,
                            scopes,
                            indexes,
                            context,
                            output,
                            next_number,
                            prior_source,
                            rendered_cells,
                            rendered_bytes,
                            limits,
                            depth + 1,
                        )?;
                    }
                    Some(value) if block.inverted && !text::shown(Some(value)) => {
                        self.render_range(
                            block.start,
                            block.end + 1,
                            &block.children,
                            scopes,
                            indexes,
                            context,
                            output,
                            next_number,
                            prior_source,
                            rendered_cells,
                            rendered_bytes,
                            limits,
                            depth + 1,
                        )?;
                    }
                    _ => {}
                }
                *prior_source = Some(self.rows[block.end].number);
                row = block.end + 1;
            } else {
                let source = self.rows[row].number;
                if let Some(prior) = *prior_source
                    && source > prior
                {
                    *next_number = next_number
                        .checked_add(source - prior - 1)
                        .ok_or_else(|| limit_error("Rendered worksheet row number overflow"))?;
                }
                if *next_number > MAX_ROWS || output.len() >= limits.max_output_rows {
                    return Err(limit_error("Rendered worksheet exceeds the configured row limit"));
                }
                let target = *next_number;
                let (mut xml, row_cells, cell_bytes) = self.render_row(
                    row,
                    target,
                    context,
                    scopes,
                    indexes,
                    rendered_cells,
                    rendered_bytes,
                    limits,
                )?;
                xml = set_attribute_open(&xml, "r", &target.to_string());
                *rendered_bytes = rendered_bytes
                    .checked_add(xml.len().saturating_sub(cell_bytes))
                    .ok_or_else(|| limit_error("Rendered worksheet byte count overflow"))?;
                if *rendered_bytes > limits.max_output_bytes {
                    return Err(limit_error(
                        "Rendered worksheet exceeds the configured byte limit",
                    ));
                }
                if output.len() >= MAX_ROWS as usize {
                    return Err(limit_error("Rendered worksheet exceeds the Excel row limit"));
                }
                let cells = row_cells
                    .into_iter()
                    .map(|(source_column, target_column)| CellCoordinate {
                        source_row: source,
                        source_column,
                        target_row: target,
                        target_column,
                    })
                    .collect();
                output.push(PlacedRow {
                    source,
                    target,
                    xml,
                    cells,
                });
                *next_number = next_number
                    .checked_add(1)
                    .ok_or_else(|| limit_error("Rendered worksheet row number overflow"))?;
                *prior_source = Some(source);
                row += 1;
            }
        }
        Ok(())
    }

    fn tag(&self, location: TagLocation) -> &Tag {
        &self.rows[location.row].cells[location.cell].tags[location.tag]
    }

    #[allow(clippy::too_many_arguments)]
    fn render_row(
        &self,
        row_index: usize,
        target_row: u32,
        context: &mut Context<'_>,
        scopes: &[&Value],
        indexes: &[usize],
        rendered_cells: &mut usize,
        rendered_bytes: &mut usize,
        limits: &super::WorkbookLimits,
    ) -> Result<RenderedRow, FillError> {
        let row = &self.rows[row_index];
        let mut out_cells = String::new();
        let mut coordinates = Vec::new();
        let mut cell_bytes = 0usize;
        let blocks = self
            .horizontal
            .get(&row_index)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let mut cell_index = 0;
        let mut shift = 0i64;
        while cell_index < row.cells.len() {
            if let Some(block) = blocks.iter().find(|block| block.start_cell == cell_index) {
                let tag = &row.cells[cell_index].tags[block.open_tag];
                let value = context.value(tag, scopes, indexes);
                let items: Vec<Value> = match value {
                    Some(Value::Array(items)) if !block.inverted => items.clone(),
                    Some(value) if !block.inverted && text::shown(Some(&value)) => vec![value],
                    None if block.inverted => vec![Value::Null],
                    Some(value) if block.inverted && !text::shown(Some(&value)) => {
                        vec![Value::Null]
                    }
                    _ => Vec::new(),
                };
                let width = block.end_column - block.start_column + 1;
                for (item_index, item) in items.iter().enumerate() {
                    let mut item_scopes = scopes.to_vec();
                    if !block.inverted {
                        item_scopes.push(item);
                    }
                    let mut item_indexes = indexes.to_vec();
                    item_indexes.push(item_index);
                    for source_index in block.start_cell..=block.end_cell {
                        let source_column = row.columns[source_index];
                        let offset = source_column - block.start_column;
                        let column = u32::try_from(
                            i64::from(block.start_column + offset)
                                + shift
                                + i64::from(item_index as u32) * i64::from(width),
                        )
                        .map_err(|_| error("Rendered worksheet column is invalid"))?;
                        let mut removed = self
                            .removed_tags
                            .get(&(row_index, source_index))
                            .cloned()
                            .unwrap_or_default();
                        let rendered = self.render_one_cell(
                            row_index,
                            source_index,
                            target_row,
                            column,
                            context,
                            &item_scopes,
                            &item_indexes,
                            &mut removed,
                        )?;
                        record_output_cell(&rendered, rendered_cells, rendered_bytes, limits)?;
                        cell_bytes += rendered.len();
                        out_cells.push_str(&rendered);
                        coordinates.push((source_column, column));
                    }
                }
                shift += i64::from(width) * (items.len() as i64 - 1);
                cell_index = block.end_cell + 1;
                continue;
            }
            let source_column = row.columns[cell_index];
            let column = u32::try_from(i64::from(source_column) + shift)
                .map_err(|_| error("Rendered worksheet column is invalid"))?;
            let mut removed = self
                .removed_tags
                .get(&(row_index, cell_index))
                .cloned()
                .unwrap_or_default();
            let rendered = self.render_one_cell(
                row_index,
                cell_index,
                target_row,
                column,
                context,
                scopes,
                indexes,
                &mut removed,
            )?;
            record_output_cell(&rendered, rendered_cells, rendered_bytes, limits)?;
            cell_bytes += rendered.len();
            out_cells.push_str(&rendered);
            coordinates.push((source_column, column));
            cell_index += 1;
        }
        let open_end = row
            .xml
            .find('>')
            .ok_or_else(|| error("Malformed worksheet row"))?
            + 1;
        let close_start = row.xml.rfind("</").unwrap_or(row.xml.len());
        let xml = format!(
            "{}{}{}",
            &row.xml[..open_end],
            out_cells,
            &row.xml[close_start..]
        );
        Ok((xml, coordinates, cell_bytes))
    }

    #[allow(clippy::too_many_arguments)]
    fn render_one_cell(
        &self,
        row_index: usize,
        cell_index: usize,
        target_row: u32,
        target_column: u32,
        context: &mut Context<'_>,
        scopes: &[&Value],
        indexes: &[usize],
        removed: &mut [usize],
    ) -> Result<String, FillError> {
        let row = &self.rows[row_index];
        let reference = format!("{}{}", column_name(target_column), target_row);
        if target_column > MAX_COLUMNS {
            return Err(limit_error("Rendered worksheet exceeds the Excel column limit"));
        }
        context.cell = reference;
        let rendered = row.cells[cell_index]
            .render(context, scopes, indexes, removed)
            .map_err(|failure| {
                annotate_cell_error(
                    failure,
                    &format!("{}{}", column_name(target_column), target_row),
                )
            })?;
        let target = set_cell_reference(
            &rendered,
            &format!("{}{}", column_name(target_column), target_row),
        )?;
        let source_column = row.columns[cell_index];
        super::references::shift_formula_in_cell(
            &target,
            i64::from(target_column) - i64::from(source_column),
        )
    }
}

fn pair_row_loops(
    tags: &[&Tag],
    locations: &[TagLocation],
) -> Result<(Vec<LoopBlock>, Vec<TagLocation>), FillError> {
    let mut stack: Vec<(usize, TagLocation, bool, String)> = Vec::new();
    let mut paired = Vec::new();
    let mut row_tags = Vec::new();
    for (index, tag) in tags.iter().enumerate() {
        match &tag.kind {
            Kind::Open {
                inverted,
                horizontal: false,
            } => {
                if stack.len() >= MAX_LOOP_DEPTH {
                    return Err(limit_error("Worksheet loop nesting exceeds 64"));
                }
                stack.push((index, locations[index], *inverted, tag.name.clone()));
            }
            Kind::Close { horizontal: false } => {
                let Some((_, open, inverted, name)) = stack.pop() else {
                    return Err(error("Worksheet has an unopened loop close tag"));
                };
                let close = locations[index];
                if !tag.name.is_empty() && tag.name != name {
                    return Err(error(format!(
                        "Worksheet loop {name:?} is closed by {:?}",
                        tag.name
                    )));
                }
                if open.row == close.row && open.cell == close.cell {
                    continue;
                }
                row_tags.push(open);
                row_tags.push(close);
                paired.push(LoopBlock {
                    start: open.row,
                    end: close.row,
                    open,
                    inverted,
                    children: Vec::new(),
                });
            }
            Kind::Open {
                horizontal: true, ..
            }
            | Kind::Close { horizontal: true }
            | Kind::Value
            | Kind::Change
            | Kind::Image
            | Kind::Chart
            | Kind::RichText { .. } => {}
        }
    }
    if !stack.is_empty() {
        return Err(error("Worksheet has an unclosed row loop"));
    }
    paired.sort_by_key(|block| (block.start, std::cmp::Reverse(block.end)));
    let mut roots: Vec<LoopBlock> = Vec::new();
    let mut stack: Vec<LoopBlock> = Vec::new();
    for block in paired {
        while stack.last().is_some_and(|parent| block.start > parent.end) {
            let finished = stack.pop().expect("checked above");
            if let Some(parent) = stack.last_mut() {
                parent.children.push(finished);
            } else {
                roots.push(finished);
            }
        }
        if stack.last().is_some_and(|parent| block.end > parent.end) {
            return Err(error("Worksheet row loops overlap"));
        }
        if stack.len() >= MAX_LOOP_DEPTH {
            return Err(limit_error("Worksheet loop nesting exceeds 64"));
        }
        stack.push(block);
    }
    while let Some(finished) = stack.pop() {
        if let Some(parent) = stack.last_mut() {
            parent.children.push(finished);
        } else {
            roots.push(finished);
        }
    }
    roots.sort_by_key(|block| block.start);
    Ok((roots, row_tags))
}

fn pair_horizontal_loops(
    tags: &[&Tag],
    locations: &[TagLocation],
    rows: &[Row],
) -> Result<BTreeMap<usize, Vec<HorizontalBlock>>, FillError> {
    let mut result = BTreeMap::<usize, Vec<HorizontalBlock>>::new();
    let mut consumed = std::collections::BTreeSet::new();
    for (index, tag) in tags.iter().enumerate() {
        if consumed.contains(&index) {
            continue;
        }
        let location = locations[index];
        let Kind::Open {
            horizontal: true,
            inverted,
        } = tag.kind
        else {
            if matches!(tag.kind, Kind::Close { horizontal: true }) {
                return Err(error("Worksheet has an unopened horizontal loop close tag"));
            }
            continue;
        };
        let close = (index + 1..tags.len()).find(|candidate| {
            locations[*candidate].row == location.row
                && matches!(tags[*candidate].kind, Kind::Close { horizontal: true })
                && (tags[*candidate].name.is_empty() || tags[*candidate].name == tag.name)
        });
        let (end, close_tag) = close.map_or((location.cell, None), |candidate| {
            (locations[candidate].cell, Some(locations[candidate].tag))
        });
        if let Some(close_index) = close {
            if tags[index + 1..close_index].iter().any(|inner| {
                matches!(
                    inner.kind,
                    Kind::Open {
                        horizontal: true,
                        ..
                    }
                )
            }) {
                return Err(error(
                    "Nested horizontal loops in one row are not supported",
                ));
            }
            consumed.insert(close_index);
        }
        let row = &rows[location.row];
        let (start_column, _) = validate_cell_reference(
            &row.xml[row.cell_spans[location.cell].0..row.cell_spans[location.cell].1],
        )?;
        let (end_column, _) =
            validate_cell_reference(&row.xml[row.cell_spans[end].0..row.cell_spans[end].1])?;
        if end_column < start_column {
            return Err(error("Horizontal loop close must follow its open marker"));
        }
        result
            .entry(location.row)
            .or_default()
            .push(HorizontalBlock {
                start_cell: location.cell,
                end_cell: end,
                open_tag: location.tag,
                close_tag,
                start_column,
                end_column,
                inverted,
            });
    }
    for blocks in result.values_mut() {
        blocks.sort_by_key(|block| block.start_cell);
    }
    Ok(result)
}

fn direct_children(xml: &str) -> Result<Vec<Element>, FillError> {
    let tokens = tokenize(xml).map_err(error)?;
    let mut at = 0usize;
    let mut depth = 0usize;
    let mut open: Option<(String, usize, usize)> = None;
    let mut children = Vec::new();
    for token in tokens {
        let (length, element) = match token {
            Token::Text(text) => (text.len(), None),
            Token::Element { kind, name, raw } => (raw.len(), Some((kind, name, raw))),
        };
        if let Some((kind, name, raw)) = element {
            match kind {
                ElementKind::SelfClosing if depth == 0 => children.push(Element {
                    name: name.to_string(),
                    start: at,
                    open_end: at + length,
                    close_start: at + length,
                    end: at + length,
                }),
                ElementKind::Open => {
                    if depth == 0 {
                        open = Some((name.to_string(), at, at + length));
                    }
                    depth += 1;
                }
                ElementKind::Close => {
                    depth = depth.saturating_sub(1);
                    if depth == 0
                        && let Some((name, start, open_end)) = open.take()
                    {
                        children.push(Element {
                            name,
                            start,
                            open_end,
                            close_start: at,
                            end: at + length,
                        });
                    }
                }
                ElementKind::Other => {}
                ElementKind::SelfClosing => {}
            }
            let _ = raw;
        }
        at += length;
    }
    Ok(children)
}

fn validate_cell_reference(cell_xml: &str) -> Result<(u32, u32), FillError> {
    let cell = xmlutil::elements(cell_xml)?
        .into_iter()
        .find(|element| xmlutil::local_name(&element.name) == "c")
        .ok_or_else(|| error("Worksheet row contains malformed cell XML"))?;
    let reference = read_attribute(&cell_xml[cell.start..cell.open_end], "r")
        .ok_or_else(|| error("Worksheet cell has no reference"))?;
    let (column, row) = split_reference(reference)
        .ok_or_else(|| error(format!("Invalid worksheet cell reference {reference:?}")))?;
    if column > MAX_COLUMNS || row == 0 || row > MAX_ROWS {
        return Err(error("Worksheet cell reference is outside Excel limits"));
    }
    Ok((column, row))
}

fn split_reference(reference: &str) -> Option<(u32, u32)> {
    let mut column = 0u32;
    let mut at = 0usize;
    for byte in reference.bytes() {
        if !byte.is_ascii_alphabetic() {
            break;
        }
        column = column
            .checked_mul(26)?
            .checked_add((byte.to_ascii_uppercase() - b'A' + 1) as u32)?;
        at += 1;
    }
    let row = reference.get(at..)?.parse::<u32>().ok()?;
    (column > 0).then_some((column, row))
}

fn cell_reference(xml: &str, start: usize, end: usize) -> Option<String> {
    let cell = xml.get(start..end)?;
    let reference = read_attribute(cell, "r")?;
    Some(reference.to_string())
}

fn set_cell_reference(xml: &str, reference: &str) -> Result<String, FillError> {
    let open = opening_tag(xml).ok_or_else(|| error("Rendered cell XML is malformed"))?;
    let current =
        read_attribute(open, "r").ok_or_else(|| error("Rendered cell has no reference"))?;
    let at = current.as_ptr() as usize - open.as_ptr() as usize;
    let mut output = xml.to_string();
    output.replace_range(at..at + current.len(), reference);
    Ok(output)
}

fn record_output_cell(
    xml: &str,
    cells: &mut usize,
    bytes: &mut usize,
    limits: &super::WorkbookLimits,
) -> Result<(), FillError> {
    if xml.is_empty() {
        return Ok(());
    }
    *cells = cells
        .checked_add(1)
        .ok_or_else(|| limit_error("Rendered worksheet cell count overflow"))?;
    if *cells > limits.max_output_cells {
        return Err(limit_error(
            "Rendered worksheet exceeds the configured cell limit",
        ));
    }
    *bytes = bytes
        .checked_add(xml.len())
        .ok_or_else(|| limit_error("Rendered worksheet byte count overflow"))?;
    if *bytes > limits.max_output_bytes {
        return Err(limit_error(
            "Rendered worksheet exceeds the configured byte limit",
        ));
    }
    Ok(())
}

fn annotate_cell_error(mut failure: FillError, cell: &str) -> FillError {
    if failure.errors.is_empty() {
        failure.message = format!("Cell {cell}: {}", failure.message);
    } else {
        for nested in failure.errors.iter_mut() {
            *nested = annotate_cell_error(
                std::mem::replace(nested, error("Spreadsheet cell error")),
                cell,
            );
        }
    }
    failure
}

fn limit_error(message: impl Into<String>) -> FillError {
    FillError::new(message).with_id(FillErrorId::SpreadsheetLimit)
}

fn opening_tag(xml: &str) -> Option<&str> {
    if !xml.starts_with('<') {
        return None;
    }
    let mut quote = None;
    for (at, byte) in xml.bytes().enumerate() {
        if let Some(delimiter) = quote {
            if byte == delimiter {
                quote = None;
            }
        } else if byte == b'\'' || byte == b'"' {
            quote = Some(byte);
        } else if byte == b'>' {
            return Some(&xml[..=at]);
        }
    }
    None
}

fn set_attribute_open(xml: &str, name: &str, value: &str) -> String {
    let Some(raw) = opening_tag(xml) else {
        return xml.to_string();
    };
    format!(
        "{}{}",
        crate::properties::set_attribute(raw, name, value),
        &xml[raw.len()..]
    )
}

fn column_name(mut number: u32) -> String {
    let mut name = String::new();
    while number > 0 {
        let remainder = (number - 1) % 26;
        name.insert(0, (b'A' + remainder as u8) as char);
        number = (number - 1) / 26;
    }
    name
}

fn replace_rows(inner: &str, rows: &str) -> Result<String, FillError> {
    let children = direct_children(inner)?;
    let rows_found = children
        .into_iter()
        .filter(|element| xmlutil::local_name(&element.name) == "row")
        .collect::<Vec<_>>();
    let Some(first) = rows_found.first() else {
        return Ok(format!("{inner}{rows}"));
    };
    let last = rows_found.last().expect("first exists");
    Ok(format!(
        "{}{}{}",
        &inner[..first.start],
        rows,
        &inner[last.end..]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FillOptions, NoHooks};

    #[test]
    fn horizontal_loop_repeats_cells_and_shifts_following_cells() {
        let xml = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>{&gt;items}</t></is></c><c r="B1" t="inlineStr"><is><t>{name}</t></is></c><c r="C1" t="inlineStr"><is><t>tail</t></is></c></row></sheetData></worksheet>"#;
        let sheet = Sheet::parse(xml, &[], &FillOptions::default()).unwrap();
        let data = serde_json::json!({"items":[{"name":"one"},{"name":"two"}]});
        let mut hooks = NoHooks;
        let rendered = sheet
            .render(&data, &mut hooks, "xl/worksheets/sheet1.xml", false)
            .unwrap();
        assert!(rendered.xml.contains("r=\"A1\""));
        assert!(rendered.xml.contains("r=\"B1\""));
        assert!(rendered.xml.contains("r=\"C1\""));
        assert!(rendered.xml.contains("r=\"D1\""));
        assert_eq!(rendered.cell_map.len(), 4);
    }
}
