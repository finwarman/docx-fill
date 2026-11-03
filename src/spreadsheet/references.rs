//! Spreadsheet references whose row numbers change when a template row repeats.

use crate::xml::{ElementKind, Token, tokenize};
use crate::properties::set_attribute;
use crate::{FillError, FillErrorId};

type RowMap = std::collections::BTreeMap<u32, Vec<u32>>;

/// Update row-relative references in a formula copied from `source_row` to `target_row`.
/// Absolute row references follow Excel's copy semantics and stay fixed.
#[cfg(test)]
pub(super) fn update_formula(formula: &str, source_row: u32, target_row: u32, row_map: &[(u32, u32)]) -> Result<String, FillError> {
    update_formula_with_rows(formula, source_row, target_row, &row_mapping(row_map), &[])
}

#[cfg(test)]
fn row_mapping(row_map: &[(u32, u32)]) -> RowMap {
    let mut mapping = RowMap::new();
    for &(source, target) in row_map { mapping.entry(source).or_default().push(target); }
    for targets in mapping.values_mut() { targets.sort_unstable(); targets.dedup(); }
    mapping
}

/// True when a formula names another sheet. String literals and error values such as
/// `#REF!` contain `!` without referring to a sheet.
pub(super) fn has_sheet_reference(formula: &str) -> bool {
    let mut quoted = false;
    for (at, ch) in formula.char_indices() {
        if ch == '"' { quoted = !quoted; continue; }
        if quoted || ch != '!' { continue; }
        let word = formula[..at].trim_end_matches(|c: char| c.is_ascii_alphanumeric() || c == '/');
        if !word.ends_with('#') { return true; }
    }
    false
}

fn update_formula_with_rows(formula: &str, source_row: u32, target_row: u32, mapping: &RowMap, original_rows: &[u32]) -> Result<String, FillError> {
    let copy_delta = if let Some(targets) = mapping.get(&source_row) {
        i64::from(target_row) - i64::from(targets[0])
    } else {
        i64::from(target_row) - i64::from(source_row)
    };
    let map = |row, absolute, range_end| map_row(row, source_row, target_row, absolute, range_end, copy_delta, mapping, original_rows);
    let removed = |row: u32| !mapping.contains_key(&row) && original_rows.binary_search(&row).is_ok();
    // A range endpoint on a removed row moves toward the other endpoint, as Excel shrinks a
    // range when rows are deleted. None means every row of the range was removed.
    let shrink = |end: &RowRef, toward: u32, range_end: bool| {
        let mut row = end.row;
        loop {
            if !removed(row) { return map(row, end.absolute, range_end); }
            if row == toward { return None; }
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
            let boundary_before = before.is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_' && c != '.' && c != '[');
            let boundary_after = after.is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_' && c != '(' && c != ']');
            if boundary_before && boundary_after {
                let rows = match &last {
                    None => map(first.row, first.absolute, before == Some(':')).map(|row| vec![(&first, row)]),
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
                        return Err(reference_error("A copied formula refers outside Excel's row limits"));
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
    if quoted { return Err(reference_error("A formula has an unclosed string literal")); }
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
        let last = if colon(first.end) { read_cell_ref(text, first.end + 1) } else { None };
        return Some((first, last));
    }
    let first = read_row(text, start)?;
    if !colon(first.end) { return None; }
    let last = read_row(text, first.end + 1)?;
    Some((first, Some(last)))
}

fn read_cell_ref(text: &str, start: usize) -> Option<RowRef> {
    let bytes = text.as_bytes();
    let mut at = start;
    if bytes.get(at) == Some(&b'$') { at += 1; }
    let col_start = at;
    while bytes.get(at).is_some_and(u8::is_ascii_alphabetic) { at += 1; }
    if at == col_start { return None; }
    column_number(&text[col_start..at])?;
    read_row(text, at)
}

fn read_row(text: &str, start: usize) -> Option<RowRef> {
    let bytes = text.as_bytes();
    let mut at = start;
    let absolute = bytes.get(at) == Some(&b'$');
    if absolute { at += 1; }
    let row_start = at;
    while bytes.get(at).is_some_and(u8::is_ascii_digit) { at += 1; }
    if at == row_start { return None; }
    let row = text[row_start..at].parse::<u32>().ok()?;
    if row == 0 || row > 1_048_576 { return None; }
    Some(RowRef { end: at, row_start, row, absolute })
}

fn column_number(text: &str) -> Option<u32> {
    let mut number = 0u32;
    for byte in text.bytes() {
        number = number.checked_mul(26)?.checked_add(u32::from(byte.to_ascii_uppercase() - b'A' + 1))?;
        if number > 16_384 { return None; }
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
pub(super) fn update_sheet_with_rows(xml: &str, row_map: &[(u32, u32)], original_rows: &[u32]) -> Result<String, FillError> {
    let tokens = tokenize(xml).map_err(reference_error)?;
    let mut emitted_to_source = std::collections::BTreeMap::new();
    let mut source_to_emitted = RowMap::new();
    for &(source, emitted) in row_map {
        if emitted == 0 || emitted > 1_048_576 || source == 0 || source > 1_048_576 {
            return Err(reference_error("A worksheet row reference is outside Excel's limits"));
        }
        emitted_to_source.insert(emitted, source);
        source_to_emitted.entry(source).or_default().push(emitted);
    }
    for rows in source_to_emitted.values_mut() { rows.sort_unstable(); rows.dedup(); }
    let structural = row_map.iter().any(|(source, target)| source != target)
        || source_to_emitted.values().any(|rows| rows.len() > 1)
        || original_rows.iter().any(|source| !source_to_emitted.contains_key(source));
    let mut replacements: Vec<(usize, usize, String)> = Vec::new();
    let mut offset = 0usize;
    let mut cell_row = None;
    let mut formula_start: Option<(usize, u32)> = None;
    let mut removing: Option<(usize, &str)> = None;
    for token in tokens {
        let length = match token { Token::Element { raw, .. } => raw.len(), Token::Text(text) => text.len() };
        if let Some((start, removed)) = removing {
            if let Token::Element { kind: ElementKind::Close, name, .. } = token && name == removed {
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
                        Geometry::Replace(changed) => replacements.push((offset, offset + length, changed)),
                        Geometry::Remove if kind == ElementKind::Open => {
                            removing = Some((offset, name));
                            offset += length;
                            continue;
                        }
                        Geometry::Remove => replacements.push((offset, offset + length, String::new())),
                    }
                }
                if kind == ElementKind::Open {
                    if local(name) == "c" {
                        cell_row = attr(raw, "r").and_then(cell_ref_row);
                    }
                    if local(name) == "f" {
                        if let Some(target) = cell_row {
                            let source = emitted_to_source.get(&target).copied().unwrap_or(target);
                            let formula_type = attr(raw, "t");
                            if formula_type.is_some_and(|kind| kind == "shared" || kind == "array") && structural {
                                return Err(reference_error("Shared and array formulas cannot be copied safely yet"));
                            }
                            formula_start = Some((offset + raw.len(), source));
                        }
                    }
                } else if kind == ElementKind::Close {
                    if local(name) == "f" {
                        if let Some((start, source)) = formula_start.take() {
                            let target = cell_row.unwrap_or(source);
                            let end = offset;
                            let formula = &xml[start..end];
                            let formula = crate::xml::unescape(formula);
                            let replaced = update_formula_with_rows(&formula, source, target, &source_to_emitted, original_rows)?;
                            let replaced = escape_formula(&replaced);
                            if replaced != xml[start..end] {
                                replacements.push((start, end, replaced));
                            }
                        }
                    }
                    if local(name) == "c" { cell_row = None; }
                } else if kind == ElementKind::SelfClosing && local(name) == "f" {
                    if let Some(target) = cell_row {
                        let source = emitted_to_source.get(&target).copied().unwrap_or(target);
                        if source != target || structural {
                            return Err(reference_error("Shared and array formulas cannot be copied safely yet"));
                        }
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
        if start < at { return Err(reference_error("Overlapping spreadsheet XML edits")); }
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
    if !absolute && row == formula_source { return Some(formula_target); }
    let structural = if let Some(targets) = mapping.get(&row) {
        if range_end { targets.last().copied() } else { targets.first().copied() }
    } else if original_rows.binary_search(&row).is_ok() {
        None
    } else if let Some((source, targets)) = mapping.range(..row).next_back() {
        targets.last().copied()?.checked_add(row - source)
    } else if let Some((source, targets)) = mapping.first_key_value() {
        targets.first().and_then(|target| row.checked_add(target.checked_sub(*source)?)).or(Some(row))
    } else {
        Some(row)
    }?;
    if absolute { Some(structural) }
    else { i64::from(structural).checked_add(copy_delta).and_then(|mapped| u32::try_from(mapped).ok()) }
}

fn escape_formula(formula: &str) -> String {
    let mut out = String::with_capacity(formula.len());
    for ch in formula.chars() {
        match ch { '&' => out.push_str("&amp;"), '<' => out.push_str("&lt;"), '>' => out.push_str("&gt;"), _ => out.push(ch) }
    }
    out
}

/// Sets `count` on collections whose items a loop copied or removed.
fn update_counts(xml: &str) -> Result<String, FillError> {
    let mut xml = xml.to_string();
    for (container, item) in [("mergeCells", "mergeCell"), ("dataValidations", "dataValidation")] {
        xml = update_count(&xml, container, item)?;
    }
    Ok(xml)
}

fn update_count(xml: &str, container_name: &str, item: &str) -> Result<String, FillError> {
    let tokens = tokenize(xml).map_err(reference_error)?;
    let mut count = 0usize;
    let mut container = None;
    let mut offset = 0usize;
    for token in &tokens {
        if let Token::Element { kind, name, raw } = token {
            match kind {
                ElementKind::SelfClosing | ElementKind::Open if local(name) == item => count += 1,
                ElementKind::SelfClosing | ElementKind::Open if local(name) == container_name => container = Some((offset, offset + raw.len(), *raw)),
                _ => {}
            }
        }
        offset += match token { Token::Element { raw, .. } => raw.len(), Token::Text(text) => text.len() };
    }
    let Some((start, end, raw)) = container else { return Ok(xml.to_string()) };
    let updated = set_attribute(raw, "count", &count.to_string());
    Ok(format!("{}{}{}", &xml[..start], updated, &xml[end..]))
}

enum Geometry {
    Keep,
    Replace(String),
    /// Drop the whole element, children included.
    Remove,
}

fn rewrite_geometry(raw: &str, element: &str, map: &RowMap, original_rows: &[u32]) -> Result<Geometry, FillError> {
    match local(element) {
        "dimension" => rewrite_dimension(raw, map, original_rows),
        "mergeCell" => rewrite_merge(raw, map, original_rows),
        "selection" => rewrite_selection(raw, map, original_rows),
        "conditionalFormatting" | "dataValidation" => {
            let Some((_, _, value)) = attribute_range(raw, "sqref") else { return Ok(Geometry::Keep) };
            let Some(next) = remap_reference_list(value, map, original_rows)? else { return Ok(Geometry::Keep) };
            if next.is_empty() { return Ok(Geometry::Remove); }
            Ok(Geometry::Replace(with_value(raw, "sqref", &next)))
        }
        "autoFilter" | "hyperlink" => {
            let Some((_, _, value)) = attribute_range(raw, "ref") else { return Ok(Geometry::Keep) };
            let Some(next) = remap_reference_list(value, map, original_rows)? else { return Ok(Geometry::Keep) };
            if next.is_empty() { return Err(reference_error("Spreadsheet geometry refers only to rows removed by a loop")); }
            Ok(Geometry::Replace(with_value(raw, "ref", &next)))
        }
        _ => Ok(Geometry::Keep),
    }
}

fn rewrite_dimension(raw: &str, map: &RowMap, original_rows: &[u32]) -> Result<Geometry, FillError> {
    if map.is_empty() { return Ok(Geometry::Remove); }
    let Some((_, _, value)) = attribute_range(raw, "ref") else { return Ok(Geometry::Keep) };
    let Some((left, right)) = value.split_once(':') else { return Ok(Geometry::Keep) };
    let Some((left_col, first)) = split_cell(left) else { return Ok(Geometry::Keep) };
    let Some((right_col, last)) = split_cell(right) else { return Ok(Geometry::Keep) };
    let (low, high) = (first.min(last), first.max(last));
    let first = map_row(first, u32::MAX, u32::MAX, false, false, 0, map, original_rows)
        .or_else(|| map.range(low..=high).find_map(|(_, rows)| rows.first().copied()))
        .ok_or_else(|| reference_error("Worksheet dimension has no surviving rows"))?;
    let last = map_row(last, u32::MAX, u32::MAX, false, true, 0, map, original_rows)
        .or_else(|| map.range(low..=high).next_back().and_then(|(_, rows)| rows.last().copied()))
        .ok_or_else(|| reference_error("Worksheet dimension has no surviving rows"))?;
    let updated = format!("{left_col}{first}:{right_col}{last}");
    if updated == value { return Ok(Geometry::Keep); }
    Ok(Geometry::Replace(with_value(raw, "ref", &updated)))
}

fn rewrite_merge(raw: &str, map: &RowMap, original_rows: &[u32]) -> Result<Geometry, FillError> {
    let Some((_, _, value)) = attribute_range(raw, "ref") else { return Ok(Geometry::Keep) };
    let Some((left, right)) = value.split_once(':') else { return Ok(Geometry::Keep) };
    let Some((left_col, first)) = split_cell(left) else { return Ok(Geometry::Keep) };
    let Some((right_col, last)) = split_cell(right) else { return Ok(Geometry::Keep) };
    if first == last && let Some(rows) = map.get(&first).filter(|rows| rows.len() > 1) {
        let copies = rows.iter().map(|row| with_value(raw, "ref", &format!("{left_col}{row}:{right_col}{row}")));
        return Ok(Geometry::Replace(copies.collect()));
    }
    if first != last && map.range(first.min(last)..=first.max(last)).any(|(_, rows)| rows.len() > 1) {
        return Err(reference_error("A merged range crossing a repeated row cannot be updated safely"));
    }
    let Some(next) = remap_reference_list(value, map, original_rows)? else { return Ok(Geometry::Keep) };
    // A merge left with one cell or none merges nothing.
    match next.split_once(':') {
        Some((left, right)) if left != right => Ok(Geometry::Replace(with_value(raw, "ref", &next))),
        _ => Ok(Geometry::Remove),
    }
}

/// `activeCell` is one cell, so a repeated row keeps it on the first copy. When its row or
/// every `sqref` range is removed, the selection falls back to a surviving cell, else A1.
fn rewrite_selection(raw: &str, map: &RowMap, original_rows: &[u32]) -> Result<Geometry, FillError> {
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
        let first = sqref_now.split_ascii_whitespace().next().and_then(|token| token.split(':').next());
        next_active = Some(first.unwrap_or("A1").to_string());
    }
    if sqref.is_some() && sqref_now.is_empty() {
        let active_now = next_active.as_deref().or(active).unwrap_or("A1");
        next_sqref = Some(active_now.to_string());
    }
    if next_sqref.is_none() && next_active.is_none() { return Ok(Geometry::Keep); }
    let mut changed = raw.to_string();
    if let Some(value) = &next_active { changed = with_value(&changed, "activeCell", value); }
    if let Some(value) = &next_sqref { changed = with_value(&changed, "sqref", value); }
    Ok(Geometry::Replace(changed))
}

/// Replaces an existing attribute value in an opening tag.
fn with_value(raw: &str, name: &str, value: &str) -> String {
    let Some((start, end, _)) = attribute_range(raw, name) else { return raw.to_string() };
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
        if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':') || after.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':') { continue; }
        while raw.as_bytes().get(at).is_some_and(u8::is_ascii_whitespace) { at += 1; }
        if raw.as_bytes().get(at) != Some(&b'=') { continue; }
        at += 1;
        while raw.as_bytes().get(at).is_some_and(u8::is_ascii_whitespace) { at += 1; }
        let quote = *raw.as_bytes().get(at)?;
        if quote != b'"' && quote != b'\'' { continue; }
        let value_start = at + 1;
        let end = raw[value_start..].find(char::from(quote))? + value_start;
        return Some((value_start, end, &raw[value_start..end]));
    }
    None
}

fn remap_reference_list(value: &str, map: &RowMap, original_rows: &[u32]) -> Result<Option<String>, FillError> {
    let mut output = Vec::new();
    let mut changed = false;
    for token in value.split_ascii_whitespace() {
        let Some((left, right)) = token.split_once(':') else {
            let Some((col, row)) = split_cell(token) else { output.push(token.to_string()); continue };
            let Some(rows) = map.get(&row) else {
                if original_rows.binary_search(&row).is_ok() { changed = true; continue; }
                output.push(token.to_string()); continue;
            };
            changed = true;
            output.extend(rows.iter().map(|row| format!("{col}{row}")));
            continue;
        };
        let Some((left_col, first)) = split_cell(left) else { output.push(token.to_string()); continue };
        let Some((right_col, last)) = split_cell(right) else { output.push(token.to_string()); continue };
        let low = first.min(last);
        let high = first.max(last);
        let rows: Vec<u32> = map.range(low..=high).flat_map(|(_, rows)| rows.iter().copied()).collect();
        if rows.is_empty() {
            // A range made only of removed rows is dropped; one that also covers blank rows is not safe to guess.
            let removed = original_rows.iter().filter(|row| (low..=high).contains(*row)).count();
            if removed as u64 == u64::from(high - low) + 1 { changed = true; continue; }
            if removed > 0 { return Err(reference_error("Spreadsheet geometry refers only to rows removed by a loop")); }
            output.push(token.to_string()); continue;
        }
        if rows.iter().any(|row| *row < low || *row > high) || rows.len() != (high - low + 1) as usize || rows.iter().enumerate().any(|(index, row)| index > 0 && rows[index - 1] + 1 != *row) {
            changed = true;
        }
        let first_out = *rows.first().expect("nonempty");
        let last_out = *rows.last().expect("nonempty");
        if first_out != first || last_out != last { changed = true; }
        output.push(format!("{left_col}{first_out}:{right_col}{last_out}"));
    }
    if changed { Ok(Some(output.join(" "))) } else { Ok(None) }
}

fn split_cell(reference: &str) -> Option<(&str, u32)> {
    let mut at = 0;
    while reference.as_bytes().get(at).is_some_and(u8::is_ascii_alphabetic) { at += 1; }
    if at == 0 { return None; }
    let column = &reference[..at];
    column_number(column)?;
    let row = reference[at..].parse::<u32>().ok()?;
    (row > 0 && row <= 1_048_576).then_some((column, row))
}

fn local(name: &str) -> &str { name.rsplit_once(':').map_or(name, |(_, local)| local) }

fn attr<'a>(raw: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = raw;
    while let Some(at) = rest.find(name) {
        let before = rest[..at].chars().next_back();
        let tail = &rest[at + name.len()..];
        rest = tail;
        if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':') { continue; }
        let value = tail.trim_start().strip_prefix('=')?.trim_start();
        let quote = value.chars().next()?;
        if quote != '"' && quote != '\'' { continue; }
        return Some(&value[1..value[1..].find(quote)? + 1]);
    }
    None
}

fn cell_ref_row(reference: &str) -> Option<u32> {
    let bytes = reference.as_bytes();
    let mut at = usize::from(bytes.first() == Some(&b'$'));
    while bytes.get(at) == Some(&b'$') || bytes.get(at).is_some_and(u8::is_ascii_alphabetic) { at += 1; }
    if bytes.get(at) == Some(&b'$') { at += 1; }
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
        assert_eq!(update_formula("A2*$B$1+C$4+$D5+\"A2\"", 2, 3, &[]).unwrap(), "A3*$B$1+C$4+$D6+\"A2\"");
    }

    #[test]
    fn translates_sheet_qualified_and_range_references() {
        assert_eq!(update_formula("'Other Sheet'!A2:B9", 2, 4, &[]).unwrap(), "'Other Sheet'!A4:B11");
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
        assert_eq!(remap_reference_list("A2 A5", &map, &[]).unwrap().as_deref(), Some("A2 A3 A5"));
        assert_eq!(remap_reference_list("A2:B2", &map, &[]).unwrap().as_deref(), Some("A2:B3"));
    }

    #[test]
    fn expands_footer_ranges_and_remaps_absolute_rows() {
        let map = [(1, 1), (2, 2), (2, 3), (3, 4)];
        assert_eq!(update_formula("SUM(B2:B2)", 5, 5, &map).unwrap(), "SUM(B2:B3)");
        assert_eq!(update_formula("$B$2+$B$3", 2, 3, &map).unwrap(), "$B$2+$B$4");
    }

    #[test]
    fn remaps_references_in_static_formulas_after_insertions() {
        let map = [(1, 1), (2, 2), (2, 3), (3, 4)];
        assert_eq!(update_formula("A3+SUM(B2:B2)", 1, 1, &map).unwrap(), "A4+SUM(B2:B3)");
    }

    #[test]
    fn copied_formula_moves_relative_references_to_other_rows() {
        let map = [(1, 1), (2, 2), (2, 3), (3, 4)];
        assert_eq!(update_formula("A1+$A$1", 2, 3, &map).unwrap(), "A2+$A$1");
    }

    #[test]
    fn deleted_rows_become_ref_errors_and_geometry_is_trimmed() {
        let map = [(1, 1), (3, 2)];
        assert_eq!(update_formula_with_rows("A2+A3", 1, 1, &row_mapping(&map), &[1, 2, 3]).unwrap(), "#REF!+A2");
        let xml = r#"<worksheet><dimension ref="A1:B3"/><sheetData><row r="1"/><row r="2"/></sheetData><mergeCells count="0"><mergeCell ref="A2:B2"/></mergeCells></worksheet>"#;
        let result = update_sheet_with_rows(xml, &map, &[1, 2, 3]).unwrap();
        assert!(result.contains(r#"<dimension ref="A1:B2"/>"#));
        assert!(!result.contains("mergeCell ref="));
        assert!(result.contains(r#"<mergeCells count="0">"#));
    }

    #[test]
    fn dimension_keeps_blank_tail_rows_after_structural_insertion() {
        let map = [(1, 1), (2, 2), (2, 3), (3, 4)];
        let xml = r#"<worksheet><dimension ref="A1:B100"/><sheetData><row r="1"/><row r="2"/><row r="3"/><row r="4"/></sheetData></worksheet>"#;
        let result = update_sheet_with_rows(xml, &map, &[1, 2, 3]).unwrap();
        assert!(result.contains(r#"<dimension ref="A1:B101"/>"#));
    }
}
