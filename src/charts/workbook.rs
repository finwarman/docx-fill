use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read, Write};

use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

use crate::properties::{find_element_end, set_attribute, set_child};
use crate::xml::{ElementKind, Token, check_balanced, escape, is_xml_char, read_attribute, tokenize, unescape};
use crate::{FillError, FillErrorId};

const CHART_EX_NS: &str = "http://schemas.microsoft.com/office/drawing/2014/chartex";
const SHEET_NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const REL_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const OFFICE_REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const TYPES_NS: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
const WORKBOOK_TYPE: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml";
const PACKAGE_TYPE: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
const CHART_ORDER: &[&str] = &["c:date1904", "c:lang", "c:roundedCorners", "mc:AlternateContent", "c:style", "c:clrMapOvr", "c:pivotSource", "c:protection", "c:chart", "c:spPr", "c:txPr", "c:externalData", "c:printSettings", "c:userShapes", "c:extLst"];
const SHEET_TYPE: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml";
const MAX_ARCHIVE_BYTES: usize = 64 * 1024 * 1024;
const MAX_UNPACKED_BYTES: u64 = 256 * 1024 * 1024;
const MAX_XML_BYTES: u64 = 16 * 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;
const MAX_ROWS: usize = 1_048_576;
const MAX_COLUMNS: usize = 16_384;
const MAX_CELLS: usize = 1_000_000;
const MAX_CELL_CHARS: usize = 32_767;

struct Updated {
    archive: Vec<u8>,
    chart: String,
}

pub(super) struct Prepared {
    pub(super) chart: String,
    pub(super) relations: Vec<u8>,
    pub(super) path: String,
    pub(super) bytes: Vec<u8>,
    pub(super) content_type: String,
}

pub(super) fn prepare(sources: &BTreeMap<&str, &[u8]>, source_path: &str, source: &str, filled: &str, chart_path: &str) -> Result<Prepared, FillError> {
    let external = one(source, "externalData")?;
    let raw = &source[external.start..external.open_end];
    let id = read_attribute(raw, "r:id").ok_or_else(|| error("The chart has no embedded workbook relationship"))?;
    let rels_path = super::relation_path(source_path);
    let relations = sources.get(rels_path.as_str()).and_then(|bytes| std::str::from_utf8(bytes).ok()).ok_or_else(|| error("Missing chart relationships"))?;
    root(relations, "Relationships", REL_NS)?;
    let matches: Vec<_> = elements(relations, "Relationship")?.into_iter().filter(|item| read_attribute(&relations[item.start..item.open_end], "Id") == Some(id)).collect();
    let [relationship] = &matches[..] else { return Err(error("Missing or duplicate embedded workbook relationship")); };
    let raw = &relations[relationship.start..relationship.open_end];
    if read_attribute(raw, "Type") != Some(&format!("{OFFICE_REL_NS}/package")) || read_attribute(raw, "TargetMode") == Some("External") { return Err(error("Chart data must use an internal embedded XLSX workbook")); }
    let folder = source_path.rsplit_once('/').map_or("", |(folder, _)| folder);
    let old_path = resolve(folder, read_attribute(raw, "Target").ok_or_else(|| error("The embedded workbook relationship has no target"))?)?;
    let workbook = sources.get(old_path.as_str()).ok_or_else(|| error("Missing embedded workbook part"))?;
    let updated = update(workbook, filled)?;
    let folder = old_path.rsplit_once('/').map_or("", |(folder, _)| folder);
    let path = (1..).map(|index| if folder.is_empty() { format!("docxFillWorkbook{index}.xlsx") } else { format!("{folder}/docxFillWorkbook{index}.xlsx") }).find(|path| !sources.contains_key(path.as_str())).expect("finite package parts");
    let target = escaped(&super::relative_target(chart_path, &path));
    let replacement = set_attribute(raw, "Target", &target);
    let mut relations = relations.to_string();
    relations.replace_range(relationship.start..relationship.open_end, &replacement);
    let modern = external.name == "cx:externalData";
    let root = one(&updated.chart, if modern { "chartData" } else { "chartSpace" })?;
    let inner = &updated.chart[root.open_end..root.close_start];
    let order: &[&str] = if modern { &["cx:externalData", "cx:data", "cx:extLst"] } else { CHART_ORDER };
    let inner = set_child(inner, external.name, &source[external.start..external.end], order);
    let chart = format!("{}{}{}", &updated.chart[..root.open_end], inner, &updated.chart[root.close_start..]);
    let content_type = format!("<Override PartName=\"/{}\" ContentType=\"{PACKAGE_TYPE}\"/>", escaped(&path));
    Ok(Prepared { chart, relations: relations.into_bytes(), path, bytes: updated.archive, content_type })
}

fn escaped(value: &str) -> String { let mut out = String::new(); escape(value, &mut out); out }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CellKind { Text, Number }

#[derive(Debug)]
struct Column {
    header: String,
    kind: CellKind,
    values: Vec<Option<String>>,
    shared: bool,
}

struct Element<'a> {
    name: &'a str,
    start: usize,
    open_end: usize,
    close_start: usize,
    end: usize,
}

fn error(message: impl Into<String>) -> FillError {
    FillError::new(message).with_id(FillErrorId::ChartWorkbook)
}

/// Add a data sheet to a copy of the workbook and point chart caches at that sheet.
fn update(bytes: &[u8], chart: &str) -> Result<Updated, FillError> {
    if bytes.len() > MAX_ARCHIVE_BYTES { return Err(error("The embedded workbook exceeds 64 MiB")); }
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|cause| error(format!("Cannot read the embedded workbook: {cause}")))?;
    let names = check_archive(&mut archive)?;
    let root_rels = read_xml(&mut archive, "_rels/.rels")?;
    let workbook_path = office_document(&root_rels)?;
    let mut workbook = read_xml(&mut archive, &workbook_path)?;
    let workbook_root = root(&workbook, "workbook", SHEET_NS)?;
    let wb_prefix = prefix(workbook_root.name).to_string();
    let rels_path = super::relation_path(&workbook_path);
    let mut rels = read_xml(&mut archive, &rels_path)?;
    root(&rels, "Relationships", REL_NS)?;
    let folder = workbook_path.rsplit_once('/').map_or("", |(folder, _)| folder);
    let sheet_paths = sheet_paths(&rels, folder)?;
    let mut types = read_xml(&mut archive, "[Content_Types].xml")?;
    root(&types, "Types", TYPES_NS)?;
    let workbook_type = elements(&types, "Override")?.into_iter().any(|item| {
        let raw = &types[item.start..item.open_end];
        read_attribute(raw, "PartName").map(unescape).as_deref() == Some(&format!("/{workbook_path}")) && read_attribute(raw, "ContentType") == Some(WORKBOOK_TYPE)
    });
    if !workbook_type { return Err(error("Only standard XLSX workbooks are supported")); }
    if elements(&workbook, "workbookProtection")?.iter().any(|item| read_attribute(&workbook[item.start..item.open_end], "lockStructure").is_some_and(|value| matches!(value, "1" | "true"))) { return Err(error("Cannot add chart data to a structure-protected workbook")); }
    let sheets = elements(&workbook, "sheet")?;
    let sheet_count = sheets.len();
    let mut sheet_names = BTreeSet::new();
    let mut sheet_ids = BTreeSet::new();
    for sheet in sheets {
        let raw = &workbook[sheet.start..sheet.open_end];
        let name = read_attribute(raw, "name").map(unescape).filter(|name| !name.is_empty()).ok_or_else(|| error("Missing worksheet name"))?;
        let id = read_attribute(raw, "sheetId").and_then(|id| id.parse::<u32>().ok()).filter(|id| *id > 0).ok_or_else(|| error("Invalid worksheet ID"))?;
        if !sheet_names.insert(name.to_lowercase()) || !sheet_ids.insert(id) { return Err(error("Duplicate worksheet name or ID")); }
    }
    let ids: BTreeSet<_> = elements(&rels, "Relationship")?.iter().filter_map(|item| read_attribute(&rels[item.start..item.open_end], "Id").map(str::to_string)).collect();
    let max_id = sheet_ids.last().copied().unwrap_or(0);
    let sheet_id = max_id.checked_add(1).ok_or_else(|| error("Workbook sheet IDs are exhausted"))?;
    let folder = workbook_path.rsplit_once('/').map_or("", |(folder, _)| folder);
    let (sheet_name, sheet_path, id) = (1..).find_map(|index| {
        let sheet = if index == 1 { "DocxFillData".to_string() } else { format!("DocxFillData{index}") };
        let path = if folder.is_empty() { format!("worksheets/docxFillData{index}.xml") } else { format!("{folder}/worksheets/docxFillData{index}.xml") };
        let id = format!("rIdDocxFillData{index}");
        (!sheet_names.contains(&sheet.to_lowercase()) && !names.contains(&path) && !ids.contains(&id)).then_some((sheet, path, id))
    }).expect("finite workbook entries");
    let (chart, columns) = if one(chart, "chartSpace")?.name == "cx:chartSpace" {
        extended_data(chart, &sheet_name)?
    } else { chart_data(chart, &sheet_name)? };
    let sheet_xml = write_sheet(&columns)?;
    let sheet = format!("<{wb_prefix}sheet name=\"{sheet_name}\" sheetId=\"{sheet_id}\" xmlns:r=\"{OFFICE_REL_NS}\" r:id=\"{id}\"/>");
    workbook = append_to(&workbook, "sheets", &sheet)?;
    let views: Vec<_> = elements(&workbook, "workbookView")?.into_iter().map(|view| (view.start, view.open_end)).collect();
    if views.is_empty() {
        let sheets = one(&workbook, "sheets")?;
        workbook.insert_str(sheets.start, &format!("<{wb_prefix}bookViews><{wb_prefix}workbookView activeTab=\"{sheet_count}\"/></{wb_prefix}bookViews>"));
    } else {
        for (start, end) in views.into_iter().rev() {
            let replacement = set_attribute(&workbook[start..end], "activeTab", &sheet_count.to_string());
            workbook.replace_range(start..end, &replacement);
        }
    }
    let rel_prefix = prefix(root(&rels, "Relationships", REL_NS)?.name);
    let target = super::relative_target(&workbook_path, &sheet_path);
    rels = append_to(&rels, "Relationships", &format!("<{rel_prefix}Relationship Id=\"{id}\" Type=\"{OFFICE_REL_NS}/worksheet\" Target=\"{target}\"/>"))?;
    let types_prefix = prefix(root(&types, "Types", TYPES_NS)?.name);
    types = append_to(&types, "Types", &format!("<{types_prefix}Override PartName=\"/{sheet_path}\" ContentType=\"{SHEET_TYPE}\"/>"))?;
    let mut changes = BTreeMap::from([(workbook_path, workbook), (rels_path, rels), ("[Content_Types].xml".into(), types), (sheet_path, sheet_xml)]);
    clear_selection(&mut archive, &mut changes, &sheet_paths)?;
    let mut output = ZipWriter::new(Cursor::new(Vec::new()));
    output.set_raw_comment(archive.comment().into()).map_err(|cause| error(cause.to_string()))?;
    for index in 0..archive.len() {
        let file = archive.by_index_raw(index).map_err(|cause| error(cause.to_string()))?;
        let name = file.name().map_err(|cause| error(cause.to_string()))?;
        if !changes.contains_key(name.as_ref()) { output.raw_copy_file(file).map_err(|cause| error(cause.to_string()))?; }
    }
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, xml) in changes {
        output.start_file(name, options).map_err(|cause| error(cause.to_string()))?;
        output.write_all(xml.as_bytes()).map_err(|cause| error(cause.to_string()))?;
    }
    let archive = output.finish().map_err(|cause| error(cause.to_string()))?.into_inner();
    if archive.len() > MAX_ARCHIVE_BYTES { return Err(error("The generated workbook exceeds 64 MiB")); }
    Ok(Updated { archive, chart })
}

fn check_archive(archive: &mut ZipArchive<Cursor<&[u8]>>) -> Result<BTreeSet<String>, FillError> {
    if archive.len() >= MAX_ENTRIES || archive.has_duplicate_names() { return Err(error("The embedded workbook has too many or duplicate ZIP entries")); }
    let mut total = 0u64;
    let mut names = BTreeSet::new();
    for index in 0..archive.len() {
        let file = archive.by_index_raw(index).map_err(|cause| error(cause.to_string()))?;
        total = total.checked_add(file.size()).ok_or_else(|| error("Workbook size overflow"))?;
        if total > MAX_UNPACKED_BYTES { return Err(error("The unpacked workbook exceeds 256 MiB")); }
        let name = file.name().map_err(|cause| error(cause.to_string()))?.into_owned();
        if name.starts_with('/') || name.contains('\\') || name.split('/').any(|part| matches!(part, "." | "..")) { return Err(error("The embedded workbook has an invalid ZIP path")); }
        names.insert(name);
    }
    if names.iter().any(|name| name.starts_with("_xmlsignatures/")) { return Err(error("Signed workbooks cannot be updated")); }
    Ok(names)
}

fn read_xml(archive: &mut ZipArchive<Cursor<&[u8]>>, path: &str) -> Result<String, FillError> {
    let file = archive.by_name(path).map_err(|cause| error(format!("Cannot read {path}: {cause}")))?;
    if file.size() > MAX_XML_BYTES { return Err(error(format!("Workbook XML exceeds 16 MiB: {path}"))); }
    let mut xml = String::new();
    file.take(MAX_XML_BYTES + 1).read_to_string(&mut xml).map_err(|cause| error(format!("Cannot read {path}: {cause}")))?;
    if xml.len() as u64 > MAX_XML_BYTES { return Err(error(format!("Workbook XML exceeds 16 MiB: {path}"))); }
    let tokens = tokenize(&xml).map_err(error)?;
    check_balanced(&tokens).map_err(error)?;
    Ok(xml)
}

fn office_document(rels: &str) -> Result<String, FillError> {
    root(rels, "Relationships", REL_NS)?;
    let found: Vec<_> = elements(rels, "Relationship")?.into_iter().filter_map(|item| {
        let raw = &rels[item.start..item.open_end];
        (read_attribute(raw, "Type") == Some(&format!("{OFFICE_REL_NS}/officeDocument"))).then_some(raw)
    }).collect();
    let [raw] = found[..] else { return Err(error("The embedded workbook must have one office-document relationship")); };
    if read_attribute(raw, "TargetMode") == Some("External") { return Err(error("External workbook targets are unsupported")); }
    resolve("", read_attribute(raw, "Target").ok_or_else(|| error("The workbook relationship has no target"))?)
}

fn resolve(folder: &str, target: &str) -> Result<String, FillError> {
    let target = unescape(target);
    if target.contains(['\\', ':', '#', '?']) { return Err(error("Unsupported workbook relationship target")); }
    super::resolve(folder, &target).filter(|path| !path.is_empty()).ok_or_else(|| error("Invalid workbook relationship target"))
}

fn prefix(name: &str) -> &str { name.rfind(':').map_or("", |at| &name[..=at]) }
fn local(name: &str) -> &str { name.rsplit(':').next().unwrap_or(name) }

fn elements<'a>(xml: &'a str, name: &str) -> Result<Vec<Element<'a>>, FillError> {
    let mut found = Vec::new();
    let mut at = 0;
    for token in tokenize(xml).map_err(error)? {
        let length = match token { Token::Text(raw) | Token::Element { raw, .. } => raw.len() };
        if let Token::Element { kind: ElementKind::Open | ElementKind::SelfClosing, name: actual, raw } = token
            && local(actual) == name {
            let end = find_element_end(xml, at, actual).ok_or_else(|| error(format!("Unclosed workbook element {actual}")))?;
            let close_start = if raw.ends_with("/>") { at + length } else { xml[..end].rfind('<').ok_or_else(|| error("Missing XML close tag"))? };
            found.push(Element { name: actual, start: at, open_end: at + length, close_start, end });
        }
        at += length;
    }
    Ok(found)
}

fn children<'a>(xml: &'a str, name: &str) -> Result<Vec<Element<'a>>, FillError> {
    let mut depth = 0usize;
    let mut at = 0;
    let mut found = Vec::new();
    for token in tokenize(xml).map_err(error)? {
        let length = match token { Token::Text(raw) | Token::Element { raw, .. } => raw.len() };
        if let Token::Element { kind, name: actual, raw } = token {
            if depth == 1 && kind != ElementKind::Close && local(actual) == name {
                let end = find_element_end(xml, at, actual).ok_or_else(|| error("Unclosed chart element"))?;
                let close_start = if raw.ends_with("/>") { at + length } else { xml[..end].rfind('<').ok_or_else(|| error("Missing XML close tag"))? };
                found.push(Element { name: actual, start: at, open_end: at + length, close_start, end });
            }
            match kind { ElementKind::Open => depth += 1, ElementKind::Close => depth = depth.saturating_sub(1), _ => {} }
        }
        at += length;
    }
    Ok(found)
}

fn one<'a>(xml: &'a str, name: &str) -> Result<Element<'a>, FillError> {
    let mut found = elements(xml, name)?;
    if found.len() != 1 { return Err(error(format!("Expected one workbook {name} element"))); }
    Ok(found.remove(0))
}

fn root<'a>(xml: &'a str, name: &str, namespace: &str) -> Result<Element<'a>, FillError> {
    if xml.contains("<!DOCTYPE") { return Err(error("Workbook XML cannot contain a document type")); }
    let tokens = tokenize(xml).map_err(error)?;
    check_balanced(&tokens).map_err(error)?;
    let mut depth = 0usize;
    let mut roots = Vec::new();
    for token in tokens {
        if let Token::Element { kind, name, .. } = token {
            if depth == 0 && matches!(kind, ElementKind::Open | ElementKind::SelfClosing) { roots.push(name); }
            match kind { ElementKind::Open => depth += 1, ElementKind::Close => depth = depth.saturating_sub(1), _ => {} }
        }
    }
    let [actual] = roots[..] else { return Err(error("Workbook XML must have one root")); };
    let found = one(xml, name)?;
    if actual != found.name { return Err(error(format!("Expected {name} at the XML root"))); }
    let key = if prefix(found.name).is_empty() { "xmlns".to_string() } else { format!("xmlns:{}", prefix(found.name).trim_end_matches(':')) };
    if read_attribute(&xml[found.start..found.open_end], &key) != Some(namespace) { return Err(error(format!("Unsupported {name} namespace"))); }
    Ok(found)
}

fn append_to(xml: &str, name: &str, child: &str) -> Result<String, FillError> {
    let item = one(xml, name)?;
    let mut out = xml.to_string();
    if xml[item.start..item.open_end].ends_with("/>") {
        let opening = &xml[item.start..item.open_end - 2];
        out.replace_range(item.start..item.end, &format!("{opening}>{child}</{}>", item.name));
    } else { out.insert_str(item.close_start, child); }
    Ok(out)
}

fn sheet_paths(rels: &str, folder: &str) -> Result<BTreeSet<String>, FillError> {
    let mut paths = BTreeSet::new();
    for item in elements(rels, "Relationship")? {
        let raw = &rels[item.start..item.open_end];
        if !matches!(read_attribute(raw, "Type"), Some(kind) if kind == format!("{OFFICE_REL_NS}/worksheet") || kind == format!("{OFFICE_REL_NS}/chartsheet")) { continue; }
        if read_attribute(raw, "TargetMode") == Some("External") { return Err(error("External worksheets are unsupported")); }
        let target = read_attribute(raw, "Target").ok_or_else(|| error("Missing worksheet target"))?;
        paths.insert(resolve(folder, target)?);
    }
    Ok(paths)
}

fn clear_selection(archive: &mut ZipArchive<Cursor<&[u8]>>, changes: &mut BTreeMap<String, String>, paths: &BTreeSet<String>) -> Result<(), FillError> {
    for path in paths {
        let xml = read_xml(archive, path)?;
        let mut edited = xml.clone();
        for view in elements(&xml, "sheetView")?.into_iter().rev() {
            if read_attribute(&xml[view.start..view.open_end], "tabSelected").is_some_and(|value| matches!(value, "1" | "true")) {
                let raw = set_attribute(&xml[view.start..view.open_end], "tabSelected", "0");
                edited.replace_range(view.start..view.open_end, &raw);
            }
        }
        if edited != xml { changes.insert(path.clone(), edited); }
    }
    Ok(())
}

fn chart_data(chart: &str, sheet: &str) -> Result<(String, Vec<Column>), FillError> {
    if chart.len() as u64 > MAX_XML_BYTES { return Err(error("Chart XML exceeds 16 MiB")); }
    let tokens = tokenize(chart).map_err(error)?;
    check_balanced(&tokens).map_err(error)?;
    // Series hidden by a filter sit in extLst and keep their workbook references.
    let extensions = elements(chart, "extLst")?;
    let series: Vec<_> = elements(chart, "ser")?.into_iter().filter(|item| !extensions.iter().any(|ext| ext.start < item.start && item.end <= ext.end)).collect();
    if series.is_empty() { return Err(error("Workbook updates require standard chart series")); }
    let mut columns: Vec<Column> = Vec::new();
    let mut replacements = Vec::new();
    let mut total = 0usize;
    for (index, series) in series.iter().enumerate() {
        let xml = &chart[series.start..series.end];
        let title = children(xml, "tx")?.first().and_then(|tx| super::element_text(&xml[tx.start..tx.end], "c:v"));
        let mut edits = Vec::new();
        let mut primary = None;
        for role in ["cat", "xVal", "val", "yVal", "bubbleSize"] {
            let spans = children(xml, role)?;
            if spans.is_empty() { continue; }
            if spans.len() != 1 { return Err(error("Multiple data ranges in one chart series are unsupported")); }
            let span = &spans[0];
            let content = &xml[span.open_end..span.close_start];
            let literals: Vec<_> = elements(content, "strLit")?.into_iter().chain(elements(content, "numLit")?).collect();
            let [literal] = &literals[..] else { return Err(error("Workbook updates require literal series data")); };
            let kind = if local(literal.name) == "strLit" { CellKind::Text } else { CellKind::Number };
            let values = read_points(&content[literal.start..literal.end], kind)?;
            total = total.checked_add(values.len()).filter(|total| *total <= MAX_CELLS).ok_or_else(|| error("Chart data exceeds one million cells"))?;
            let shared = role == "cat";
            let column = if let Some(column) = columns.iter().position(|column| shared && column.shared && column.kind == kind && column.values == values) {
                column
            } else {
                if columns.len() >= MAX_COLUMNS { return Err(error("Chart data exceeds the worksheet column limit")); }
                let header = match role { "cat" => "Category", "xVal" => "X", "bubbleSize" => "Size", _ => "Value" }.to_string();
                columns.push(Column { header, kind, values, shared }); columns.len() - 1
            };
            if matches!(role, "val" | "yVal") { primary = Some(column); }
            let count = columns[column].values.len();
            let column_name = column_name(column);
            let range = if count == 1 { format!("${column_name}$2") } else { format!("${column_name}$2:${column_name}${}", count + 1) };
            let data_kind = if kind == CellKind::Text { "str" } else { "num" };
            let cached = &content[literal.open_end..literal.close_start];
            let value = format!("<{}><c:{data_kind}Ref><c:f>'{sheet}'!{range}</c:f><c:{data_kind}Cache>{cached}</c:{data_kind}Cache></c:{data_kind}Ref></{}>", span.name, span.name);
            edits.push((span.start, span.end, value));
        }
        let primary = primary.ok_or_else(|| error("The chart series has no numeric value range"))?;
        columns[primary].header = title.clone().unwrap_or_else(|| format!("Series {}", index + 1));
        if let Some(title) = title {
            let tx = children(xml, "tx")?.remove(0);
            let mut escaped = String::new(); escape(&title, &mut escaped);
            let column = column_name(primary);
            edits.push((tx.start, tx.end, format!("<{}><c:strRef><c:f>'{sheet}'!${column}$1</c:f><c:strCache><c:ptCount val=\"1\"/><c:pt idx=\"0\"><c:v>{escaped}</c:v></c:pt></c:strCache></c:strRef></{}>", tx.name, tx.name)));
        }
        edits.sort_by_key(|edit| edit.0);
        let mut changed = xml.to_string();
        for (start, end, value) in edits.into_iter().rev() { changed.replace_range(start..end, &value); }
        replacements.push((series.start, series.end, changed));
    }
    let mut out = chart.to_string();
    for (start, end, changed) in replacements.into_iter().rev() { out.replace_range(start..end, &changed); }
    Ok((out, columns))
}

fn extended_data(chart: &str, sheet: &str) -> Result<(String, Vec<Column>), FillError> {
    root(chart, "chartSpace", CHART_EX_NS)?;
    let data = one(chart, "chartData")?;
    let data_xml = &chart[data.start..data.end];
    let mut columns = Vec::new();
    let mut edits = Vec::new();
    let mut values = BTreeMap::new();
    let mut total = 0usize;
    for item in children(data_xml, "data")? {
        let xml = &data_xml[item.start..item.end];
        let id = read_attribute(&xml[..item.open_end - item.start], "id").ok_or_else(|| error("Missing extended-chart data ID"))?;
        if values.contains_key(id) { return Err(error("Duplicate extended-chart data ID")); }
        let mut primary = None;
        for dimension in children(xml, "strDim")?.into_iter().chain(children(xml, "numDim")?) {
            let raw = &xml[dimension.start..dimension.end];
            let kind = if local(dimension.name) == "strDim" { CellKind::Text } else { CellKind::Number };
            let role = read_attribute(&raw[..dimension.open_end - dimension.start], "type").ok_or_else(|| error("Missing extended-chart dimension type"))?;
            let levels = children(raw, "lvl")?;
            if levels.is_empty() { return Err(error("Extended-chart dimensions need cached levels")); }
            if kind == CellKind::Number && levels.len() != 1 { return Err(error("Numeric extended-chart dimensions need one cached level")); }
            let first_column = columns.len();
            let mut count = None;
            // Office stores cached hierarchy levels leaf-first, opposite worksheet columns.
            for (index, level) in levels.iter().rev().enumerate() {
                let level_xml = &raw[level.start..level.end];
                let points = level_points(level_xml, kind)?;
                if count.is_some_and(|count| count != points.len()) { return Err(error("Hierarchy levels must have equal point counts")); }
                count = Some(points.len());
                total = total.checked_add(points.len()).filter(|total| *total <= MAX_CELLS).ok_or_else(|| error("Chart data exceeds one million cells"))?;
                if columns.len() >= MAX_COLUMNS { return Err(error("Chart data exceeds the worksheet column limit")); }
                let header = read_attribute(&level_xml[..level.open_end - level.start], "name").map(unescape).unwrap_or_else(|| format!("{role} {}", index + 1));
                columns.push(Column { header, kind, values: points, shared: false });
            }
            if kind == CellKind::Number && (primary.is_none() || role == "val") { primary = Some(first_column); }
            let last_column = columns.len() - 1;
            let first = column_name(first_column);
            let last = column_name(last_column);
            let count = count.expect("nonempty levels");
            let reference = if first_column == last_column && count == 1 { format!("${first}$2") } else { format!("${first}$2:${last}${}", count + 1) };
            let header = if first_column == last_column { format!("${first}$1") } else { format!("${first}$1:${last}$1") };
            let cached: String = levels.iter().map(|level| &raw[level.start..level.end]).collect();
            let replacement = format!("{}<cx:f dir=\"col\">'{sheet}'!{reference}</cx:f><cx:nf>'{sheet}'!{header}</cx:nf>{cached}</{}>", &raw[..dimension.open_end - dimension.start], dimension.name);
            edits.push((data.start + item.start + dimension.start, data.start + item.start + dimension.end, replacement));
        }
        values.insert(id.to_string(), primary.ok_or_else(|| error("Extended-chart data has no numeric dimension"))?);
    }
    if values.is_empty() { return Err(error("Extended chart has no cached data")); }
    for series in elements(chart, "series")? {
        if series.name != "cx:series" { continue; }
        let xml = &chart[series.start..series.end];
        let id = one(xml, "dataId")?;
        let id = read_attribute(&xml[id.start..id.open_end], "val").ok_or_else(|| error("Missing extended-chart series data ID"))?;
        let column = *values.get(id).ok_or_else(|| error("Unknown extended-chart series data ID"))?;
        let Some(tx) = children(xml, "tx")?.into_iter().next() else { continue };
        let tx_xml = &xml[tx.start..tx.end];
        let Some(text) = elements(tx_xml, "txData")?.into_iter().next() else { continue };
        let title = super::element_text(&tx_xml[text.start..text.end], "cx:v").ok_or_else(|| error("Missing cached extended-chart series title"))?;
        columns[column].header = title.clone();
        let name = column_name(column);
        let replacement = format!("<cx:txData><cx:f>'{sheet}'!${name}$1</cx:f><cx:v>{}</cx:v></cx:txData>", escaped(&title));
        edits.push((series.start + tx.start + text.start, series.start + tx.start + text.end, replacement));
    }
    edits.sort_by_key(|edit| edit.0);
    let mut output = chart.to_string();
    for (start, end, value) in edits.into_iter().rev() { output.replace_range(start..end, &value); }
    Ok((output, columns))
}

fn level_points(xml: &str, kind: CellKind) -> Result<Vec<Option<String>>, FillError> {
    let level = one(xml, "lvl")?;
    let count = read_attribute(&xml[level.start..level.open_end], "ptCount").and_then(|value| value.parse::<usize>().ok()).filter(|count| *count > 0 && *count < MAX_ROWS && *count <= MAX_CELLS).ok_or_else(|| error("Workbook ranges require 1 to 1,000,000 points"))?;
    let mut values = vec![None; count];
    let mut used = BTreeSet::new();
    for point in children(xml, "pt")? {
        let at = read_attribute(&xml[point.start..point.open_end], "idx").and_then(|value| value.parse::<usize>().ok()).filter(|index| *index < count).ok_or_else(|| error("Invalid extended-chart point index"))?;
        if !used.insert(at) { return Err(error("Duplicate extended-chart point index")); }
        let raw = &xml[point.open_end..point.close_start];
        if raw.contains('<') { return Err(error("Extended-chart points must contain text")); }
        let value = unescape(raw);
        if value.encode_utf16().count() > MAX_CELL_CHARS || !value.chars().all(is_xml_char) { return Err(error("Invalid or oversized worksheet text")); }
        if kind == CellKind::Number && !value.parse::<f64>().is_ok_and(f64::is_finite) { return Err(error("Invalid numeric chart value")); }
        values[at] = Some(value);
    }
    Ok(values)
}

fn read_points(xml: &str, kind: CellKind) -> Result<Vec<Option<String>>, FillError> {
    let count = one(xml, "ptCount")?;
    let count = read_attribute(&xml[count.start..count.open_end], "val").and_then(|value| value.parse::<usize>().ok()).filter(|count| *count > 0 && *count < MAX_ROWS && *count <= MAX_CELLS).ok_or_else(|| error("Workbook ranges require 1 to 1,000,000 points"))?;
    let mut values = vec![None; count];
    let mut used = BTreeSet::new();
    for point in elements(xml, "pt")? {
        let at = read_attribute(&xml[point.start..point.open_end], "idx").and_then(|value| value.parse::<usize>().ok()).filter(|index| *index < count).ok_or_else(|| error("Invalid chart point index"))?;
        if !used.insert(at) { return Err(error("Duplicate chart point index")); }
        let value = super::element_text(&xml[point.start..point.end], "c:v").ok_or_else(|| error("Missing chart point value"))?;
        if value.encode_utf16().count() > MAX_CELL_CHARS || !value.chars().all(is_xml_char) { return Err(error("Invalid or oversized worksheet text")); }
        if kind == CellKind::Number && !value.parse::<f64>().is_ok_and(f64::is_finite) { return Err(error("Invalid numeric chart value")); }
        values[at] = Some(value);
    }
    Ok(values)
}

fn column_name(mut column: usize) -> String {
    let mut out = Vec::new();
    loop {
        out.push((b'A' + (column % 26) as u8) as char);
        if column < 26 { break; }
        column = column / 26 - 1;
    }
    out.into_iter().rev().collect()
}

fn write_sheet(columns: &[Column]) -> Result<String, FillError> {
    let mut xml = format!("<?xml version=\"1.0\"?><worksheet xmlns=\"{SHEET_NS}\"><sheetViews><sheetView workbookViewId=\"0\" tabSelected=\"1\"/></sheetViews><sheetData><row r=\"1\">");
    for (index, column) in columns.iter().enumerate() {
        write_cell(&mut xml, &format!("{}1", column_name(index)), &column.header, CellKind::Text)?;
    }
    xml.push_str("</row>");
    let rows = columns.iter().map(|column| column.values.len()).max().unwrap_or(0);
    for row in 0..rows {
        xml.push_str(&format!("<row r=\"{}\">", row + 2));
        for (index, column) in columns.iter().enumerate() {
            if let Some(Some(value)) = column.values.get(row) { write_cell(&mut xml, &format!("{}{}", column_name(index), row + 2), value, column.kind)?; }
        }
        xml.push_str("</row>");
        if xml.len() > MAX_ARCHIVE_BYTES { return Err(error("Generated worksheet exceeds 64 MiB")); }
    }
    xml.push_str("</sheetData></worksheet>");
    Ok(xml)
}

fn write_cell(out: &mut String, cell: &str, value: &str, kind: CellKind) -> Result<(), FillError> {
    if value.encode_utf16().count() > MAX_CELL_CHARS || !value.chars().all(is_xml_char) { return Err(error("Invalid or oversized worksheet text")); }
    let mut escaped = String::new(); escape(value, &mut escaped);
    match kind {
        CellKind::Text => out.push_str(&format!("<c r=\"{cell}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">{escaped}</t></is></c>")),
        CellKind::Number => out.push_str(&format!("<c r=\"{cell}\"><v>{escaped}</v></c>")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hierarchy() -> String {
        format!("<cx:chartSpace xmlns:cx=\"{CHART_EX_NS}\"><cx:chartData><cx:data id=\"0\"><cx:strDim type=\"cat\"><cx:lvl ptCount=\"2\"><cx:pt idx=\"0\">Leaf A</cx:pt><cx:pt idx=\"1\">Leaf B</cx:pt></cx:lvl><cx:lvl ptCount=\"2\"><cx:pt idx=\"0\">Parent</cx:pt><cx:pt idx=\"1\">Parent</cx:pt></cx:lvl></cx:strDim><cx:numDim type=\"size\"><cx:lvl ptCount=\"2\" formatCode=\"0.00\"><cx:pt idx=\"0\">10</cx:pt><cx:pt idx=\"1\">20</cx:pt></cx:lvl></cx:numDim></cx:data></cx:chartData><cx:chart><cx:plotArea><cx:plotAreaRegion><cx:series layoutId=\"sunburst\"><cx:tx><cx:txData><cx:v>Revenue</cx:v></cx:txData></cx:tx><cx:dataId val=\"0\"/></cx:series></cx:plotAreaRegion></cx:plotArea></cx:chart></cx:chartSpace>")
    }

    #[test]
    fn reverses_hierarchy_columns() {
        let (xml, columns) = extended_data(&hierarchy(), "Data").unwrap();
        assert_eq!(columns[0].values, [Some("Parent".into()), Some("Parent".into())]);
        assert_eq!(columns[1].values, [Some("Leaf A".into()), Some("Leaf B".into())]);
        assert_eq!(columns[2].values, [Some("10".into()), Some("20".into())]);
        assert_eq!(columns[2].header, "Revenue");
        assert!(xml.contains("<cx:f dir=\"col\">'Data'!$A$2:$B$3</cx:f>"));
        assert!(xml.contains("<cx:f dir=\"col\">'Data'!$C$2:$C$3</cx:f>"));
        assert!(xml.contains("<cx:f>'Data'!$C$1</cx:f><cx:v>Revenue</cx:v>"));
        assert!(xml.find("Leaf A").unwrap() < xml.find("Parent").unwrap());
    }

    #[test]
    fn rejects_unequal_levels() {
        let xml = hierarchy().replacen("ptCount=\"2\"", "ptCount=\"1\"", 1).replacen("<cx:pt idx=\"1\">Leaf B</cx:pt>", "", 1);
        assert!(extended_data(&xml, "Data").unwrap_err().message.contains("equal point counts"));
    }
}
