//! Cell hyperlinks and notes stored in worksheet relationships and content parts.
use std::collections::BTreeSet;

use serde_json::Value;

use crate::xml::{read_attribute, unescape};
use crate::{FillError, FillErrorId};

use super::effects::{CellEffect, EffectKind};
use super::package_util::{self, Parts};
use super::references::WorksheetMap;
use super::xmlutil::{elements, escaped, local_name, prefix};

pub(super) fn transform_existing(
    parts: &mut Parts,
    sheet: &str,
    map: &WorksheetMap,
) -> Result<(), FillError> {
    let comments_kind = format!("{}/comments", package_util::OFFICE_NS);
    let comment_parts = package_util::targets(parts, sheet, &comments_kind)?;
    if comment_parts.len() > 1 {
        return Err(package_util::error("Worksheet has multiple comments parts"));
    }
    if let Some((_, path)) = comment_parts.first() {
        let original = package_util::xml(parts, path)?.to_string();
        let all = elements(&original)?;
        if let Some(list) = all.iter().find(|e| local_name(&e.name) == "commentList") {
            let comments = direct_children(&all, list)
                .into_iter()
                .filter(|e| local_name(&e.name) == "comment")
                .collect::<Vec<_>>();
            let mut rewritten = Vec::new();
            for comment in comments {
                let raw = &original[comment.start..comment.end];
                let Some(reference) =
                    read_attribute(&original[comment.start..comment.open_end], "ref")
                else {
                    return Err(package_util::error("Comment has no cell reference"));
                };
                let Some((column, row)) = parse_cell(reference) else {
                    return Err(package_util::error("Comment has an invalid cell reference"));
                };
                for (target_column, target_row) in mapped_cells(map, row, column) {
                    let target = format!("{}{}", column_name(target_column), target_row);
                    rewritten.push(crate::properties::set_attribute(raw, "ref", &target));
                }
            }
            let mut changed = original.clone();
            if original[list.start..list.open_end].ends_with("/>") {
                changed.replace_range(
                    list.start..list.end,
                    &format!(
                        "{}>{}</{}>",
                        &original[list.start..list.open_end - 2],
                        rewritten.join(""),
                        list.name
                    ),
                );
            } else {
                changed.replace_range(list.open_end..list.close_start, &rewritten.join(""));
            }
            parts.insert(path.clone(), changed.into_bytes());
        }
    }

    let vml_kind = format!("{}/vmlDrawing", package_util::OFFICE_NS);
    for (_, path) in package_util::targets(parts, sheet, &vml_kind)? {
        let original = package_util::xml(parts, &path)?.to_string();
        let all = elements(&original)?;
        let mut replacements = Vec::new();
        for shape in all.iter().filter(|e| local_name(&e.name) == "shape") {
            let children = all.iter().filter(|e| {
                local_name(&e.name) == "Row"
                    && e.start >= shape.open_end
                    && e.end <= shape.close_start
            });
            let Some(row_node) = children.clone().next() else {
                continue;
            };
            let col_node = all.iter().find(|e| {
                local_name(&e.name) == "Column"
                    && e.start >= shape.open_end
                    && e.end <= shape.close_start
            });
            let Some(col_node) = col_node else { continue };
            let row = original[row_node.open_end..row_node.close_start]
                .trim()
                .parse::<u32>()
                .ok();
            let column = original[col_node.open_end..col_node.close_start]
                .trim()
                .parse::<u32>()
                .ok();
            let (Some(row), Some(column)) = (row, column) else {
                continue;
            };
            let targets = mapped_cells(map, row + 1, column + 1);
            if targets.is_empty() {
                replacements.push((shape.start, shape.end, String::new()));
                continue;
            }
            let raw = &original[shape.start..shape.end];
            let mut clones = Vec::new();
            for (index, (target_column, target_row)) in targets.iter().enumerate() {
                let mut copy = raw.to_string();
                if index > 0
                    && let Some(id) =
                        read_attribute(&copy[..copy.find('>').unwrap_or(copy.len()) + 1], "id")
                {
                    copy =
                        crate::properties::set_attribute(&copy, "id", &format!("{id}_copy{index}"));
                }
                copy =
                    replace_text_element(&copy, "Row", &target_row.saturating_sub(1).to_string())?;
                copy = replace_text_element(
                    &copy,
                    "Column",
                    &target_column.saturating_sub(1).to_string(),
                )?;
                clones.push(copy);
            }
            replacements.push((shape.start, shape.end, clones.join("")));
        }
        replacements.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
        let mut changed = original;
        for (start, end, value) in replacements {
            changed.replace_range(start..end, &value);
        }
        parts.insert(path, changed.into_bytes());
    }
    Ok(())
}

fn direct_children<'a>(
    all: &'a [super::xmlutil::Element],
    parent: &super::xmlutil::Element,
) -> Vec<&'a super::xmlutil::Element> {
    let mut out = Vec::new();
    let mut skip = parent.open_end;
    for item in all
        .iter()
        .filter(|e| e.start >= parent.open_end && e.end <= parent.close_start)
    {
        if item.start >= skip {
            out.push(item);
            skip = item.end;
        }
    }
    out
}

fn mapped_cells(map: &WorksheetMap, row: u32, column: u32) -> Vec<(u32, u32)> {
    let explicit = map
        .cells
        .iter()
        .filter(|cell| cell.source_row == row && cell.source_column == column)
        .map(|cell| (cell.target_column, cell.target_row))
        .collect::<Vec<_>>();
    if !explicit.is_empty() {
        return explicit;
    }
    map.rows
        .get(&row)
        .into_iter()
        .flatten()
        .map(|target| (column, *target))
        .collect()
}

fn column_name(mut n: u32) -> String {
    let mut out = String::new();
    while n > 0 {
        let d = ((n - 1) % 26) as u8;
        out.push(char::from(b'A' + d));
        n = (n - 1) / 26;
    }
    out.chars().rev().collect()
}

fn replace_text_element(xml: &str, name: &str, value: &str) -> Result<String, FillError> {
    let all = elements(xml)?;
    if let Some(node) = all.iter().find(|e| local_name(&e.name) == name) {
        let mut changed = xml.to_string();
        changed.replace_range(node.open_end..node.close_start, value);
        Ok(changed)
    } else {
        Ok(xml.to_string())
    }
}

pub(super) fn apply(
    parts: &mut Parts,
    workbook: &str,
    sheet: &str,
    effects: &[CellEffect],
) -> Result<(), FillError> {
    let mut seen_links = BTreeSet::new();
    let mut seen_comments = BTreeSet::new();
    for effect in effects {
        match effect.kind {
            EffectKind::Hyperlink => {
                if !seen_links.insert(effect.cell.as_str()) {
                    return Err(cell_error(
                        &effect.cell,
                        "Multiple hyperlink values target one cell",
                    ));
                }
                add_hyperlink(parts, sheet, &effect.cell, &effect.value)?;
            }
            EffectKind::Comment => {
                if !seen_comments.insert(effect.cell.as_str()) {
                    return Err(cell_error(
                        &effect.cell,
                        "Multiple note values target one cell",
                    ));
                }
                add_comment(parts, workbook, sheet, &effect.cell, &effect.value)?;
            }
            EffectKind::Style | EffectKind::Image | EffectKind::Chart => {}
        }
    }
    Ok(())
}

fn add_hyperlink(
    parts: &mut Parts,
    sheet: &str,
    cell: &str,
    value: &Value,
) -> Result<(), FillError> {
    let (target, tooltip, display) = hyperlink_fields(value).ok_or_else(|| {
        cell_error(
            cell,
            "Hyperlinks require a string target or target descriptor",
        )
    })?;
    validate_target(&target).map_err(|message| cell_error(cell, message))?;
    let mut sheet_xml = package_util::xml(parts, sheet)?.to_string();
    let sheet_elements = elements(&sheet_xml)?;
    let root = sheet_elements
        .first()
        .ok_or_else(|| cell_error(cell, "Worksheet is empty"))?;
    let p = prefix(&root.name);
    if !sheet_xml[..root.open_end].contains("xmlns:r=") {
        let open = &sheet_xml[root.start..root.open_end];
        let at = open.rfind('>').unwrap_or(open.len());
        let changed = format!(
            "{} xmlns:r=\"{}\"{}",
            &open[..at],
            package_util::OFFICE_NS,
            &open[at..]
        );
        sheet_xml.replace_range(root.start..root.open_end, &changed);
        parts.insert(sheet.to_string(), sheet_xml.as_bytes().to_vec());
    }
    let rel_id = if let Some(location) = target.strip_prefix('#') {
        if location.is_empty() {
            return Err(cell_error(cell, "Internal hyperlink location is empty"));
        }
        None
    } else {
        Some(package_util::add_relation(
            parts,
            sheet,
            &format!("{}/hyperlink", package_util::OFFICE_NS),
            &target,
            true,
        )?)
    };
    let existing = package_util::xml(parts, sheet)?.to_string();
    if let Some((start, end)) = element_range(&existing, "hyperlinks")? {
        let link_xml = &existing[start..end];
        if elements(link_xml)?.iter().any(|node| {
            local_name(&node.name) == "hyperlink"
                && read_attribute(&link_xml[node.start..node.open_end], "ref") == Some(cell)
        }) {
            return Err(cell_error(cell, "Cell already has a hyperlink"));
        }
        let node = hyperlink_xml(
            p,
            cell,
            rel_id.as_deref(),
            target.strip_prefix('#'),
            tooltip.as_deref(),
            display.as_deref(),
        );
        let changed = package_util::append(&existing, "hyperlinks", &node)?;
        parts.insert(sheet.to_string(), changed.into_bytes());
    } else {
        let node = format!(
            "<{p}hyperlinks>{}</{p}hyperlinks>",
            hyperlink_xml(
                p,
                cell,
                rel_id.as_deref(),
                target.strip_prefix('#'),
                tooltip.as_deref(),
                display.as_deref()
            )
        );
        let updated = package_util::insert_sheet_child(
            &existing,
            &node,
            &[
                "printOptions",
                "pageMargins",
                "pageSetup",
                "headerFooter",
                "drawing",
                "legacyDrawing",
                "tableParts",
                "extLst",
            ],
        )?;
        parts.insert(sheet.to_string(), updated.into_bytes());
    }
    Ok(())
}

fn hyperlink_xml(
    prefix: &str,
    cell: &str,
    relationship: Option<&str>,
    location: Option<&str>,
    tooltip: Option<&str>,
    _display: Option<&str>,
) -> String {
    let mut xml = format!("<{prefix}hyperlink ref=\"{}\"", escaped(cell));
    if let Some(relationship) = relationship {
        xml.push_str(&format!(" r:id=\"{}\"", escaped(relationship)));
    }
    if let Some(location) = location {
        xml.push_str(&format!(" location=\"{}\"", escaped(location)));
    }
    if let Some(tooltip) = tooltip {
        xml.push_str(&format!(" tooltip=\"{}\"", escaped(tooltip)));
    }
    xml.push_str("/>");
    xml
}

fn add_comment(
    parts: &mut Parts,
    workbook: &str,
    sheet: &str,
    cell: &str,
    value: &Value,
) -> Result<(), FillError> {
    let (text, author) = comment_fields(value)
        .ok_or_else(|| cell_error(cell, "A note requires text and an optional author"))?;
    if text.encode_utf16().count() > 32_767 {
        return Err(cell_error(
            cell,
            "A note exceeds Excel's 32,767 character limit",
        ));
    }
    let (column, row) =
        parse_cell(cell).ok_or_else(|| cell_error(cell, "Invalid note cell address"))?;
    let comment_rel_kind = format!("{}/comments", package_util::OFFICE_NS);
    let comments_existing = package_util::targets(parts, sheet, &comment_rel_kind)?;
    if comments_existing.len() > 1 {
        return Err(cell_error(cell, "Worksheet has multiple comments parts"));
    }
    let comments_path = if let Some((_, path)) = comments_existing.first() {
        path.clone()
    } else {
        let dir = workbook.rsplit_once('/').map_or("xl", |(dir, _)| dir);
        package_util::unique_path(parts, dir, "comments", "xml")
    };
    let comment_xml = if parts.contains_key(&comments_path) {
        package_util::xml(parts, &comments_path)?.to_string()
    } else {
        format!(
            "<comments xmlns=\"{}\"><authors/><commentList/></comments>",
            package_util::SHEET_NS
        )
    };
    let comment_elements = elements(&comment_xml)?;
    let root = comment_elements
        .first()
        .ok_or_else(|| cell_error(cell, "Comments part is empty"))?;
    let p = prefix(&root.name);
    let all = elements(&comment_xml)?;
    let authors = all
        .iter()
        .find(|e| local_name(&e.name) == "authors")
        .ok_or_else(|| cell_error(cell, "Comments part has no authors"))?;
    let list = all
        .iter()
        .find(|e| local_name(&e.name) == "commentList")
        .ok_or_else(|| cell_error(cell, "Comments part has no commentList"))?;
    let old_author_xml = &comment_xml[authors.open_end..authors.close_start];
    let old_authors: Vec<String> = elements(old_author_xml)?
        .into_iter()
        .filter(|e| local_name(&e.name) == "author")
        .map(|e| unescape(&old_author_xml[e.open_end..e.close_start]))
        .collect();
    let author_id = old_authors
        .iter()
        .position(|name| name == &author)
        .unwrap_or(old_authors.len());
    let list_xml = &comment_xml[list.open_end..list.close_start];
    if elements(list_xml)?.iter().any(|e| {
        local_name(&e.name) == "comment"
            && read_attribute(&list_xml[e.start..e.open_end], "ref") == Some(cell)
    }) {
        return Err(cell_error(cell, "Cell already has a note"));
    }
    let comment = format!(
        "<{p}comment ref=\"{}\" authorId=\"{author_id}\"><{p}text><{p}t{}>{}</{p}t></{p}text></{p}comment>",
        escaped(cell),
        space_preserve(&text),
        escaped(&excel_escape(&text))
    );
    let mut updated = comment_xml.clone();
    if author_id == old_authors.len() {
        updated = package_util::append(
            &updated,
            "authors",
            &format!("<{p}author>{}</{p}author>", escaped(&author)),
        )?;
    }
    let list = elements(&updated)?
        .into_iter()
        .find(|e| local_name(&e.name) == "commentList")
        .ok_or_else(|| cell_error(cell, "Comments part has no commentList"))?;
    let _ = list;
    updated = package_util::append(&updated, "commentList", &comment)?;
    parts.insert(comments_path.clone(), updated.into_bytes());
    if comments_existing.is_empty() {
        let rel = package_util::add_relation(
            parts,
            sheet,
            &comment_rel_kind,
            &package_util::relative(sheet, &comments_path),
            false,
        )?;
        let _ = rel;
        package_util::type_override(
            parts,
            &comments_path,
            "application/vnd.openxmlformats-officedocument.spreadsheetml.comments+xml",
        )?;
    }
    add_vml_note(
        parts,
        sheet,
        &comments_path,
        cell,
        column,
        row,
        text.as_str(),
    )?;
    Ok(())
}

fn add_vml_note(
    parts: &mut Parts,
    sheet: &str,
    comments_path: &str,
    cell: &str,
    column: u32,
    row: u32,
    text: &str,
) -> Result<(), FillError> {
    let vml_kind = format!("{}/vmlDrawing", package_util::OFFICE_NS);
    let drawings = package_util::targets(parts, sheet, &vml_kind)?;
    if drawings.len() > 1 {
        return Err(cell_error(cell, "Worksheet has multiple VML drawings"));
    }
    let path = if let Some((_, path)) = drawings.first() {
        path.clone()
    } else {
        let dir = sheet
            .rsplit_once('/')
            .map_or("xl/worksheets", |(dir, _)| dir);
        package_util::unique_path(parts, dir, "vmlDrawing", "vml")
    };
    let xml=parts.get(&path).map(|v|std::str::from_utf8(v)).transpose().map_err(|_|cell_error(cell,"Invalid VML drawing XML"))?.map(str::to_string).unwrap_or_else(||"<xml xmlns:v=\"urn:schemas-microsoft-com:vml\" xmlns:o=\"urn:schemas-microsoft-com:office:office\" xmlns:x=\"urn:schemas-microsoft-com:office:excel\"><o:shapelayout v:ext=\"edit\"><o:idmap v:ext=\"edit\" data=\"1\"/></o:shapelayout><v:shapetype id=\"_x0000_t202\" coordsize=\"21600,21600\" o:spt=\"202\" path=\"m,l,21600r21600,l21600,xe\"><v:stroke joinstyle=\"miter\"/><v:path gradientshapeok=\"t\" o:connecttype=\"rect\"/></v:shapetype></xml>".to_string());
    let count = elements(&xml)?
        .iter()
        .filter(|e| {
            local_name(&e.name) == "shape"
                && read_attribute(&xml[e.start..e.open_end], "id")
                    .is_some_and(|id| id.starts_with("_x0000_s"))
        })
        .count()
        + 1024;
    let shape = format!(
        "<v:shape id=\"_x0000_s{count}\" type=\"#_x0000_t202\" style=\"position:absolute;visibility:hidden\" fillcolor=\"#ffffe1\" o:insetmode=\"auto\"><v:fill color2=\"#ffffe1\"/><v:shadow on=\"t\" color=\"black\" obscured=\"t\"/><v:path o:connecttype=\"none\"/><v:textbox style=\"mso-direction-alt:auto\"><div style=\"text-align:left\"/></v:textbox><x:ClientData ObjectType=\"Note\"><x:MoveWithCells/><x:SizeWithCells/><x:AutoFill>False</x:AutoFill><x:Row>{}</x:Row><x:Column>{}</x:Column></x:ClientData></v:shape>",
        row - 1,
        column - 1
    );
    let updated = package_util::append(&xml, "xml", &shape)?;
    parts.insert(path.clone(), updated.into_bytes());
    if drawings.is_empty() {
        package_util::add_relation(
            parts,
            sheet,
            &vml_kind,
            &package_util::relative(sheet, &path),
            false,
        )?;
        let mut sheet_xml = package_util::xml(parts, sheet)?.to_string();
        let sheet_elements = elements(&sheet_xml)?;
        let p = sheet_elements.first().map_or("", |e| prefix(&e.name));
        if let Some(root) = sheet_elements.first()
            && !sheet_xml[..root.open_end].contains("xmlns:r=")
        {
            let open = &sheet_xml[root.start..root.open_end];
            let at = open.rfind('>').unwrap_or(open.len());
            let changed = format!(
                "{} xmlns:r=\"{}\"{}",
                &open[..at],
                package_util::OFFICE_NS,
                &open[at..]
            );
            sheet_xml.replace_range(root.start..root.open_end, &changed);
        }
        let rel_path = package_util::rel_path(sheet);
        let rels = std::str::from_utf8(
            parts
                .get(&rel_path)
                .ok_or_else(|| cell_error(cell, "Worksheet relationships are missing"))?,
        )
        .map_err(|_| cell_error(cell, "Invalid worksheet relationships"))?;
        let vml_id = elements(rels)?
            .iter()
            .filter(|e| local_name(&e.name) == "Relationship")
            .find_map(|e| {
                let raw = &rels[e.start..e.open_end];
                (read_attribute(raw, "Type") == Some(vml_kind.as_str()))
                    .then(|| read_attribute(raw, "Id").map(str::to_string))
                    .flatten()
            })
            .ok_or_else(|| cell_error(cell, "New VML relationship is missing"))?;
        let node = format!("<{p}legacyDrawing r:id=\"{vml_id}\"/>");
        let updated = package_util::insert_sheet_child(&sheet_xml, &node, &["extLst"])?;
        parts.insert(sheet.to_string(), updated.into_bytes());
        package_util::type_override(
            parts,
            &path,
            "application/vnd.openxmlformats-officedocument.vmlDrawing",
        )?;
    }
    let _ = comments_path;
    let _ = text;
    Ok(())
}

fn element_range(xml: &str, name: &str) -> Result<Option<(usize, usize)>, FillError> {
    Ok(elements(xml)?
        .into_iter()
        .find(|e| local_name(&e.name) == name)
        .map(|e| (e.open_end, e.close_start)))
}

fn hyperlink_fields(value: &Value) -> Option<(String, Option<String>, Option<String>)> {
    if let Some(target) = value.as_str() {
        return Some((target.to_string(), None, None));
    }
    let object = value.as_object()?;
    Some((
        object.get("target")?.as_str()?.to_string(),
        object
            .get("tooltip")
            .and_then(Value::as_str)
            .map(str::to_string),
        object
            .get("display")
            .and_then(Value::as_str)
            .map(str::to_string),
    ))
}
fn validate_target(target: &str) -> Result<(), String> {
    if target.len() > 2083 || target.trim() != target || target.chars().any(|c| c.is_control()) {
        return Err("Hyperlink target is invalid or exceeds Excel's URL length limit".into());
    }
    if target.starts_with('#') {
        return if target.len() > 1 {
            Ok(())
        } else {
            Err("Internal hyperlink location is empty".into())
        };
    }
    let scheme = target
        .split_once(':')
        .map(|(s, _)| s.to_ascii_lowercase())
        .ok_or("Hyperlink target must include an approved scheme")?;
    if matches!(scheme.as_str(), "https" | "http" | "mailto") {
        Ok(())
    } else {
        Err("Hyperlink scheme must be http, https, or mailto".into())
    }
}
fn comment_fields(value: &Value) -> Option<(String, String)> {
    if let Some(text) = value.as_str() {
        return Some((text.to_string(), "docx-fill".into()));
    }
    let object = value.as_object()?;
    Some((
        object.get("text")?.as_str()?.to_string(),
        object
            .get("author")
            .and_then(Value::as_str)
            .unwrap_or("docx-fill")
            .to_string(),
    ))
}
fn parse_cell(cell: &str) -> Option<(u32, u32)> {
    let at = cell.bytes().position(|b| !b.is_ascii_alphabetic())?;
    let mut column = 0u32;
    for b in cell[..at].bytes() {
        column = column
            .checked_mul(26)?
            .checked_add((b.to_ascii_uppercase() - b'A' + 1) as u32)?;
    }
    let row = cell[at..].parse::<u32>().ok()?;
    if column == 0 || column > 16384 || row == 0 || row > 1_048_576 {
        None
    } else {
        Some((column, row))
    }
}
fn space_preserve(text: &str) -> &'static str {
    if text.chars().next().is_some_and(char::is_whitespace)
        || text.chars().next_back().is_some_and(char::is_whitespace)
    {
        " xml:space=\"preserve\""
    } else {
        ""
    }
}
fn excel_escape(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if !crate::xml::is_xml_char(ch) {
            out.push_str(&format!("_x{:04X}_", u32::from(ch)));
        } else {
            out.push(ch);
        }
    }
    out
}
fn cell_error(cell: &str, message: impl Into<String>) -> FillError {
    FillError::new(format!("Cell {cell}: {}", message.into())).with_id(FillErrorId::SpreadsheetCell)
}
