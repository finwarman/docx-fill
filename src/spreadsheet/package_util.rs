//! Package edits shared by spreadsheet content modules.
use super::xmlutil::{elements, escaped, local_name, prefix};
use crate::xml::{read_attribute, unescape};
use crate::{FillError, FillErrorId};
use std::collections::BTreeMap;

pub(super) const REL_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
pub(super) const OFFICE_NS: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
pub(super) const SHEET_NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
pub(super) type Parts = BTreeMap<String, Vec<u8>>;

pub(super) fn error(message: impl Into<String>) -> FillError {
    super::error(message).with_id(FillErrorId::SpreadsheetPackage)
}
pub(super) fn xml<'a>(parts: &'a Parts, path: &str) -> Result<&'a str, FillError> {
    std::str::from_utf8(
        parts
            .get(path)
            .ok_or_else(|| error(format!("Missing spreadsheet part: {path}")))?,
    )
    .map_err(|_| error(format!("Invalid UTF-8 in {path}")))
}
pub(super) fn rel_path(source: &str) -> String {
    match source.rsplit_once('/') {
        Some((dir, file)) => format!("{dir}/_rels/{file}.rels"),
        None => format!("_rels/{source}.rels"),
    }
}
pub(super) fn relative(source: &str, target: &str) -> String {
    let mut from: Vec<_> = source.split('/').collect();
    from.pop();
    let mut to: Vec<_> = target.split('/').collect();
    while from.first() == to.first() && !from.is_empty() {
        from.remove(0);
        to.remove(0);
    }
    format!("{}{}", "../".repeat(from.len()), to.join("/"))
}
pub(super) fn resolve(source: &str, target: &str) -> Result<String, FillError> {
    if target.contains(['\\', ':', '?', '#']) {
        return Err(error("Unsafe relationship target"));
    }
    let mut path: Vec<_> = if target.starts_with('/') {
        Vec::new()
    } else {
        source
            .rsplit_once('/')
            .map_or(Vec::new(), |(dir, _)| dir.split('/').collect())
    };
    for item in target.trim_start_matches('/').split('/') {
        match item {
            "" | "." => {}
            ".." => {
                path.pop()
                    .ok_or_else(|| error("Relationship escapes package"))?;
            }
            _ => path.push(item),
        }
    }
    if path.is_empty() {
        return Err(error("Empty relationship target"));
    }
    Ok(path.join("/"))
}
pub(super) fn append(xml: &str, parent: &str, child: &str) -> Result<String, FillError> {
    let all = elements(xml)?;
    let node = all
        .iter()
        .find(|e| local_name(&e.name) == parent)
        .ok_or_else(|| error(format!("Missing {parent}")))?;
    let mut out = xml.to_string();
    if xml[node.start..node.open_end].ends_with("/>") {
        let raw = &xml[node.start..node.open_end - 2];
        out.replace_range(
            node.start..node.end,
            &format!("{raw}>{child}</{}>", node.name),
        );
    } else {
        out.insert_str(node.close_start, child);
    }
    Ok(out)
}
pub(super) fn type_override(parts: &mut Parts, path: &str, mime: &str) -> Result<(), FillError> {
    let types = xml(parts, "[Content_Types].xml")?;
    for node in elements(types)?
        .iter()
        .filter(|e| local_name(&e.name) == "Override")
    {
        if read_attribute(&types[node.start..node.open_end], "PartName")
            .map(unescape)
            .as_deref()
            == Some(&format!("/{path}"))
        {
            return Ok(());
        }
    }
    let root = elements(types)?.remove(0);
    let new = format!(
        "<{}Override PartName=\"/{}\" ContentType=\"{}\"/>",
        prefix(&root.name),
        escaped(path),
        escaped(mime)
    );
    let types = append(types, "Types", &new)?;
    parts.insert("[Content_Types].xml".into(), types.into_bytes());
    Ok(())
}
pub(super) fn add_relation(
    parts: &mut Parts,
    source: &str,
    kind: &str,
    target: &str,
    external: bool,
) -> Result<String, FillError> {
    let path = rel_path(source);
    let rels = parts
        .get(&path)
        .map(|v| std::str::from_utf8(v))
        .transpose()
        .map_err(|_| error("Invalid relationship XML"))?
        .map(str::to_string)
        .unwrap_or_else(|| format!("<Relationships xmlns=\"{REL_NS}\"/>"));
    let all = elements(&rels)?;
    let ids: std::collections::BTreeSet<_> = all
        .iter()
        .filter_map(|e| read_attribute(&rels[e.start..e.open_end], "Id"))
        .collect();
    let id = (1..)
        .map(|n| format!("rIdDocxFill{n}"))
        .find(|id| !ids.contains(id.as_str()))
        .expect("finite relationships");
    let p = all.first().map_or("", |e| prefix(&e.name));
    let node = format!(
        "<{p}Relationship Id=\"{id}\" Type=\"{}\" Target=\"{}\"{}/>",
        escaped(kind),
        escaped(target),
        if external {
            " TargetMode=\"External\""
        } else {
            ""
        }
    );
    parts.insert(path, append(&rels, "Relationships", &node)?.into_bytes());
    Ok(id)
}
pub(super) fn targets(
    parts: &Parts,
    source: &str,
    kind: &str,
) -> Result<Vec<(String, String)>, FillError> {
    let path = rel_path(source);
    let Some(bytes) = parts.get(&path) else {
        return Ok(Vec::new());
    };
    let rels = std::str::from_utf8(bytes).map_err(|_| error("Invalid relationships"))?;
    elements(rels)?
        .iter()
        .filter(|e| local_name(&e.name) == "Relationship")
        .filter_map(|e| {
            let raw = &rels[e.start..e.open_end];
            (read_attribute(raw, "Type") == Some(kind)).then_some(raw)
        })
        .map(|raw| {
            if read_attribute(raw, "TargetMode") == Some("External") {
                return Err(error("External drawing dependencies cannot be edited"));
            }
            let target = unescape(
                read_attribute(raw, "Target")
                    .ok_or_else(|| error("Missing relationship target"))?,
            );
            let id = read_attribute(raw, "Id")
                .ok_or_else(|| error("Missing relationship ID"))?
                .to_string();
            Ok((id, resolve(source, &target)?))
        })
        .collect()
}
pub(super) fn unique_path(parts: &Parts, directory: &str, stem: &str, extension: &str) -> String {
    (1..)
        .map(|n| format!("{directory}/{stem}{n}.{extension}"))
        .find(|path| !parts.contains_key(path))
        .expect("finite package")
}
pub(super) fn insert_sheet_child(
    xml: &str,
    child: &str,
    before: &[&str],
) -> Result<String, FillError> {
    let all = elements(xml)?;
    let root = all.first().ok_or_else(|| error("Missing worksheet root"))?;
    if xml[root.start..root.open_end].ends_with("/>") {
        return append(xml, "worksheet", child);
    }
    let at = all
        .iter()
        .filter(|e| before.contains(&local_name(&e.name)))
        .map(|e| e.start)
        .min()
        .unwrap_or(root.close_start);
    let mut out = xml.to_string();
    out.insert_str(at, child);
    Ok(out)
}
