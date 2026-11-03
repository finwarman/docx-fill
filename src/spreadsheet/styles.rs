//! Stylesheet XF allocation for cell number formats and visual styles.
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

use crate::xml::{read_attribute, unescape};
use crate::{FillError, FillErrorId};

use super::effects::{CellEffect, EffectKind};
use super::package_util::{self, Parts};
use super::xmlutil::{elements, escaped, local_name, prefix};

const MIME: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml";

pub(super) fn apply(
    parts: &mut Parts,
    workbook: &str,
    effects: &[CellEffect],
) -> Result<(), FillError> {
    let mut merged: BTreeMap<(String, String), Value> = BTreeMap::new();
    for effect in effects.iter().filter(|e| e.kind == EffectKind::Style) {
        let key = (effect.part.clone(), effect.cell.clone());
        let dst = merged
            .entry(key)
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        let dst = dst
            .as_object_mut()
            .ok_or_else(|| cell_error(&effect.cell, "Style must be an object"))?;
        let src = effect
            .value
            .as_object()
            .ok_or_else(|| cell_error(&effect.cell, "Style must be an object"))?;
        for (key, value) in src {
            dst.insert(key.clone(), value.clone());
        }
    }
    let effects: Vec<_> = merged
        .into_iter()
        .map(|((part, cell), value)| CellEffect {
            kind: EffectKind::Style,
            part,
            cell,
            value,
        })
        .collect();
    if effects.is_empty() {
        return Ok(());
    }
    let rel_kind = format!("{}/styles", package_util::OFFICE_NS);
    let rels = package_util::targets(parts, workbook, &rel_kind)?;
    if rels.len() > 1 {
        return Err(error("Workbook has multiple style sheets"));
    }
    let path = if let Some((_, path)) = rels.first() {
        path.clone()
    } else {
        let dir = workbook.rsplit_once('/').map_or("xl", |(dir, _)| dir);
        let path = package_util::unique_path(parts, dir, "styles", "xml");
        package_util::add_relation(
            parts,
            workbook,
            &rel_kind,
            &package_util::relative(workbook, &path),
            false,
        )?;
        package_util::type_override(parts, &path, MIME)?;
        let root = format!(
            "<styleSheet xmlns=\"{}\"><fonts count=\"1\"><font><sz val=\"11\"/><name val=\"Calibri\"/></font></fonts><fills count=\"2\"><fill><patternFill patternType=\"none\"/></fill><fill><patternFill patternType=\"gray125\"/></fill></fills><borders count=\"1\"><border><left/><right/><top/><bottom/><diagonal/></border></borders><cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs><cellXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/></cellXfs></styleSheet>",
            package_util::SHEET_NS
        );
        parts.insert(path.clone(), root.into_bytes());
        path
    };
    let original = package_util::xml(parts, &path)?.to_string();
    let all = elements(&original)?;
    let root = all.first().ok_or_else(|| error("Stylesheet is empty"))?;
    let p = prefix(&root.name);
    let mut xml = original;
    let mut font_ids = read_item_ids(&xml, "fonts", "font")?;
    let mut fill_ids = read_item_ids(&xml, "fills", "fill")?;
    let mut border_ids = read_item_ids(&xml, "borders", "border")?;
    let mut num_ids = read_numfmt_ids(&xml)?;
    let mut xf_ids = read_xfs(&xml)?;
    let mut originals: HashMap<String, String> = HashMap::new();
    let mut cell_styles: BTreeMap<String, Vec<(String, u32)>> = BTreeMap::new();
    for effect in &effects {
        let style = effect
            .value
            .as_object()
            .ok_or_else(|| cell_error(&effect.cell, "Style must be an object"))?;
        reject_unknown(
            style,
            &[
                "numberFormat",
                "format",
                "font",
                "fill",
                "border",
                "alignment",
            ],
        )?;
        let number_format = style
            .get("numberFormat")
            .or_else(|| style.get("format"))
            .and_then(Value::as_str);
        let num_fmt_id = if let Some(format) = number_format {
            Some(numfmt(&mut xml, p, format, &mut num_ids)?)
        } else {
            None
        };
        let font_id = if let Some(font) = style.get("font") {
            Some(add_font(&mut xml, p, font, &mut font_ids)?)
        } else {
            None
        };
        let fill_id = if let Some(fill) = style.get("fill") {
            Some(add_fill(&mut xml, p, fill, &mut fill_ids)?)
        } else {
            None
        };
        let border_id = if let Some(border) = style.get("border") {
            Some(add_border(&mut xml, p, border, &mut border_ids)?)
        } else {
            None
        };
        let alignment = if let Some(alignment) = style.get("alignment") {
            Some(read_alignment(alignment)?)
        } else {
            None
        };
        let sheet_path = &effect.part;
        let sheet_xml = if let Some(xml) = originals.get(sheet_path) {
            xml.clone()
        } else {
            let xml = package_util::xml(parts, sheet_path)?.to_string();
            originals.insert(sheet_path.clone(), xml.clone());
            xml
        };
        let base = cell_style_id(&sheet_xml, &effect.cell)
            .and_then(|id| xf_ids.get(id as usize))
            .cloned()
            .ok_or_else(|| cell_error(&effect.cell, "Cell style index is invalid"))?;
        let xf = make_xf(
            p,
            &base,
            num_fmt_id,
            font_id,
            fill_id,
            border_id,
            alignment.as_deref(),
        )?;
        let xf_id = deduplicate(&mut xml, "cellXfs", "xf", &xf, &mut xf_ids)?;
        cell_styles
            .entry(sheet_path.clone())
            .or_default()
            .push((effect.cell.clone(), xf_id));
    }
    for (sheet, styles) in cell_styles {
        let original = originals.get(&sheet).expect("cached sheet XML");
        let updated = set_cell_styles(original, &styles)?;
        parts.insert(sheet, updated.into_bytes());
    }
    parts.insert(path, xml.into_bytes());
    Ok(())
}

fn read_item_ids(xml: &str, container: &str, item: &str) -> Result<Vec<String>, FillError> {
    let all = elements(xml)?;
    let node = all
        .iter()
        .find(|e| local_name(&e.name) == container)
        .ok_or_else(|| error(format!("Stylesheet has no {container}")))?;
    let children = children(&all, node);
    Ok(children
        .iter()
        .filter(|e| local_name(&e.name) == item)
        .map(|e| xml[e.start..e.end].to_string())
        .collect())
}
fn read_numfmt_ids(xml: &str) -> Result<Vec<(u32, String)>, FillError> {
    let all = elements(xml)?;
    let Some(node) = all.iter().find(|e| local_name(&e.name) == "numFmts") else {
        return Ok(Vec::new());
    };
    let items = children(&all, node);
    items
        .iter()
        .filter(|e| local_name(&e.name) == "numFmt")
        .map(|e| {
            let raw = &xml[e.start..e.open_end];
            let id = read_attribute(raw, "numFmtId")
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| error("Invalid number format ID"))?;
            let format = read_attribute(raw, "formatCode")
                .map(unescape)
                .ok_or_else(|| error("Missing number format code"))?;
            Ok((id, format))
        })
        .collect()
}
fn read_xfs(xml: &str) -> Result<Vec<String>, FillError> {
    read_item_ids(xml, "cellXfs", "xf")
}
fn children<'a>(
    all: &'a [super::xmlutil::Element],
    parent: &super::xmlutil::Element,
) -> Vec<&'a super::xmlutil::Element> {
    let mut out = Vec::new();
    let mut skip = parent.open_end;
    for e in all
        .iter()
        .filter(|e| e.start >= parent.open_end && e.end <= parent.close_start)
    {
        if e.start < skip {
            continue;
        }
        out.push(e);
        skip = e.end;
    }
    out
}
fn numfmt(
    xml: &mut String,
    prefix: &str,
    format: &str,
    existing: &mut Vec<(u32, String)>,
) -> Result<u32, FillError> {
    if format.is_empty() || format.len() > 255 || format.chars().any(char::is_control) {
        return Err(error(
            "Number format must contain 1 to 255 printable characters",
        ));
    }
    if let Some(id) = built_in(format) {
        return Ok(id);
    }
    if let Some((id, _)) = existing.iter().find(|(_, value)| value == format) {
        return Ok(*id);
    }
    let id = (164..=65535)
        .find(|id| !existing.iter().any(|(other, _)| other == id))
        .ok_or_else(|| error("No custom Excel number format IDs remain"))?;
    let node = format!(
        "<{prefix}numFmt numFmtId=\"{id}\" formatCode=\"{}\"/>",
        escaped(format)
    );
    let all = elements(xml)?;
    if all.iter().any(|e| local_name(&e.name) == "numFmts") {
        let mut updated = package_util::append(xml, "numFmts", &node)?;
        updated = set_count(&updated, "numFmts")?;
        *xml = updated;
    } else {
        let root = all.first().ok_or_else(|| error("Stylesheet is empty"))?;
        let node = format!("<{prefix}numFmts count=\"1\">{node}</{prefix}numFmts>");
        xml.insert_str(root.open_end, &node);
    }
    existing.push((id, format.into()));
    Ok(id)
}
fn built_in(format: &str) -> Option<u32> {
    match format {
        "0" => Some(1),
        "0.00" => Some(2),
        "#,##0" => Some(3),
        "#,##0.00" => Some(4),
        "0%" => Some(9),
        "0.00%" => Some(10),
        "0.00E+00" => Some(11),
        "# ?/?" => Some(12),
        "# ??/??" => Some(13),
        "mm-dd-yy" => Some(14),
        "d-mmm-yy" => Some(15),
        "d-mmm" => Some(16),
        "mmm-yy" => Some(17),
        "h:mm AM/PM" => Some(18),
        "h:mm:ss AM/PM" => Some(19),
        "h:mm" => Some(20),
        "h:mm:ss" => Some(21),
        "m/d/yy h:mm" => Some(22),
        "@" => Some(49),
        _ => None,
    }
}
fn add_font(
    xml: &mut String,
    p: &str,
    value: &Value,
    items: &mut Vec<String>,
) -> Result<u32, FillError> {
    let obj = value
        .as_object()
        .ok_or_else(|| error("Font style must be an object"))?;
    reject_unknown(
        obj,
        &[
            "bold",
            "italic",
            "underline",
            "strike",
            "size",
            "name",
            "color",
        ],
    )?;
    let mut inner = String::new();
    for (key, tag) in [
        ("bold", "b"),
        ("italic", "i"),
        ("strike", "strike"),
        ("underline", "u"),
    ] {
        if obj.get(key).is_some_and(|v| v == true) {
            inner.push_str(&format!("<{p}{tag}/>"));
        }
    }
    if let Some(size) = obj.get("size") {
        let n = size
            .as_f64()
            .filter(|n| n.is_finite() && *n >= 1.0 && *n <= 409.0)
            .ok_or_else(|| error("Font size must be between 1 and 409 points"))?;
        inner.push_str(&format!("<{p}sz val=\"{n}\"/>"));
    }
    if let Some(color) = obj.get("color") {
        inner.push_str(&color_node(p, "color", color)?);
    }
    if let Some(name) = obj.get("name") {
        let name = name
            .as_str()
            .filter(|n| !n.is_empty() && n.len() <= 31)
            .ok_or_else(|| error("Font name must contain 1 to 31 characters"))?;
        inner.push_str(&format!("<{p}name val=\"{}\"/>", escaped(name)));
    }
    intern(
        xml,
        "fonts",
        "font",
        &format!("<{p}font>{inner}</{p}font>"),
        items,
    )
}
fn add_fill(
    xml: &mut String,
    p: &str,
    value: &Value,
    items: &mut Vec<String>,
) -> Result<u32, FillError> {
    let (fg, bg, pattern) = if let Some(color) = value.as_str() {
        (Some(color), None, "solid")
    } else {
        let obj = value
            .as_object()
            .ok_or_else(|| error("Fill must be a color or object"))?;
        reject_unknown(obj, &["color", "background", "pattern"])?;
        (
            obj.get("color").and_then(Value::as_str),
            obj.get("background").and_then(Value::as_str),
            obj.get("pattern")
                .and_then(Value::as_str)
                .unwrap_or("solid"),
        )
    };
    if !matches!(
        pattern,
        "none"
            | "solid"
            | "gray125"
            | "darkDown"
            | "darkGray"
            | "darkGrid"
            | "darkHorizontal"
            | "darkTrellis"
            | "darkUp"
            | "darkVertical"
            | "gray0625"
            | "lightDown"
            | "lightGray"
            | "lightGrid"
            | "lightHorizontal"
            | "lightTrellis"
            | "lightUp"
            | "lightVertical"
            | "mediumGray"
    ) {
        return Err(error("Unsupported fill pattern"));
    }
    let mut inner = String::new();
    if let Some(color) = fg {
        inner.push_str(&color_node(p, "fgColor", &Value::String(color.into()))?);
    }
    if let Some(color) = bg {
        inner.push_str(&color_node(p, "bgColor", &Value::String(color.into()))?);
    }
    intern(
        xml,
        "fills",
        "fill",
        &format!(
            "<{p}fill><{p}patternFill patternType=\"{pattern}\">{inner}</{p}patternFill></{p}fill>"
        ),
        items,
    )
}
fn add_border(
    xml: &mut String,
    p: &str,
    value: &Value,
    items: &mut Vec<String>,
) -> Result<u32, FillError> {
    let obj = value
        .as_object()
        .ok_or_else(|| error("Border must be an object"))?;
    reject_unknown(obj, &["left", "right", "top", "bottom", "diagonal"])?;
    let mut inner = String::new();
    for side in ["left", "right", "top", "bottom", "diagonal"] {
        let node = if let Some(side_value) = obj.get(side) {
            let spec = side_value
                .as_object()
                .ok_or_else(|| error("Border side must be an object"))?;
            reject_unknown(spec, &["style", "color"])?;
            let style = spec
                .get("style")
                .and_then(Value::as_str)
                .ok_or_else(|| error("Border side requires a style"))?;
            if !matches!(
                style,
                "none"
                    | "thin"
                    | "medium"
                    | "dashed"
                    | "dotted"
                    | "thick"
                    | "double"
                    | "hair"
                    | "mediumDashed"
                    | "dashDot"
                    | "mediumDashDot"
                    | "dashDotDot"
                    | "mediumDashDotDot"
                    | "slantDashDot"
            ) {
                return Err(error("Unsupported border style"));
            }
            let color = spec
                .get("color")
                .map(|v| color_node(p, "color", v))
                .transpose()?
                .unwrap_or_default();
            format!("<{p}{side} style=\"{style}\">{color}</{p}{side}>")
        } else {
            format!("<{p}{side}/>")
        };
        inner.push_str(&node);
    }
    intern(
        xml,
        "borders",
        "border",
        &format!("<{p}border>{inner}</{p}border>"),
        items,
    )
}
fn read_alignment(value: &Value) -> Result<String, FillError> {
    let obj = value
        .as_object()
        .ok_or_else(|| error("Alignment must be an object"))?;
    reject_unknown(
        obj,
        &[
            "horizontal",
            "vertical",
            "wrapText",
            "shrinkToFit",
            "textRotation",
            "indent",
            "relativeIndent",
            "justifyLastLine",
            "readingOrder",
        ],
    )?;
    let mut attrs = Vec::new();
    for name in ["horizontal", "vertical"] {
        if let Some(v) = obj.get(name) {
            let v = v
                .as_str()
                .ok_or_else(|| error("Alignment direction must be a string"))?;
            let valid = if name == "horizontal" {
                matches!(
                    v,
                    "general"
                        | "left"
                        | "center"
                        | "right"
                        | "fill"
                        | "justify"
                        | "centerContinuous"
                        | "distributed"
                )
            } else {
                matches!(v, "top" | "center" | "bottom" | "justify" | "distributed")
            };
            if !valid {
                return Err(error("Unsupported alignment value"));
            }
            attrs.push(format!("{name}=\"{v}\""));
        }
    }
    for name in ["wrapText", "shrinkToFit", "justifyLastLine"] {
        if let Some(v) = obj.get(name) {
            attrs.push(format!("{name}=\"{}\"", bool_value(v)?));
        }
    }
    for name in ["indent", "relativeIndent", "readingOrder", "textRotation"] {
        if let Some(v) = obj.get(name) {
            let n = v
                .as_u64()
                .filter(|n| *n <= 255)
                .ok_or_else(|| error(format!("{name} must be an integer from 0 to 255")))?;
            attrs.push(format!("{name}=\"{n}\""));
        }
    }
    Ok(format!("<alignment {}/>", attrs.join(" ")))
}
fn make_xf(
    _p: &str,
    base: &str,
    num: Option<u32>,
    font: Option<u32>,
    fill: Option<u32>,
    border: Option<u32>,
    alignment: Option<&str>,
) -> Result<String, FillError> {
    let mut xf = base.to_string();
    for (name, id, apply) in [
        ("numFmtId", num, "applyNumberFormat"),
        ("fontId", font, "applyFont"),
        ("fillId", fill, "applyFill"),
        ("borderId", border, "applyBorder"),
    ] {
        if let Some(id) = id {
            xf = crate::properties::set_attribute(&xf, name, &id.to_string());
            xf = crate::properties::set_attribute(&xf, apply, "1");
        }
    }
    if let Some(alignment) = alignment {
        let all = elements(&xf)?;
        if let Some(old) = all.iter().find(|e| local_name(&e.name) == "alignment") {
            xf.replace_range(old.start..old.end, alignment);
        } else {
            let _root = all.first().ok_or_else(|| error("Invalid cell format"))?;
            xf = package_util::append(&xf, "xf", alignment)?;
        }
    }
    Ok(xf)
}
fn intern(
    xml: &mut String,
    container: &str,
    item: &str,
    new: &str,
    items: &mut Vec<String>,
) -> Result<u32, FillError> {
    if let Some(i) = items.iter().position(|old| old == new) {
        return Ok(i as u32);
    }
    let all = elements(xml)?;
    let parent = all
        .iter()
        .find(|e| local_name(&e.name) == container)
        .ok_or_else(|| error(format!("Stylesheet has no {container}")))?;
    let _ = parent;
    *xml = package_util::append(xml, container, new)?;
    *xml = set_count(xml, container)?;
    let index = items.len() as u32;
    items.push(new.into());
    let _ = item;
    Ok(index)
}
fn deduplicate(
    xml: &mut String,
    container: &str,
    item: &str,
    new: &str,
    items: &mut Vec<String>,
) -> Result<u32, FillError> {
    intern(xml, container, item, new, items)
}
fn set_count(xml: &str, container: &str) -> Result<String, FillError> {
    let all = elements(xml)?;
    let parent = all
        .iter()
        .find(|e| local_name(&e.name) == container)
        .ok_or_else(|| error(format!("Stylesheet has no {container}")))?;
    let children = children(&all, parent);
    let count = children.len();
    let open = &xml[parent.start..parent.open_end];
    let new = crate::properties::set_attribute(open, "count", &count.to_string());
    let mut updated = xml.to_string();
    updated.replace_range(parent.start..parent.open_end, &new);
    Ok(updated)
}
fn color_node(p: &str, name: &str, value: &Value) -> Result<String, FillError> {
    let text = value
        .as_str()
        .ok_or_else(|| error("Color must be a hex string"))?;
    let raw = text.trim_start_matches('#');
    if !(raw.len() == 6 || raw.len() == 8) || !raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(error("Color must be six or eight hexadecimal digits"));
    }
    let rgb = if raw.len() == 6 {
        format!("FF{}", raw.to_ascii_uppercase())
    } else {
        raw.to_ascii_uppercase()
    };
    Ok(format!("<{p}{name} rgb=\"{rgb}\"/>"))
}
fn bool_value(value: &Value) -> Result<&'static str, FillError> {
    value
        .as_bool()
        .map(|v| if v { "1" } else { "0" })
        .ok_or_else(|| error("Style flag must be a boolean"))
}
fn reject_unknown(map: &serde_json::Map<String, Value>, allowed: &[&str]) -> Result<(), FillError> {
    if let Some(key) = map.keys().find(|key| !allowed.contains(&key.as_str())) {
        Err(error(format!("Unsupported style property {key:?}")))
    } else {
        Ok(())
    }
}
fn set_cell_styles(sheet: &str, styles: &[(String, u32)]) -> Result<String, FillError> {
    let all = elements(sheet)?;
    let mut wanted: HashMap<&str, u32> = styles
        .iter()
        .map(|(cell, id)| (cell.as_str(), *id))
        .collect();
    let mut edits = Vec::new();
    for node in all.iter().filter(|e| local_name(&e.name) == "c") {
        let raw = &sheet[node.start..node.open_end];
        if let Some(cell) = read_attribute(raw, "r").filter(|cell| wanted.contains_key(cell)) {
            let id = wanted.remove(cell).expect("style map");
            edits.push((
                node.start,
                node.open_end,
                crate::properties::set_attribute(raw, "s", &id.to_string()),
            ));
        }
    }
    if let Some(cell) = wanted.keys().next() {
        return Err(cell_error(cell, "Style target cell is missing"));
    }
    let mut updated = sheet.to_string();
    edits.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
    for (start, end, text) in edits {
        updated.replace_range(start..end, &text);
    }
    Ok(updated)
}
fn cell_style_id(sheet: &str, cell: &str) -> Option<u32> {
    elements(sheet)
        .ok()?
        .into_iter()
        .find(|e| {
            local_name(&e.name) == "c"
                && read_attribute(&sheet[e.start..e.open_end], "r") == Some(cell)
        })
        .and_then(|e| read_attribute(&sheet[e.start..e.open_end], "s").and_then(|v| v.parse().ok()))
        .or(Some(0))
}
fn cell_error(cell: &str, message: impl Into<String>) -> FillError {
    FillError::new(format!("Cell {cell}: {}", message.into())).with_id(FillErrorId::SpreadsheetCell)
}
fn error(message: impl Into<String>) -> FillError {
    FillError::new(message).with_id(FillErrorId::SpreadsheetPackage)
}
