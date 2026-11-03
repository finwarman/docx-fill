//! SpreadsheetML templates. Package I/O stays with the caller, as it does for DOCX.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::properties::set_attribute;
use crate::xml::{read_attribute, unescape};
use crate::{FillError, FillErrorId, FillHooks, FillOptions, NoHooks, PackageParts};

mod cells;
mod references;
mod sheet;
mod text;
mod xmlutil;

use xmlutil::{elements, local_name, prefix};

const SHEET_NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const REL_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const OFFICE_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const TYPES_NS: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
const MAX_PARTS: usize = 10_000;
const MAX_BYTES: usize = 256 * 1024 * 1024;
const MAX_XML: usize = 16 * 1024 * 1024;

pub(super) fn error(message: impl Into<String>) -> FillError {
    FillError::new(message).with_id(FillErrorId::SpreadsheetPackage)
}

/// A worksheet's display name and package part path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkbookSheet {
    pub name: String,
    pub part: String,
}

/// A reusable XLSX/XLTX template. Unchanged package parts are borrowed during rendering.
///
/// Unlike the infallible DOCX renderer, spreadsheet rendering returns errors for unsupported
/// structural edits rather than returning a workbook with stale references.
#[derive(Debug)]
pub struct WorkbookTemplate {
    parts: Vec<(String, Vec<u8>)>,
    workbook: String,
    relationships: String,
    shared_strings: Option<String>,
    sheets: Vec<(WorkbookSheet, sheet::Sheet)>,
    row_numbers: Vec<Vec<u32>>,
    calc_chains: BTreeSet<String>,
}

#[derive(Debug)]
struct Relationship { id: String, kind: String, target: String, external: bool }

impl WorkbookTemplate {
    /// Parses an unpacked workbook using the existing delimiter, expression and hook options.
    /// The input must contain its content types and root/workbook relationships.
    pub fn parse(parts: impl IntoIterator<Item = (String, Vec<u8>)>, options: &FillOptions) -> Result<Self, FillError> {
        crate::template::validate_tag_prefixes(options)?;
        let parts: Vec<_> = parts.into_iter().collect();
        let mut names = BTreeSet::new();
        let mut total = 0usize;
        if parts.len() > MAX_PARTS { return Err(error("Workbook has more than 10,000 parts")); }
        for (name, bytes) in &parts {
            if !valid_path(name) || !names.insert(name.as_str()) { return Err(error(format!("Invalid or duplicate workbook part: {name}"))); }
            total = total.checked_add(bytes.len()).ok_or_else(|| error("Workbook size overflow"))?;
            if total > MAX_BYTES { return Err(error("Unpacked workbook exceeds 256 MiB")); }
            if name.starts_with("_xmlsignatures/") { return Err(error("Signed workbooks cannot be filled")); }
        }
        let sources: BTreeMap<_, _> = parts.iter().map(|(name, bytes)| (name.as_str(), bytes.as_slice())).collect();
        let root_rels = read_xml(&sources, "_rels/.rels")?;
        let roots = relationships(root_rels)?;
        let office: Vec<_> = roots.iter().filter(|rel| rel.kind == format!("{OFFICE_NS}/officeDocument")).collect();
        let [office] = office.as_slice() else { return Err(error("Expected one officeDocument relationship")); };
        if office.external { return Err(error("The workbook relationship must be internal")); }
        let workbook = resolve("", &office.target)?;
        let workbook_xml = read_xml(&sources, &workbook)?;
        let wb_root = check_root(workbook_xml, "workbook", SHEET_NS)?;
        let types = read_xml(&sources, "[Content_Types].xml")?;
        check_root(types, "Types", TYPES_NS)?;
        let mut overrides = BTreeMap::new();
        for item in elements(types)?.into_iter().filter(|e| local_name(&e.name) == "Override") {
            let raw = &types[item.start..item.open_end];
            let name = unescape(required(raw, "PartName")?);
            if overrides.insert(name, unescape(required(raw, "ContentType")?)).is_some() { return Err(error("Duplicate content type override")); }
        }
        let content_type = overrides.get(&format!("/{workbook}")).map(String::as_str);
        if !matches!(content_type, Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml" | "application/vnd.openxmlformats-officedocument.spreadsheetml.template.main+xml")) {
            return Err(error("Only standard XLSX and XLTX workbook content types are supported"));
        }
        let relationships_path = relation_path(&workbook);
        let rels = relationships(read_xml(&sources, &relationships_path)?)?;
        let rels_by_id: BTreeMap<_, _> = rels.iter().map(|rel| (rel.id.as_str(), rel)).collect();
        let shared_rels: Vec<_> = rels.iter().filter(|rel| rel.kind == format!("{OFFICE_NS}/sharedStrings")).collect();
        if shared_rels.len() > 1 { return Err(error("Duplicate shared-string relationships")); }
        let shared_strings = shared_rels.first().map(|rel| {
            if rel.external { return Err(error("Shared strings must be internal")); }
            resolve(&workbook, &rel.target)
        }).transpose()?;
        let shared = if let Some(path) = &shared_strings {
            let xml = read_xml(&sources, path)?;
            check_root(xml, "sst", SHEET_NS)?;
            let all = elements(xml)?;
            direct_children(&all, &all[0]).into_iter().filter(|e| e.name == format!("{}si", prefix(&all[0].name))).map(|e| xml[e.start..e.end].to_string()).collect()
        } else { Vec::new() };
        let mut sheets = Vec::new();
        let mut row_numbers = Vec::new();
        let mut sheet_names = BTreeSet::new();
        let mut sheet_ids = BTreeSet::new();
        let mut sheet_paths = BTreeSet::new();
        let mut errors = Vec::new();
        let wb_elements = elements(workbook_xml)?;
        let sheet_containers: Vec<_> = direct_children(&wb_elements, &wb_elements[0]).into_iter().filter(|e| e.name == format!("{}sheets", prefix(&wb_elements[0].name))).collect();
        let [sheet_container] = sheet_containers.as_slice() else { return Err(error("Expected one workbook sheets collection")); };
        for item in direct_children(&wb_elements, sheet_container).into_iter().filter(|e| e.name == format!("{}sheet", prefix(&wb_elements[0].name))) {
            let raw = &workbook_xml[item.start..item.open_end];
            let name = unescape(required(raw, "name")?);
            let id = required(raw, "sheetId")?.parse::<u32>().ok().filter(|id| *id > 0).ok_or_else(|| error("Invalid worksheet ID"))?;
            if name.is_empty() || !sheet_names.insert(name.to_lowercase()) || !sheet_ids.insert(id) { return Err(error("Empty or duplicate worksheet name or ID")); }
            let rid = relationship_id(raw, wb_root)?;
            let rel = rels_by_id.get(rid.as_str()).ok_or_else(|| error(format!("Worksheet {name} has a missing relationship")))?;
            if rel.external { return Err(error("Worksheets must be internal")); }
            let path = resolve(&workbook, &rel.target)?;
            if !sheet_paths.insert(path.clone()) { return Err(error("Multiple sheets reference the same part")); }
            if rel.kind == format!("{OFFICE_NS}/chartsheet") { read_xml(&sources, &path)?; continue; }
            if rel.kind != format!("{OFFICE_NS}/worksheet") { return Err(error(format!("Unsupported sheet relationship for {name}"))); }
            if overrides.get(&format!("/{path}")).map(String::as_str) != Some("application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml") { return Err(error(format!("Missing worksheet content type: {path}"))); }
            let xml = read_xml(&sources, &path)?;
            check_root(xml, "worksheet", SHEET_NS)?;
            let sheet_elements = elements(xml)?;
            let row_prefix = prefix(&sheet_elements[0].name);
            let rows = direct_children(&sheet_elements, &sheet_elements[0]).into_iter().find(|e| e.name == format!("{row_prefix}sheetData"))
                .map(|data| direct_children(&sheet_elements, data).into_iter().filter(|e| e.name == format!("{row_prefix}row"))
                    .filter_map(|e| read_attribute(&xml[e.start..e.open_end], "r")?.parse().ok()).collect()).unwrap_or_default();
            match sheet::Sheet::parse(xml, &shared, options) {
                Ok(sheet) => { sheets.push((WorkbookSheet { name, part: path }, sheet)); row_numbers.push(rows); }
                Err(error) => errors.push(error.in_part(&path)),
            }
        }
        if !errors.is_empty() { return Err(FillError::many(errors)); }
        let mut calc_chains = BTreeSet::new();
        for rel in &rels {
            if rel.kind == format!("{OFFICE_NS}/calcChain") {
                if rel.external { return Err(error("Calculation chains must be internal")); }
                let path = resolve(&workbook, &rel.target)?;
                if !sources.contains_key(path.as_str()) { return Err(error("Missing calculation chain part")); }
                calc_chains.insert(path);
            }
        }
        Ok(Self { parts, workbook, relationships: relationships_path, shared_strings, sheets, row_numbers, calc_chains })
    }

    /// Worksheet names and package paths in workbook order. Chart sheets are preserved separately.
    pub fn sheets(&self) -> Vec<&WorkbookSheet> { self.sheets.iter().map(|(info, _)| info).collect() }

    /// Fills a workbook, rejecting invalid values and unsupported structural edits.
    pub fn render(&self, data: &Value) -> Result<PackageParts<'_>, FillError> { self.render_with(data, &mut NoHooks) }

    /// Fills with the same expression, missing-value and parser hooks used by DOCX templates.
    pub fn render_with(&self, data: &Value, hooks: &mut dyn FillHooks) -> Result<PackageParts<'_>, FillError> {
        let mut changed = BTreeMap::new();
        let mut errors = Vec::new();
        let mut structural = false;
        let mut row_maps = BTreeMap::new();
        for ((info, sheet), original_rows) in self.sheets.iter().zip(&self.row_numbers) {
            let result = (|| {
                let rendered = sheet.render(data, hooks, &info.part)?;
                row_maps.insert(info.part.clone(), rendered.row_map.clone());
                let resized = rendered.row_map.len() != original_rows.len() || rendered.row_map.iter().any(|(old, new)| old != new);
                structural |= resized;
                if resized {
                    for item in elements(&rendered.xml)? {
                        if matches!(local_name(&item.name), "drawing" | "legacyDrawing" | "tablePart" | "pivotTablePart" | "extLst" | "hyperlink" | "formula" | "formula1" | "formula2" | "rowBreaks" | "colBreaks") {
                            return Err(error("Row loops with drawings, tables, pivots, links, validation formulas, page breaks or worksheet extensions are not supported yet").with_id(FillErrorId::SpreadsheetReference));
                        }
                    }
                }
                let xml = if resized { references::update_sheet_with_rows(&rendered.xml, &rendered.row_map, original_rows)? } else { rendered.xml };
                let original = self.part(&info.part);
                if xml.as_bytes() != original { changed.insert(info.part.clone(), xml.into_bytes()); }
                Ok(())
            })();
            if let Err(error) = result { errors.push(error.in_part(&info.part)); }
        }
        if !errors.is_empty() { return Err(FillError::many(errors)); }
        if changed.is_empty() { return Ok(self.parts.iter().map(|(name, bytes)| (name.clone(), Cow::Borrowed(bytes.as_slice()))).collect()); }
        if structural {
            let xml = std::str::from_utf8(self.part(&self.workbook)).expect("validated workbook XML");
            if elements(xml)?.iter().any(|e| local_name(&e.name) == "definedName") {
                return Err(error("Row loops in workbooks with defined names require named-range support").with_id(FillErrorId::SpreadsheetReference).in_part(&self.workbook));
            }
            for (info, _) in &self.sheets {
                let xml = changed.get(&info.part).map(Vec::as_slice).unwrap_or_else(|| self.part(&info.part));
                let xml = std::str::from_utf8(xml).expect("validated worksheet XML");
                for element in elements(xml)?.into_iter().filter(|e| local_name(&e.name) == "f") {
                    if xml[element.open_end..element.close_start].contains('!') {
                        return Err(error("Cross-sheet formula references during row expansion are not supported yet").with_id(FillErrorId::SpreadsheetReference).in_part(&info.part));
                    }
                }
            }
        }
        // Existing formula results are stale after input cells change. Explicit results on
        // newly generated formula cells remain useful until the spreadsheet recalculates.
        for (info, _) in &self.sheets {
            let original = std::str::from_utf8(self.part(&info.part)).expect("validated worksheet XML");
            let rendered = changed.get(&info.part).map(Vec::as_slice).unwrap_or_else(|| self.part(&info.part));
            let rendered = std::str::from_utf8(rendered).expect("rendered worksheet XML");
            let updated = clear_formula_caches(original, rendered, &row_maps[&info.part])?;
            if updated != rendered { changed.insert(info.part.clone(), updated.into_bytes()); }
        }
        let workbook = std::str::from_utf8(self.part(&self.workbook)).expect("validated workbook XML");
        changed.insert(self.workbook.clone(), recalculate(workbook)?.into_bytes());
        if !self.calc_chains.is_empty() {
            let rels = std::str::from_utf8(self.part(&self.relationships)).expect("validated relationships");
            changed.insert(self.relationships.clone(), remove_elements(rels, |name, raw| local_name(name) == "Relationship" && read_attribute(raw, "Type") == Some(&format!("{OFFICE_NS}/calcChain")))?.into_bytes());
            let types = std::str::from_utf8(self.part("[Content_Types].xml")).expect("validated content types");
            changed.insert("[Content_Types].xml".into(), remove_elements(types, |name, raw| local_name(name) == "Override" && read_attribute(raw, "PartName").map(unescape).is_some_and(|path| self.calc_chains.contains(path.trim_start_matches('/'))))?.into_bytes());
        }
        if let Some(path) = &self.shared_strings {
            let xml = std::str::from_utf8(self.part(path)).expect("validated shared strings");
            let root = elements(xml)?.remove(0);
            let raw = &xml[root.start..root.open_end];
            let without_count = remove_attribute(raw, "count");
            let mut updated = xml.to_string(); updated.replace_range(root.start..root.open_end, &without_count);
            if updated != xml { changed.insert(path.clone(), updated.into_bytes()); }
        }
        let mut bytes = 0usize;
        for (name, original) in &self.parts {
            bytes = bytes.checked_add(changed.get(name).unwrap_or(original).len()).ok_or_else(|| error("Rendered workbook size overflow"))?;
            if bytes > MAX_BYTES { return Err(error("Rendered workbook exceeds 256 MiB")); }
        }
        Ok(self.parts.iter().filter(|(name, _)| !self.calc_chains.contains(name)).map(|(name, bytes)| {
            (name.clone(), changed.remove(name).map_or_else(|| Cow::Borrowed(bytes.as_slice()), Cow::Owned))
        }).collect())
    }

    fn part(&self, name: &str) -> &[u8] { &self.parts.iter().find(|(path, _)| path == name).expect("validated workbook part").1 }
}

fn valid_path(path: &str) -> bool {
    !path.is_empty() && !path.starts_with('/') && !path.ends_with('/') && !path.contains(['\\', ':', '?', '#']) && path.split('/').all(|segment| !matches!(segment, "" | "." | ".."))
}

fn read_xml<'a>(sources: &BTreeMap<&str, &'a [u8]>, path: &str) -> Result<&'a str, FillError> {
    let bytes = sources.get(path).ok_or_else(|| error(format!("Missing workbook part: {path}")))?;
    if bytes.len() > MAX_XML { return Err(error("Spreadsheet XML exceeds 16 MiB").in_part(path)); }
    std::str::from_utf8(bytes).map_err(|_| error("Spreadsheet XML is not UTF-8").in_part(path))
}

fn required<'a>(raw: &'a str, name: &str) -> Result<&'a str, FillError> {
    read_attribute(raw, name).ok_or_else(|| error(format!("Missing spreadsheet attribute: {name}")))
}

fn check_root<'a>(xml: &'a str, expected: &str, namespace: &str) -> Result<&'a str, FillError> {
    let all = elements(xml)?;
    let root = all.first().ok_or_else(|| error("Empty spreadsheet XML"))?;
    let raw = &xml[root.start..root.open_end];
    let attr = if prefix(&root.name).is_empty() { "xmlns".into() } else { format!("xmlns:{}", prefix(&root.name).trim_end_matches(':')) };
    if local_name(&root.name) != expected || read_attribute(raw, &attr) != Some(namespace) { return Err(error(format!("Expected {expected} in the standard OOXML namespace"))); }
    for outer in [&xml[..root.start], &xml[root.end..]] {
        for token in crate::xml::tokenize(outer).map_err(error)? {
            match token {
                crate::xml::Token::Text(text) if text.trim().is_empty() => {},
                crate::xml::Token::Element { kind: crate::xml::ElementKind::Other, raw, .. } if raw.starts_with("<?") || raw.starts_with("<!--") => {},
                _ => return Err(error("Unexpected content outside spreadsheet XML root")),
            }
        }
    }
    if xml.contains("<!DOCTYPE") || xml.contains("<!ENTITY") { return Err(error("DTD declarations are not supported in spreadsheet XML")); }
    Ok(raw)
}

fn direct_children<'a>(all: &'a [xmlutil::Element], parent: &xmlutil::Element) -> Vec<&'a xmlutil::Element> {
    let mut children = Vec::new();
    let mut skip = parent.open_end;
    for item in all.iter().filter(|e| e.start >= parent.open_end && e.end <= parent.close_start) {
        if item.start < skip { continue; }
        children.push(item);
        skip = item.end;
    }
    children
}

fn relationships(xml: &str) -> Result<Vec<Relationship>, FillError> {
    check_root(xml, "Relationships", REL_NS)?;
    let mut ids = BTreeSet::new();
    let mut result = Vec::new();
    for item in elements(xml)?.into_iter().filter(|e| local_name(&e.name) == "Relationship") {
        let raw = &xml[item.start..item.open_end];
        let id = unescape(required(raw, "Id")?);
        if id.is_empty() || !ids.insert(id.clone()) { return Err(error("Empty or duplicate relationship ID")); }
        let mode = read_attribute(raw, "TargetMode");
        if mode.is_some_and(|mode| mode != "External" && mode != "Internal") { return Err(error("Invalid relationship TargetMode")); }
        result.push(Relationship { id, kind: unescape(required(raw, "Type")?), target: unescape(required(raw, "Target")?), external: mode == Some("External") });
    }
    Ok(result)
}

fn relationship_id(raw: &str, root: &str) -> Result<String, FillError> {
    // The relationship namespace can use any prefix declared on the root or sheet.
    for text in [raw, root] {
        for fragment in text.split_whitespace() {
            let Some(rest) = fragment.strip_prefix("xmlns:") else { continue };
            let ns_prefix = rest.split(['=', ' ', '\t']).next().unwrap_or_default();
            let attr = format!("xmlns:{ns_prefix}");
            if read_attribute(raw, &attr).or_else(|| read_attribute(root, &attr)) == Some(OFFICE_NS)
                && let Some(value) = read_attribute(raw, &format!("{ns_prefix}:id")) { return Ok(unescape(value)); }
        }
    }
    Err(error("Worksheet has no office relationship ID"))
}

fn relation_path(source: &str) -> String {
    match source.rsplit_once('/') { Some((folder, file)) => format!("{folder}/_rels/{file}.rels"), None => format!("_rels/{source}.rels") }
}

fn resolve(source: &str, target: &str) -> Result<String, FillError> {
    if target.is_empty() || target.contains(['\\', ':', '?', '#', '%']) { return Err(error("Unsupported or unsafe relationship target")); }
    let mut segments: Vec<_> = if target.starts_with('/') { Vec::new() } else { source.rsplit_once('/').map_or(Vec::new(), |(folder, _)| folder.split('/').collect()) };
    for segment in target.trim_start_matches('/').split('/') {
        match segment {
            "" | "." => {},
            ".." => { if segments.pop().is_none() { return Err(error("Relationship target escapes the package")); } },
            _ => segments.push(segment),
        }
    }
    let path = segments.join("/");
    if !valid_path(&path) { return Err(error("Invalid relationship target")); }
    Ok(path)
}

fn recalculate(xml: &str) -> Result<String, FillError> {
    let all = elements(xml)?;
    let root = all.first().ok_or_else(|| error("Missing workbook XML root"))?;
    let tag = all.iter().find(|item| local_name(&item.name) == "calcPr");
    let mut out = xml.to_string();
    if let Some(item) = tag {
        let mut raw = xml[item.start..item.open_end].to_string();
        for (name, value) in [("calcMode", "auto"), ("fullCalcOnLoad", "1"), ("forceFullCalc", "1")] { raw = set_attribute(&raw, name, value); }
        out.replace_range(item.start..item.open_end, &raw);
    } else {
        let at = all.iter().find(|item| matches!(local_name(&item.name), "oleSize" | "customWorkbookViews" | "pivotCaches" | "smartTagPr" | "smartTagTypes" | "webPublishing" | "fileRecoveryPr" | "webPublishObjects" | "extLst")).map_or(root.close_start, |item| item.start);
        out.insert_str(at, &format!("<{}calcPr calcMode=\"auto\" fullCalcOnLoad=\"1\" forceFullCalc=\"1\"/>", prefix(&root.name)));
    }
    Ok(out)
}

fn remove_elements(xml: &str, mut remove: impl FnMut(&str, &str) -> bool) -> Result<String, FillError> {
    let mut out = xml.to_string();
    for item in elements(xml)?.into_iter().rev() {
        if remove(&item.name, &xml[item.start..item.open_end]) { out.replace_range(item.start..item.end, ""); }
    }
    Ok(out)
}

fn remove_attribute(raw: &str, name: &str) -> String {
    let Some(value) = read_attribute(raw, name) else { return raw.to_string() };
    let value_at = value.as_ptr() as usize - raw.as_ptr() as usize;
    let start = raw[..value_at - 1].rfind(name).unwrap_or(value_at - 1);
    let mut out = raw.to_string();
    out.replace_range(start..value_at + value.len() + 1, "");
    out
}

fn clear_formula_caches(original: &str, rendered: &str, row_map: &[(u32, u32)]) -> Result<String, FillError> {
    let mut formula_cells = BTreeSet::new();
    for cell in elements(original)?.into_iter().filter(|e| local_name(&e.name) == "c") {
        let inner = &original[cell.open_end..cell.close_start];
        if elements(inner)?.iter().any(|e| local_name(&e.name) == "f")
            && let Some(reference) = read_attribute(&original[cell.start..cell.open_end], "r") { formula_cells.insert(reference.to_string()); }
    }
    let sources: BTreeMap<_, _> = row_map.iter().map(|(old, new)| (*new, *old)).collect();
    let mut out = rendered.to_string();
    for cell in elements(rendered)?.into_iter().filter(|e| local_name(&e.name) == "c").rev() {
        let Some(reference) = read_attribute(&rendered[cell.start..cell.open_end], "r") else { continue };
        let digit = reference.find(|c: char| c.is_ascii_digit()).unwrap_or(reference.len());
        let row = reference[digit..].parse::<u32>().unwrap_or(0);
        let source = sources.get(&row).copied().unwrap_or(row);
        if formula_cells.contains(&format!("{}{source}", &reference[..digit])) {
            let inner = &rendered[cell.open_end..cell.close_start];
            let updated = remove_elements(inner, |name, _| local_name(name) == "v")?;
            out.replace_range(cell.open_end..cell.close_start, &updated);
        }
    }
    Ok(out)
}
