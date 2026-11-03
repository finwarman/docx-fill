//! Worksheet parsing and row-loop rendering.

use serde_json::Value;

use crate::xml::{ElementKind, Token, read_attribute, tokenize};
use crate::{FillError, FillOptions};
use crate::render::FillHooks;

use super::{cells::Cell, error, text::{self, Context, Kind, Tag}, xmlutil::{self, Element}};

const MAX_ROWS: u32 = 1_048_576;
const MAX_COLUMNS: u32 = 16_384;
const MAX_CELLS: usize = 1_000_000;
const MAX_LOOP_DEPTH: usize = 64;

#[derive(Debug)]
struct Row {
    xml: String,
    number: u32,
    cells: Vec<Cell>,
    cell_spans: Vec<(usize, usize)>,
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

struct PlacedRow {
    source: u32,
    target: u32,
    xml: String,
}

/// A parsed worksheet with cell text tags and row loops.
#[derive(Debug)]
pub(super) struct Sheet {
    xml: String,
    sheet_data: Element,
    rows: Vec<Row>,
    blocks: Vec<LoopBlock>,
    row_loop_tags: Vec<TagLocation>,
}

pub(super) struct RenderedSheet {
    pub xml: String,
    pub row_map: Vec<(u32, u32)>,
}

impl Sheet {
    pub(super) fn parse(xml: &str, shared: &[String], options: &FillOptions) -> Result<Self, FillError> {
        let sheet_data = xmlutil::elements(xml)?.into_iter().find(|element| xmlutil::local_name(&element.name) == "sheetData")
            .ok_or_else(|| error("Worksheet has no sheetData element"))?;
        let row_elements = direct_children(&xml[sheet_data.open_end..sheet_data.close_start])?
            .into_iter().filter(|element| xmlutil::local_name(&element.name) == "row").collect::<Vec<_>>();
        if row_elements.len() > MAX_ROWS as usize {
            return Err(error("Worksheet row count exceeds the Excel limit"));
        }

        let mut rows = Vec::with_capacity(row_elements.len());
        let mut seen_rows = std::collections::BTreeSet::new();
        let mut cell_count = 0usize;
        let mut previous_row = 0;
        for element in row_elements {
            let sheet_data_start = sheet_data.open_end;
            let row_xml = xml[sheet_data_start + element.start..sheet_data_start + element.end].to_string();
            let row_open = &row_xml[..element.open_end - element.start];
            let number = read_attribute(row_open, "r").and_then(|raw| raw.parse::<u32>().ok())
                .filter(|number| (1..=MAX_ROWS).contains(number))
                .ok_or_else(|| error("Worksheet row has an invalid row number"))?;
            if !seen_rows.insert(number) {
                return Err(error("Worksheet has duplicate row numbers"));
            }
            if number <= previous_row { return Err(error("Worksheet rows are not in ascending order")); }
            previous_row = number;
            let row_inner = if element.close_start == element.open_end {
                ""
            } else {
                &row_xml[element.open_end - element.start..element.close_start - element.start]
            };
            let cell_elements = direct_children(row_inner)?.into_iter().filter(|cell| xmlutil::local_name(&cell.name) == "c").collect::<Vec<_>>();
            cell_count = cell_count.checked_add(cell_elements.len()).ok_or_else(|| error("Worksheet cell count overflow"))?;
            if cell_count > MAX_CELLS { return Err(error("Worksheet exceeds one million cells")); }
            let mut cells = Vec::with_capacity(cell_elements.len());
            let mut spans = Vec::with_capacity(cell_elements.len());
            let mut previous_column = 0;
            for cell in cell_elements {
                let start = element.open_end - element.start + cell.start;
                let end = element.open_end - element.start + cell.end;
                let cell_xml = &row_xml[start..end];
                let (column, referenced_row) = validate_cell_reference(cell_xml)?;
                if referenced_row != number { return Err(error("Worksheet cell reference does not match its row")); }
                if column <= previous_column { return Err(error("Worksheet cells are duplicate or out of column order")); }
                previous_column = column;
                cells.push(Cell::parse(cell_xml, shared, options)?);
                spans.push((start, end));
            }
            rows.push(Row { xml: row_xml, number, cells, cell_spans: spans });
        }

        // Row loops cross a cell boundary or a row boundary. Paired tags inside one cell stay
        // with the cell renderer, which handles inline loops.
        let mut locations = Vec::new();
        let mut tags = Vec::new();
        for (row_index, row) in rows.iter().enumerate() {
            for (cell_index, cell) in row.cells.iter().enumerate() {
                for (tag_index, tag) in cell.tags.iter().enumerate() {
                    locations.push(TagLocation { row: row_index, cell: cell_index, tag: tag_index });
                    tags.push(tag);
                }
            }
        }
        let (blocks, row_loop_tags) = pair_row_loops(&tags, &locations)?;
        Ok(Self { xml: xml.to_string(), sheet_data, rows, blocks, row_loop_tags })
    }

    pub(super) fn render(&self, data: &Value, hooks: &mut dyn FillHooks, part: &str) -> Result<RenderedSheet, FillError> {
        let mut context = Context::new(part, hooks);
        let scopes = vec![data];
        let indexes = Vec::new();
        let mut placed_rows = Vec::<PlacedRow>::new();
        let mut next_number = self.rows.first().map_or(1, |row| row.number);
        let mut prior_source = None;
        let mut rendered_cells = 0usize;
        self.render_range(0, self.rows.len(), &self.blocks, &scopes, &indexes, &mut context, &mut placed_rows, &mut next_number, &mut prior_source, &mut rendered_cells, 0)?;

        let row_map = placed_rows.iter().map(|row| (row.source, row.target)).collect();
        let rows_xml = placed_rows.into_iter().map(|row| row.xml).collect::<String>();
        let mut xml = self.xml.clone();
        let start = self.sheet_data.open_end;
        let end = self.sheet_data.close_start;
        let original_inner = &xml[start..end];
        let new_inner = replace_rows(original_inner, &rows_xml)?;
        xml.replace_range(start..end, &new_inner);
        Ok(RenderedSheet { xml, row_map })
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
        depth: usize,
    ) -> Result<(), FillError> {
        if depth > MAX_LOOP_DEPTH { return Err(error("Worksheet loop nesting exceeds 64")); }
        let mut row = start;
        while row < end {
            if let Some(block) = blocks.iter().find(|block| block.start == row && block.end < end) {
                let first_source = self.rows[block.start].number;
                if let Some(prior) = *prior_source
                    && first_source > prior
                {
                    *next_number = next_number.checked_add(first_source - prior - 1).ok_or_else(|| error("Rendered worksheet row number overflow"))?;
                }
                let value = context.value(self.tag(block.open), scopes, indexes);
                match value.as_ref() {
                    Some(Value::Array(items)) if !block.inverted => {
                        for (index, item) in items.iter().enumerate() {
                            let mut child_scopes = scopes.to_vec();
                            child_scopes.push(item);
                            let mut child_indexes = indexes.to_vec();
                            child_indexes.push(index);
                            self.render_range(block.start, block.end + 1, &block.children, &child_scopes, &child_indexes, context, output, next_number, prior_source, rendered_cells, depth + 1)?;
                        }
                    }
                    Some(Value::Array(items)) if block.inverted && items.is_empty() => {
                        self.render_range(block.start, block.end + 1, &block.children, scopes, indexes, context, output, next_number, prior_source, rendered_cells, depth + 1)?;
                    }
                    Some(value) if !block.inverted && text::shown(Some(value)) => {
                        let item = value.clone();
                        let mut child_scopes = scopes.to_vec();
                        child_scopes.push(&item);
                        self.render_range(block.start, block.end + 1, &block.children, &child_scopes, indexes, context, output, next_number, prior_source, rendered_cells, depth + 1)?;
                    }
                    None if block.inverted => {
                        self.render_range(block.start, block.end + 1, &block.children, scopes, indexes, context, output, next_number, prior_source, rendered_cells, depth + 1)?;
                    }
                    Some(value) if block.inverted && !text::shown(Some(value)) => {
                        self.render_range(block.start, block.end + 1, &block.children, scopes, indexes, context, output, next_number, prior_source, rendered_cells, depth + 1)?;
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
                    *next_number = next_number.checked_add(source - prior - 1).ok_or_else(|| error("Rendered worksheet row number overflow"))?;
                }
                if *next_number > MAX_ROWS { return Err(error("Rendered worksheet exceeds the Excel row limit")); }
                let target = *next_number;
                let mut xml = self.render_row(row, context, scopes, indexes)?;
                xml = set_attribute_open(&xml, "r", &target.to_string());
                renumber_cells(&mut xml, source, target)?;
                *rendered_cells = rendered_cells.checked_add(self.rows[row].cells.len()).ok_or_else(|| error("Rendered worksheet cell count overflow"))?;
                if *rendered_cells > MAX_CELLS { return Err(error("Rendered worksheet exceeds one million cells")); }
                if output.len() >= MAX_ROWS as usize { return Err(error("Rendered worksheet exceeds the Excel row limit")); }
                output.push(PlacedRow { source, target, xml });
                *next_number = next_number.checked_add(1).ok_or_else(|| error("Rendered worksheet row number overflow"))?;
                *prior_source = Some(source);
                row += 1;
            }
        }
        Ok(())
    }

    fn tag(&self, location: TagLocation) -> &Tag {
        &self.rows[location.row].cells[location.cell].tags[location.tag]
    }

    fn render_row(&self, row_index: usize, context: &mut Context<'_>, scopes: &[&Value], indexes: &[usize]) -> Result<String, FillError> {
        let row = &self.rows[row_index];
        let mut replacements = Vec::with_capacity(row.cells.len());
        for (cell_index, cell) in row.cells.iter().enumerate() {
            let removed_tags = self.row_loop_tags.iter().filter(|location| location.row == row_index && location.cell == cell_index).map(|location| location.tag).collect::<Vec<_>>();
            let rendered = cell.render(context, scopes, indexes, &removed_tags)?;
            replacements.push((row.cell_spans[cell_index].0, row.cell_spans[cell_index].1, rendered));
        }
        let mut xml = row.xml.clone();
        for (start, end, replacement) in replacements.into_iter().rev() {
            xml.replace_range(start..end, &replacement);
        }
        Ok(xml)
    }
}

fn pair_row_loops(tags: &[&Tag], locations: &[TagLocation]) -> Result<(Vec<LoopBlock>, Vec<TagLocation>), FillError> {
    let mut stack: Vec<(usize, TagLocation, bool, String)> = Vec::new();
    let mut paired = Vec::new();
    let mut row_tags = Vec::new();
    for (index, tag) in tags.iter().enumerate() {
        match &tag.kind {
            Kind::Open { inverted } => {
                if stack.len() >= MAX_LOOP_DEPTH { return Err(error("Worksheet loop nesting exceeds 64")); }
                stack.push((index, locations[index], *inverted, tag.name.clone()));
            }
            Kind::Close => {
                let Some((_, open, inverted, name)) = stack.pop() else {
                    return Err(error("Worksheet has an unopened loop close tag"));
                };
                let close = locations[index];
                if !tag.name.is_empty() && tag.name != name {
                    return Err(error(format!("Worksheet loop {name:?} is closed by {:?}", tag.name)));
                }
                if open.row == close.row && open.cell == close.cell {
                    continue;
                }
                row_tags.push(open);
                row_tags.push(close);
                paired.push(LoopBlock { start: open.row, end: close.row, open, inverted, children: Vec::new() });
            }
            Kind::Value | Kind::Change => {}
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
            if let Some(parent) = stack.last_mut() { parent.children.push(finished); } else { roots.push(finished); }
        }
        if stack.last().is_some_and(|parent| block.end > parent.end) {
            return Err(error("Worksheet row loops overlap"));
        }
        if stack.len() >= MAX_LOOP_DEPTH { return Err(error("Worksheet loop nesting exceeds 64")); }
        stack.push(block);
    }
    while let Some(finished) = stack.pop() {
        if let Some(parent) = stack.last_mut() { parent.children.push(finished); } else { roots.push(finished); }
    }
    roots.sort_by_key(|block| block.start);
    Ok((roots, row_tags))
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
                ElementKind::SelfClosing if depth == 0 => children.push(Element { name: name.to_string(), start: at, open_end: at + length, close_start: at + length, end: at + length }),
                ElementKind::Open => {
                    if depth == 0 { open = Some((name.to_string(), at, at + length)); }
                    depth += 1;
                }
                ElementKind::Close => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 && let Some((name, start, open_end)) = open.take() {
                        children.push(Element { name, start, open_end, close_start: at, end: at + length });
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
    let cell = xmlutil::elements(cell_xml)?.into_iter().find(|element| xmlutil::local_name(&element.name) == "c")
        .ok_or_else(|| error("Worksheet row contains malformed cell XML"))?;
    let reference = read_attribute(&cell_xml[cell.start..cell.open_end], "r").ok_or_else(|| error("Worksheet cell has no reference"))?;
    let (column, row) = split_reference(reference).ok_or_else(|| error(format!("Invalid worksheet cell reference {reference:?}")))?;
    if column > MAX_COLUMNS || row == 0 || row > MAX_ROWS { return Err(error("Worksheet cell reference is outside Excel limits")); }
    Ok((column, row))
}

fn split_reference(reference: &str) -> Option<(u32, u32)> {
    let mut column = 0u32;
    let mut at = 0usize;
    for byte in reference.bytes() {
        if !byte.is_ascii_alphabetic() { break; }
        column = column.checked_mul(26)?.checked_add((byte.to_ascii_uppercase() - b'A' + 1) as u32)?;
        at += 1;
    }
    let row = reference.get(at..)?.parse::<u32>().ok()?;
    (column > 0).then_some((column, row))
}

fn set_attribute_open(xml: &str, name: &str, value: &str) -> String {
    let Ok(tokens) = tokenize(xml) else { return xml.to_string() };
    let Some(Token::Element { raw, .. }) = tokens.first() else { return xml.to_string() };
    format!("{}{}", crate::properties::set_attribute(raw, name, value), &xml[raw.len()..])
}

fn renumber_cells(xml: &mut String, source_row: u32, target_row: u32) -> Result<(), FillError> {
    if source_row == target_row { return Ok(()); }
    let elements = xmlutil::elements(xml)?;
    let cells = elements.into_iter().filter(|element| xmlutil::local_name(&element.name) == "c").collect::<Vec<_>>();
    for cell in cells.into_iter().rev() {
        let open = &xml[cell.start..cell.open_end];
        let Some(reference) = read_attribute(open, "r") else { continue };
        let Some((column, row)) = split_reference(reference) else { continue };
        if row != source_row { continue; }
        let letters = column_name(column);
        let old = format!("r=\"{reference}\"");
        if let Some(at) = open.find(&old) {
            let new = format!("r=\"{letters}{target_row}\"");
            xml.replace_range(cell.start + at..cell.start + at + old.len(), &new);
        }
    }
    Ok(())
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
    let rows_found = children.into_iter().filter(|element| xmlutil::local_name(&element.name) == "row").collect::<Vec<_>>();
    let Some(first) = rows_found.first() else { return Ok(format!("{inner}{rows}")) };
    let last = rows_found.last().expect("first exists");
    Ok(format!("{}{}{}", &inner[..first.start], rows, &inner[last.end..]))
}
