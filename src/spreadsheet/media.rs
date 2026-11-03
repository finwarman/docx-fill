//! Images and chart data owned by a spreadsheet package.
use super::effects::{CellEffect, EffectKind};
use super::package_util::{self as pkg, OFFICE_NS, Parts, SHEET_NS};
use super::xmlutil::{elements, escaped, local_name, prefix};
use crate::xml::{read_attribute, unescape};
use crate::{FillError, FillErrorId, FillHooks};
use serde_json::Value;

const DRAWING_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing";
const A_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const IMAGE_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";
const DRAWING_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing";
const CHART_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart";

pub(super) fn apply(
    parts: &mut Parts,
    workbook: &str,
    sheet: &str,
    effects: &[CellEffect],
    hooks: &mut dyn FillHooks,
) -> Result<(), FillError> {
    for effect in effects {
        match effect.kind {
            EffectKind::Image => insert_image(parts, sheet, &effect.cell, &effect.value, hooks)
                .map_err(|error| error.in_part(sheet))?,
            EffectKind::Chart => fill_chart(parts, workbook, sheet, &effect.cell, &effect.value)
                .map_err(|error| error.in_part(sheet))?,
            _ => {}
        }
    }
    Ok(())
}
fn drawing(parts: &mut Parts, sheet: &str) -> Result<String, FillError> {
    let related = pkg::targets(parts, sheet, DRAWING_TYPE)?;
    if let Some((_, path)) = related.first() {
        return Ok(path.clone());
    }
    let path = pkg::unique_path(parts, "xl/drawings", "docxFillDrawing", "xml");
    let data = format!(
        "<xdr:wsDr xmlns:xdr=\"{DRAWING_NS}\" xmlns:a=\"{A_NS}\" xmlns:r=\"{OFFICE_NS}\"/>"
    );
    parts.insert(path.clone(), data.into_bytes());
    pkg::type_override(
        parts,
        &path,
        "application/vnd.openxmlformats-officedocument.drawing+xml",
    )?;
    let id = pkg::add_relation(
        parts,
        sheet,
        DRAWING_TYPE,
        &pkg::relative(sheet, &path),
        false,
    )?;
    let xml = pkg::xml(parts, sheet)?;
    let root = elements(xml)?.remove(0);
    let node = format!(
        "<{}drawing xmlns:r=\"{OFFICE_NS}\" r:id=\"{id}\"/>",
        prefix(&root.name)
    );
    let xml = pkg::insert_sheet_child(
        xml,
        &node,
        &[
            "legacyDrawing",
            "legacyDrawingHF",
            "picture",
            "oleObjects",
            "controls",
            "webPublishItems",
            "tableParts",
            "extLst",
        ],
    )?;
    parts.insert(sheet.into(), xml.into_bytes());
    Ok(path)
}
fn insert_image(
    parts: &mut Parts,
    sheet: &str,
    cell: &str,
    value: &Value,
    hooks: &mut dyn FillHooks,
) -> Result<(), FillError> {
    if value.is_null() || value == &Value::Bool(false) {
        return Ok(());
    }
    let Some(image) = hooks.image(sheet, value) else {
        return Ok(());
    };
    if image.bytes.len() > 64 * 1024 * 1024 {
        return Err(fail("Spreadsheet image exceeds 64 MiB"));
    }
    let extension = image.extension.to_ascii_lowercase();
    let mime = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        _ => return Err(fail("Spreadsheet images support PNG, JPEG, GIF and SVG")),
    };
    if !image.width.is_finite()
        || !image.height.is_finite()
        || image.width <= 0.0
        || image.height <= 0.0
    {
        return Err(fail("Invalid image dimensions"));
    }
    let (width, height) = hooks
        .image_size(sheet, value, &image)
        .map(|size| size.dimensions())
        .unwrap_or((image.width, image.height));
    let (col, row) = coordinate(cell)?;
    let path = drawing(parts, sheet)?;
    let media = add_media(parts, &image.bytes, &extension, mime)?;
    let mut rel = pkg::add_relation(
        parts,
        &path,
        IMAGE_TYPE,
        &pkg::relative(&path, &media),
        false,
    )?;
    let mut svg = None;
    if extension == "svg"
        && let Some(bytes) = hooks.svg_fallback(sheet, value, &image)
    {
        let fallback = add_media(parts, &bytes, "png", "image/png")?;
        svg = Some(rel);
        rel = pkg::add_relation(
            parts,
            &path,
            IMAGE_TYPE,
            &pkg::relative(&path, &fallback),
            false,
        )?;
    }
    let xml = pkg::xml(parts, &path)?;
    let next = elements(xml)?
        .iter()
        .filter(|e| local_name(&e.name) == "cNvPr")
        .filter_map(|e| {
            read_attribute(&xml[e.start..e.open_end], "id").and_then(|id| id.parse::<u32>().ok())
        })
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| fail("Drawing IDs exhausted"))?;
    let properties = hooks.image_properties(sheet, value, &image);
    let link = properties.link_target().map(str::to_string);
    let link_id = link
        .map(|target| {
            pkg::add_relation(
                parts,
                &path,
                &format!("{OFFICE_NS}/hyperlink"),
                &target,
                true,
            )
        })
        .transpose()?;
    let svg=svg.map(|id|format!("<a:extLst><a:ext uri=\"{{96DAC541-7B7A-43D3-8B79-37D633B846F1}}\"><asvg:svgBlip xmlns:asvg=\"http://schemas.microsoft.com/office/drawing/2016/SVG/main\" r:embed=\"{id}\"/></a:ext></a:extLst>")).unwrap_or_default();
    let picture = format!(
        "<pic:pic><pic:nvPicPr><pic:cNvPr id=\"{next}\" name=\"Image {next}\"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed=\"{rel}\">{svg}</a:blip><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{}\" cy=\"{}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic>",
        emu(width)?,
        emu(height)?
    );
    let mut picture = properties.apply(picture, None).replace("pic:", "xdr:");
    if let Some(id) = link_id {
        let all = elements(&picture)?;
        let node = all
            .iter()
            .find(|e| local_name(&e.name) == "cNvPr")
            .ok_or_else(|| fail("Missing image metadata"))?;
        let raw = &picture[node.start..node.open_end];
        let replacement = format!(
            "{}><a:hlinkClick r:id=\"{id}\"/></xdr:cNvPr>",
            raw.trim_end_matches("/>")
        );
        picture.replace_range(node.start..node.end, &replacement);
    }
    let anchor = format!(
        "<xdr:oneCellAnchor xmlns:xdr=\"{DRAWING_NS}\" xmlns:a=\"{A_NS}\" xmlns:r=\"{OFFICE_NS}\"><xdr:from><xdr:col>{}</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>{}</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:ext cx=\"{}\" cy=\"{}\"/>{picture}<xdr:clientData/></xdr:oneCellAnchor>",
        col - 1,
        row - 1,
        emu(width)?,
        emu(height)?
    );
    let updated = pkg::append(pkg::xml(parts, &path)?, "wsDr", &anchor)?;
    parts.insert(path, updated.into_bytes());
    Ok(())
}
fn add_media(
    parts: &mut Parts,
    bytes: &[u8],
    extension: &str,
    mime: &str,
) -> Result<String, FillError> {
    if let Some((path, _)) = parts.iter().find(|(path, value)| {
        path.starts_with("xl/media/")
            && path.ends_with(&format!(".{extension}"))
            && value.as_slice() == bytes
    }) {
        return Ok(path.clone());
    }
    let path = pkg::unique_path(parts, "xl/media", "docxFillImage", extension);
    parts.insert(path.clone(), bytes.to_vec());
    pkg::type_override(parts, &path, mime)?;
    Ok(path)
}
fn emu(px: f64) -> Result<u64, FillError> {
    let value = px * 9525.0;
    if !value.is_finite() || !(1.0..=i32::MAX as f64).contains(&value) {
        return Err(fail("Image display size is outside drawing limits"));
    }
    Ok(value.round() as u64)
}
fn coordinate(cell: &str) -> Result<(u32, u32), FillError> {
    let at = cell
        .find(|c: char| c.is_ascii_digit())
        .ok_or_else(|| fail("Invalid image cell"))?;
    let mut col = 0u32;
    for b in cell[..at].bytes() {
        if !b.is_ascii_uppercase() {
            return Err(fail("Invalid image column"));
        }
        col = col
            .checked_mul(26)
            .and_then(|n| n.checked_add(u32::from(b - b'A' + 1)))
            .ok_or_else(|| fail("Image column overflow"))?;
    }
    let row = cell[at..]
        .parse::<u32>()
        .map_err(|_| fail("Invalid image row"))?;
    if !(1..=16384).contains(&col) || !(1..=1048576).contains(&row) {
        return Err(fail("Image coordinate outside worksheet"));
    }
    Ok((col, row))
}
fn fail(message: impl Into<String>) -> FillError {
    pkg::error(message).with_id(FillErrorId::SpreadsheetCell)
}

fn fill_chart(
    parts: &mut Parts,
    workbook: &str,
    sheet: &str,
    _cell: &str,
    value: &Value,
) -> Result<(), FillError> {
    let mut charts = Vec::new();
    for (_, drawing) in pkg::targets(parts, sheet, DRAWING_TYPE)? {
        for (id, path) in pkg::targets(parts, &drawing, CHART_TYPE)? {
            charts.push((id, path));
        }
    }
    let requested = value.get("target").and_then(Value::as_str);
    let selected = match requested {
        Some(target) => charts
            .iter()
            .find(|(id, path)| {
                id == target || path == target || path.rsplit('/').next() == Some(target)
            })
            .ok_or_else(|| fail("Chart target was not found on the worksheet"))?,
        None => {
            if charts.len() != 1 {
                return Err(fail("Chart data needs one chart or an explicit target"));
            }
            &charts[0]
        }
    };
    let chart_path = selected.1.clone();
    let source = pkg::xml(parts, &chart_path)?;
    let filled = crate::charts::fill_spreadsheet_chart(source, value)?;
    let (sheet_name, sheet_path) = new_data_sheet(parts, workbook)?;
    let (chart, data) = link_chart_data(&filled, &sheet_name)?;
    parts.insert(chart_path, chart.into_bytes());
    parts.insert(sheet_path, data.into_bytes());
    Ok(())
}
fn new_data_sheet(parts: &mut Parts, workbook: &str) -> Result<(String, String), FillError> {
    let xml = pkg::xml(parts, workbook)?.to_string();
    let all = elements(&xml)?;
    let names: std::collections::BTreeSet<_> = all
        .iter()
        .filter(|e| local_name(&e.name) == "sheet")
        .filter_map(|e| read_attribute(&xml[e.start..e.open_end], "name").map(unescape))
        .collect();
    let name = (1..)
        .map(|i| format!("DocxFillChart{i}"))
        .find(|name| !names.contains(name))
        .expect("finite sheets");
    let id = all
        .iter()
        .filter(|e| local_name(&e.name) == "sheet")
        .filter_map(|e| {
            read_attribute(&xml[e.start..e.open_end], "sheetId").and_then(|s| s.parse::<u32>().ok())
        })
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| fail("Sheet IDs exhausted"))?;
    let path = pkg::unique_path(parts, "xl/worksheets", "docxFillChart", "xml");
    parts.insert(
        path.clone(),
        format!("<worksheet xmlns=\"{SHEET_NS}\"><sheetData/></worksheet>").into_bytes(),
    );
    let rel = pkg::add_relation(
        parts,
        workbook,
        &format!("{OFFICE_NS}/worksheet"),
        &pkg::relative(workbook, &path),
        false,
    )?;
    let root = all.first().ok_or_else(|| fail("Missing workbook root"))?;
    let node = format!(
        "<{}sheet xmlns:r=\"{OFFICE_NS}\" name=\"{name}\" sheetId=\"{id}\" state=\"hidden\" r:id=\"{rel}\"/>",
        prefix(&root.name)
    );
    parts.insert(
        workbook.into(),
        pkg::append(&xml, "sheets", &node)?.into_bytes(),
    );
    pkg::type_override(
        parts,
        &path,
        "application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml",
    )?;
    Ok((name, path))
}
fn link_chart_data(chart: &str, sheet: &str) -> Result<(String, String), FillError> {
    let all = elements(chart)?;
    let mut edits = Vec::new();
    let mut rows: std::collections::BTreeMap<usize, Vec<String>> =
        std::collections::BTreeMap::new();
    let mut col = 0u32;
    for literal in all
        .iter()
        .filter(|e| matches!(local_name(&e.name), "strLit" | "numLit"))
    {
        col += 1;
        if col > 16384 {
            return Err(fail("Chart data has too many columns"));
        }
        let text = local_name(&literal.name) == "strLit";
        let p = prefix(&literal.name);
        let body = &chart[literal.open_end..literal.close_start];
        let nodes = elements(body)?;
        let count = nodes
            .iter()
            .find(|e| local_name(&e.name) == "ptCount")
            .and_then(|e| read_attribute(&body[e.start..e.open_end], "val"))
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0);
        if count == 0 || count > 1_000_000 {
            return Err(fail(
                "Chart data must contain between one and one million points",
            ));
        }
        let column = column_name(col);
        for point in nodes.iter().filter(|e| local_name(&e.name) == "pt") {
            let index = read_attribute(&body[point.start..point.open_end], "idx")
                .and_then(|v| v.parse::<usize>().ok())
                .ok_or_else(|| fail("Invalid chart point"))?;
            if index >= count {
                return Err(fail("Chart point index exceeds its range"));
            }
            let content = &body[point.open_end..point.close_start];
            let value = elements(content)?
                .into_iter()
                .find(|e| local_name(&e.name) == "v")
                .map(|e| unescape(&content[e.open_end..e.close_start]))
                .unwrap_or_default();
            let address = format!("{column}{}", index + 1);
            let cell = if text {
                format!(
                    "<c r=\"{address}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
                    escaped(&value)
                )
            } else {
                format!("<c r=\"{address}\"><v>{}</v></c>", escaped(&value))
            };
            rows.entry(index + 1).or_default().push(cell);
        }
        let kind = if text { "str" } else { "num" };
        let formula = format!(
            "'{}'!${column}$1:${column}${count}",
            sheet.replace('\'', "''")
        );
        let replacement = format!(
            "<{p}{kind}Ref><{p}f>{}</{p}f><{p}{kind}Cache>{body}</{p}{kind}Cache></{p}{kind}Ref>",
            escaped(&formula)
        );
        edits.push((literal.start, literal.end, replacement));
    }
    if edits.is_empty() {
        return Err(fail("Chart layout does not expose standard data caches"));
    }
    let mut chart = chart.to_string();
    for (start, end, value) in edits.into_iter().rev() {
        chart.replace_range(start..end, &value);
    }
    let rows = rows
        .into_iter()
        .map(|(row, cells)| format!("<row r=\"{row}\">{}</row>", cells.concat()))
        .collect::<String>();
    Ok((
        chart,
        format!("<worksheet xmlns=\"{SHEET_NS}\"><sheetData>{rows}</sheetData></worksheet>"),
    ))
}
fn column_name(mut col: u32) -> String {
    let mut text = String::new();
    while col > 0 {
        col -= 1;
        text.insert(0, char::from(b'A' + (col % 26) as u8));
        col /= 26;
    }
    text
}
