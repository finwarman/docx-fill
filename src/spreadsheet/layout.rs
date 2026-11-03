//! Optional approximate row sizing. Excel remains responsible for final text layout.
use super::package_util::{self as pkg, Parts};
use super::xmlutil::{elements, local_name};
use crate::FillError;
use crate::properties::set_attribute;
use crate::xml::{read_attribute, unescape};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RowHeightMode {
    #[default]
    Preserve,
    Estimate,
}

pub(super) fn apply(
    parts: &mut Parts,
    workbook: &str,
    sheet: &str,
    mode: RowHeightMode,
) -> Result<(), FillError> {
    if mode == RowHeightMode::Preserve {
        return Ok(());
    }
    let xml = pkg::xml(parts, sheet)?.to_string();
    let all = elements(&xml)?;
    let default_width = all
        .iter()
        .find(|e| local_name(&e.name) == "sheetFormatPr")
        .and_then(|e| read_attribute(&xml[e.start..e.open_end], "defaultColWidth"))
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(8.43);
    let default_height = all
        .iter()
        .find(|e| local_name(&e.name) == "sheetFormatPr")
        .and_then(|e| read_attribute(&xml[e.start..e.open_end], "defaultRowHeight"))
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(15.0);
    let mut widths = vec![default_width; 16385];
    for col in all.iter().filter(|e| local_name(&e.name) == "col") {
        let raw = &xml[col.start..col.open_end];
        let min = read_attribute(raw, "min")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1)
            .clamp(1, 16384);
        let max = read_attribute(raw, "max")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(min)
            .clamp(min, 16384);
        if let Some(width) = read_attribute(raw, "width")
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v > 0.0)
        {
            widths[min..=max].fill(width);
        }
    }
    let shared = pkg::targets(
        parts,
        workbook,
        &format!("{}/sharedStrings", pkg::OFFICE_NS),
    )?
    .first()
    .map(|(_, path)| pkg::xml(parts, path))
    .transpose()?
    .map(|source| {
        elements(source).map(|nodes| {
            nodes
                .iter()
                .filter(|e| local_name(&e.name) == "si")
                .map(|e| text(&source[e.open_end..e.close_start]))
                .collect::<Result<Vec<_>, _>>()
        })
    })
    .transpose()?
    .transpose()?
    .unwrap_or_default();
    let wrapping = wrapping_styles(parts, workbook)?;
    let mut edits = Vec::new();
    for (row_index, row) in all
        .iter()
        .enumerate()
        .filter(|(_, e)| local_name(&e.name) == "row")
    {
        let mut height = default_height;
        for cell in all
            .iter()
            .skip(row_index + 1)
            .take_while(|e| e.start < row.close_start)
            .filter(|e| local_name(&e.name) == "c")
        {
            let raw = &xml[cell.start..cell.open_end];
            let body = &xml[cell.open_end..cell.close_start];
            let value = if read_attribute(raw, "t") == Some("s") {
                elements(body)?
                    .iter()
                    .find(|e| local_name(&e.name) == "v")
                    .and_then(|e| body[e.open_end..e.close_start].parse::<usize>().ok())
                    .and_then(|i| shared.get(i))
                    .cloned()
                    .unwrap_or_default()
            } else {
                text(body)?
            };
            let column = read_attribute(raw, "r")
                .map(column_number)
                .unwrap_or(1)
                .clamp(1, 16384);
            let style = read_attribute(raw, "s")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(0);
            let wrap = wrapping.get(style).copied().unwrap_or(false);
            let chars = widths[column].max(1.0);
            let lines = value
                .split('\n')
                .map(|line| {
                    if wrap {
                        ((line.chars().count() as f64 / chars).ceil() as usize).max(1)
                    } else {
                        1
                    }
                })
                .sum::<usize>();
            height = height.max((lines as f64 * 15.0).min(409.5));
        }
        let raw = &xml[row.start..row.open_end];
        let existing = read_attribute(raw, "ht")
            .and_then(|h| h.parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(default_height);
        height = height.max(existing).min(409.5);
        if height > existing {
            let open = set_attribute(
                &set_attribute(raw, "ht", &height.to_string()),
                "customHeight",
                "1",
            );
            edits.push((row.start, row.open_end, open));
        }
    }
    if !edits.is_empty() {
        let mut output = String::with_capacity(xml.len());
        let mut at = 0;
        for (start, end, raw) in edits {
            output.push_str(&xml[at..start]);
            output.push_str(&raw);
            at = end;
        }
        output.push_str(&xml[at..]);
        parts.insert(sheet.into(), output.into_bytes());
    }
    Ok(())
}
fn wrapping_styles(parts: &Parts, workbook: &str) -> Result<Vec<bool>, FillError> {
    let relationships = pkg::targets(parts, workbook, &format!("{}/styles", pkg::OFFICE_NS))?;
    let Some((_, path)) = relationships.first() else {
        return Ok(Vec::new());
    };
    let source = pkg::xml(parts, path)?;
    let all = elements(source)?;
    let Some(container) = all.iter().find(|e| local_name(&e.name) == "cellXfs") else {
        return Ok(Vec::new());
    };
    all.iter()
        .filter(|e| {
            local_name(&e.name) == "xf"
                && e.start >= container.open_end
                && e.end <= container.close_start
        })
        .map(|xf| {
            let inner = &source[xf.open_end..xf.close_start];
            Ok(elements(inner)?.iter().any(|e| {
                local_name(&e.name) == "alignment"
                    && matches!(
                        read_attribute(&inner[e.start..e.open_end], "wrapText"),
                        Some("1" | "true")
                    )
            }))
        })
        .collect()
}
fn text(xml: &str) -> Result<String, FillError> {
    Ok(elements(xml)?
        .iter()
        .filter(|e| matches!(local_name(&e.name), "t" | "v"))
        .map(|e| unescape(&xml[e.open_end..e.close_start]))
        .collect())
}
fn column_number(reference: &str) -> usize {
    reference
        .bytes()
        .take_while(u8::is_ascii_alphabetic)
        .fold(0usize, |n, c| {
            n.saturating_mul(26)
                .saturating_add(usize::from(c.to_ascii_uppercase() - b'A' + 1))
        })
}
