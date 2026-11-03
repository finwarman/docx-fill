//! Spreadsheet references whose row numbers change when a template row repeats.

use crate::xml::{ElementKind, Token, tokenize};
use crate::properties::set_attribute;
use crate::{FillError, FillErrorId};

/// Update row-relative references in a formula copied from `source_row` to `target_row`.
/// Absolute row references follow Excel's copy semantics and stay fixed.
#[cfg(test)]
pub(super) fn update_formula(formula: &str, source_row: u32, target_row: u32, row_map: &[(u32, u32)]) -> Result<String, FillError> {
    update_formula_with_rows(formula, source_row, target_row, row_map, &[])
}

fn update_formula_with_rows(formula: &str, source_row: u32, target_row: u32, row_map: &[(u32, u32)], original_rows: &[u32]) -> Result<String, FillError> {
    let mut mapping: std::collections::BTreeMap<u32, Vec<u32>> = std::collections::BTreeMap::new();
    for &(source, target) in row_map { mapping.entry(source).or_default().push(target); }
    for targets in mapping.values_mut() { targets.sort_unstable(); targets.dedup(); }
    let copy_delta = if let Some(targets) = mapping.get(&source_row) {
        i64::from(target_row) - i64::from(targets[0])
    } else {
        i64::from(target_row) - i64::from(source_row)
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
        if let Some((end, row_start, row, abs_row)) = read_cell_ref(formula, at) {
            let before = formula[..at].chars().next_back();
            let after = formula[end..].chars().next();
            let boundary_before = before.is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_' && c != '.' && c != '[');
            let boundary_after = after.is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_' && c != '(' && c != ']');
            if boundary_before && boundary_after {
                let right_range_end = before == Some(':');
                let mapped_row = map_row(row, source_row, target_row, abs_row, right_range_end, copy_delta, &mapping, original_rows);
                let Some(row) = mapped_row else {
                    out.push_str("#REF!");
                    at = end;
                    continue;
                };
                let row = i64::from(row);
                if !(1..=1_048_576).contains(&row) {
                    return Err(reference_error("A copied formula refers outside Excel's row limits"));
                }
                out.push_str(&formula[at..row_start]);
                out.push_str(&row.to_string());
                at = end;
                continue;
            }
        }
        out.push(ch);
        at += ch.len_utf8();
    }
    if quoted { return Err(reference_error("A formula has an unclosed string literal")); }
    Ok(out)
}

fn read_cell_ref(text: &str, start: usize) -> Option<(usize, usize, u32, bool)> {
    let bytes = text.as_bytes();
    let mut at = start;
    if bytes.get(at) == Some(&b'$') { at += 1; }
    let col_start = at;
    while bytes.get(at).is_some_and(u8::is_ascii_alphabetic) { at += 1; }
    if at == col_start { return None; }
    column_number(&text[col_start..at])?;
    let abs_row = bytes.get(at) == Some(&b'$');
    if abs_row { at += 1; }
    let row_start = at;
    while bytes.get(at).is_some_and(u8::is_ascii_digit) { at += 1; }
    if at == row_start { return None; }
    let row = text[row_start..at].parse::<u32>().ok()?;
    if row == 0 || row > 1_048_576 { return None; }
    Some((at, row_start, row, abs_row))
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
    let mut source_to_emitted: std::collections::BTreeMap<u32, Vec<u32>> = std::collections::BTreeMap::new();
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
    for token in tokens {
        match token {
            Token::Text(_) => {}
            Token::Element { kind, name, raw } => {
                if kind != ElementKind::Close {
                    if let Some(changed) = rewrite_geometry(raw, name, &source_to_emitted, original_rows)? {
                        replacements.push((offset, offset + raw.len(), changed));
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
                            let replaced = update_formula_with_rows(&formula, source, target, row_map, original_rows)?;
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
        offset += match token { Token::Element { raw, .. } => raw.len(), Token::Text(text) => text.len() };
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
    update_merge_count(&out)
}

#[allow(clippy::too_many_arguments)]
fn map_row(
    row: u32,
    formula_source: u32,
    formula_target: u32,
    absolute: bool,
    range_end: bool,
    copy_delta: i64,
    mapping: &std::collections::BTreeMap<u32, Vec<u32>>,
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

fn update_merge_count(xml: &str) -> Result<String, FillError> {
    let tokens = tokenize(xml).map_err(reference_error)?;
    let mut count = 0usize;
    let mut container = None;
    let mut offset = 0usize;
    for token in &tokens {
        if let Token::Element { kind, name, raw } = token {
            match kind {
                ElementKind::SelfClosing | ElementKind::Open if local(name) == "mergeCell" => count += 1,
                ElementKind::SelfClosing | ElementKind::Open if local(name) == "mergeCells" => container = Some((offset, offset + raw.len(), *raw)),
                _ => {}
            }
        }
        offset += match token { Token::Element { raw, .. } => raw.len(), Token::Text(text) => text.len() };
    }
    let Some((start, end, raw)) = container else { return Ok(xml.to_string()) };
    let updated = set_attribute(raw, "count", &count.to_string());
    Ok(format!("{}{}{}", &xml[..start], updated, &xml[end..]))
}

#[allow(clippy::collapsible_if)]
fn rewrite_geometry(raw: &str, element: &str, map: &std::collections::BTreeMap<u32, Vec<u32>>, original_rows: &[u32]) -> Result<Option<String>, FillError> {
    let kind = local(element);
    if kind == "dimension" && map.is_empty() { return Ok(Some(String::new())); }
    if kind == "dimension" {
        let Some((start, end, value)) = attribute_range(raw, "ref") else { return Ok(None) };
        let Some((left, right)) = value.split_once(':') else { return Ok(None) };
        let Some((left_col, first)) = split_cell(left) else { return Ok(None) };
        let Some((right_col, last)) = split_cell(right) else { return Ok(None) };
        let (low, high) = (first.min(last), first.max(last));
        let first = map_row(first, u32::MAX, u32::MAX, false, false, 0, map, original_rows)
            .or_else(|| map.range(low..=high).find_map(|(_, rows)| rows.first().copied()))
            .ok_or_else(|| reference_error("Worksheet dimension has no surviving rows"))?;
        let last = map_row(last, u32::MAX, u32::MAX, false, true, 0, map, original_rows)
            .or_else(|| map.range(low..=high).next_back().and_then(|(_, rows)| rows.last().copied()))
            .ok_or_else(|| reference_error("Worksheet dimension has no surviving rows"))?;
        let updated = format!("{left_col}{first}:{right_col}{last}");
        if updated == value { return Ok(None); }
        let mut changed = raw.to_string();
        changed.replace_range(start..end, &updated);
        return Ok(Some(changed));
    }
    if kind == "mergeCell" {
        if let Some((_, _, value)) = attribute_range(raw, "ref") {
            let Some((left, right)) = value.split_once(':') else { return Ok(None) };
            let Some((left_col, first)) = split_cell(left) else { return Ok(None) };
            let Some((right_col, last)) = split_cell(right) else { return Ok(None) };
            if first == last && let Some(rows) = map.get(&first).filter(|rows| rows.len() > 1) {
                let mut copies = Vec::with_capacity(rows.len());
                for row in rows {
                    let mut copy = raw.to_string();
                    let Some((value_start, value_end, _)) = attribute_range(&copy, "ref") else { continue };
                    copy.replace_range(value_start..value_end, &format!("{left_col}{row}:{right_col}{row}"));
                    copies.push(copy);
                }
                return Ok(Some(copies.concat()));
            }
            if first == last && original_rows.binary_search(&first).is_ok() && !map.contains_key(&first) { return Ok(Some(String::new())); }
            if first != last && map.range(first.min(last)..=first.max(last)).any(|(_, rows)| rows.len() > 1) {
                return Err(reference_error("A merged range crossing a repeated row cannot be updated safely"));
            }
        }
    }
    let names: &[&str] = match kind {
        "mergeCell" | "autoFilter" | "conditionalFormatting" | "dataValidation" => &["ref", "sqref"],
        "hyperlink" => &["ref"],
        "selection" => &["activeCell", "sqref"],
        _ => return Ok(None),
    };
    let mut changed = raw.to_string();
    let mut modified = false;
    for name in names {
        let Some((_, _, value)) = attribute_range(raw, name) else { continue };
        let Some(next) = remap_reference_list(value, map, original_rows)? else { continue };
        let Some((start, end, _)) = attribute_range(&changed, name) else { continue };
        changed.replace_range(start..end, &next);
        modified = true;
    }
    Ok(modified.then_some(changed))
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

fn remap_reference_list(value: &str, map: &std::collections::BTreeMap<u32, Vec<u32>>, original_rows: &[u32]) -> Result<Option<String>, FillError> {
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
            if original_rows.iter().any(|row| (low..=high).contains(row)) { return Err(reference_error("Spreadsheet geometry refers only to rows removed by a loop")); }
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
        assert_eq!(update_formula_with_rows("A2+A3", 1, 1, &map, &[1, 2, 3]).unwrap(), "#REF!+A2");
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
