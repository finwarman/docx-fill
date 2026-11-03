//! SpreadsheetML templates. Package I/O stays with the caller, as it does for DOCX.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde_json::Value;

use crate::properties::set_attribute;
use crate::xml::{read_attribute, unescape};
use crate::{FillError, FillErrorId, FillHooks, FillOptions, NoHooks, PackageParts};

mod cells;
mod content;
mod effects;
mod inspection;
mod layout;
mod media;
mod package_util;
mod references;
mod sheet;
mod styles;
mod surfaces;
mod text;
mod workbook;
mod xmlutil;

pub use inspection::{SpreadsheetSheetTags, SpreadsheetTag, SpreadsheetTagKind};
pub use layout::RowHeightMode;
pub use workbook::SheetInstance;

/// Resource limits applied while rendering a workbook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkbookLimits {
    pub max_output_rows: usize,
    pub max_output_cells: usize,
    pub max_output_bytes: usize,
    pub max_loop_depth: usize,
}

impl Default for WorkbookLimits {
    fn default() -> Self {
        Self {
            max_output_rows: 1_048_576,
            max_output_cells: 1_000_000,
            max_output_bytes: 256 * 1024 * 1024,
            max_loop_depth: 64,
        }
    }
}

use xmlutil::{elements, local_name, prefix};

const SHEET_NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const STRICT_SHEET_NS: &str = "http://purl.oclc.org/ooxml/spreadsheetml/main";
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
    /// Chart sheets, drawings or pivot caches that may read worksheet rows.
    row_dependents: bool,
    date_1904: bool,
    edits: workbook::WorkbookEdits,
    options: FillOptions,
    limits: WorkbookLimits,
    strict: bool,
    invalid_dependencies: BTreeSet<String>,
    chart_sheets: Vec<(String, String)>,
    locked_structure: bool,
    row_height_mode: RowHeightMode,
}

#[derive(Debug)]
struct Relationship {
    id: String,
    kind: String,
    target: String,
    external: bool,
}

impl WorkbookTemplate {
    /// Parses an unpacked workbook using the existing delimiter, expression and hook options.
    /// The input must contain its content types and root/workbook relationships.
    pub fn parse(
        parts: impl IntoIterator<Item = (String, Vec<u8>)>,
        options: &FillOptions,
    ) -> Result<Self, FillError> {
        crate::template::validate_tag_prefixes(options)?;
        let parts: Vec<_> = parts.into_iter().collect();
        let mut names = BTreeSet::new();
        let mut total = 0usize;
        if parts.len() > MAX_PARTS {
            return Err(error("Workbook has more than 10,000 parts"));
        }
        for (name, bytes) in &parts {
            if !valid_path(name) || !names.insert(name.as_str()) {
                return Err(error(format!("Invalid or duplicate workbook part: {name}")));
            }
            total = total
                .checked_add(bytes.len())
                .ok_or_else(|| error("Workbook size overflow"))?;
            if total > MAX_BYTES {
                return Err(error("Unpacked workbook exceeds 256 MiB"));
            }
            if name.to_lowercase().starts_with("_xmlsignatures/") {
                return Err(error("Signed workbooks cannot be filled"));
            }
        }
        let sources: BTreeMap<_, _> = parts
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
            .collect();
        let root_rels = read_xml(&sources, "_rels/.rels")?;
        let roots = relationships(root_rels)?;
        let office: Vec<_> = roots
            .iter()
            .filter(|rel| {
                rel.kind == format!("{OFFICE_NS}/officeDocument")
                    || rel.kind
                        == "http://purl.oclc.org/ooxml/officeDocument/relationships/officeDocument"
            })
            .collect();
        let [office] = office.as_slice() else {
            return Err(error("Expected one officeDocument relationship"));
        };
        if office.external {
            return Err(error("The workbook relationship must be internal"));
        }
        let workbook = resolve("", &office.target)?;
        let workbook_xml = read_xml(&sources, &workbook)?;
        let wb_root = check_root_either(workbook_xml, "workbook", &[SHEET_NS, STRICT_SHEET_NS])?;
        let sheet_ns = root_namespace(workbook_xml)?;
        let strict = sheet_ns == STRICT_SHEET_NS;
        let office_ns = if sheet_ns == STRICT_SHEET_NS {
            "http://purl.oclc.org/ooxml/officeDocument/relationships"
        } else {
            OFFICE_NS
        };
        let types = read_xml(&sources, "[Content_Types].xml")?;
        check_root(types, "Types", TYPES_NS)?;
        let mut overrides = BTreeMap::new();
        for item in elements(types)?
            .into_iter()
            .filter(|e| local_name(&e.name) == "Override")
        {
            let raw = &types[item.start..item.open_end];
            let name = unescape(required(raw, "PartName")?);
            if overrides
                .insert(name, unescape(required(raw, "ContentType")?))
                .is_some()
            {
                return Err(error("Duplicate content type override"));
            }
        }
        if overrides.values().any(|content_type| {
            let lowered = content_type.to_lowercase();
            lowered.contains("digital-signature") || lowered.contains("vbaprojectsignature")
        }) {
            return Err(error("Digitally signed workbooks cannot be edited"));
        }
        for (path, bytes) in parts.iter().filter(|(path, _)| path.ends_with(".rels")) {
            let rels =
                std::str::from_utf8(bytes).map_err(|_| error("Invalid package relationships"))?;
            if relationships(rels)?.iter().any(|rel| {
                let kind = rel.kind.to_lowercase();
                kind.contains("digital-signature") || kind.contains("vbaprojectsignature")
            }) {
                return Err(error(format!(
                    "Signed workbook relationship in {path} cannot be edited"
                )));
            }
        }
        let content_type = overrides.get(&format!("/{workbook}")).map(String::as_str);
        if !matches!(
            content_type,
            Some(
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"
                    | "application/vnd.openxmlformats-officedocument.spreadsheetml.template.main+xml"
                    | "application/vnd.ms-excel.sheet.macroEnabled.main+xml"
                    | "application/vnd.ms-excel.template.macroEnabled.main+xml"
            )
        ) {
            return Err(error("Unsupported spreadsheet workbook content type"));
        }
        let relationships_path = relation_path(&workbook);
        let rels = relationships(read_xml(&sources, &relationships_path)?)?;
        let rels_by_id: BTreeMap<_, _> = rels.iter().map(|rel| (rel.id.as_str(), rel)).collect();
        let shared_rels: Vec<_> = rels
            .iter()
            .filter(|rel| rel.kind == format!("{office_ns}/sharedStrings"))
            .collect();
        if shared_rels.len() > 1 {
            return Err(error("Duplicate shared-string relationships"));
        }
        let shared_strings = shared_rels
            .first()
            .map(|rel| {
                if rel.external {
                    return Err(error("Shared strings must be internal"));
                }
                resolve(&workbook, &rel.target)
            })
            .transpose()?;
        let shared = if let Some(path) = &shared_strings {
            let xml = read_xml(&sources, path)?;
            check_root(xml, "sst", sheet_ns)?;
            let all = elements(xml)?;
            direct_children(&all, &all[0])
                .into_iter()
                .filter(|e| e.name == format!("{}si", prefix(&all[0].name)))
                .map(|e| xml[e.start..e.end].to_string())
                .collect()
        } else {
            Vec::new()
        };
        let mut sheets = Vec::new();
        let mut chart_sheets = Vec::new();
        let mut row_numbers = Vec::new();
        let mut sheet_names = BTreeSet::new();
        let mut sheet_ids = BTreeSet::new();
        let mut sheet_paths = BTreeSet::new();
        let mut errors = Vec::new();
        let mut invalid_dependencies = BTreeSet::new();
        let wb_elements = elements(workbook_xml)?;
        let mut row_dependents = wb_elements
            .iter()
            .any(|e| local_name(&e.name) == "pivotCaches")
            || rels
                .iter()
                .any(|rel| rel.kind == format!("{office_ns}/pivotCacheDefinition"));
        let sheet_containers: Vec<_> = direct_children(&wb_elements, &wb_elements[0])
            .into_iter()
            .filter(|e| e.name == format!("{}sheets", prefix(&wb_elements[0].name)))
            .collect();
        let [sheet_container] = sheet_containers.as_slice() else {
            return Err(error("Expected one workbook sheets collection"));
        };
        for item in direct_children(&wb_elements, sheet_container)
            .into_iter()
            .filter(|e| e.name == format!("{}sheet", prefix(&wb_elements[0].name)))
        {
            let raw = &workbook_xml[item.start..item.open_end];
            let name = unescape(required(raw, "name")?);
            let id = required(raw, "sheetId")?
                .parse::<u32>()
                .ok()
                .filter(|id| (1..=65_534).contains(id))
                .ok_or_else(|| error("Invalid worksheet ID"))?;
            if name.is_empty() || !sheet_names.insert(name.to_lowercase()) || !sheet_ids.insert(id)
            {
                return Err(error("Empty or duplicate worksheet name or ID"));
            }
            let rid = relationship_id(raw, wb_root, office_ns)?;
            let rel = rels_by_id
                .get(rid.as_str())
                .ok_or_else(|| error(format!("Worksheet {name} has a missing relationship")))?;
            if rel.external {
                return Err(error("Worksheets must be internal"));
            }
            let path = resolve(&workbook, &rel.target)?;
            if !sheet_paths.insert(path.clone()) {
                return Err(error("Multiple sheets reference the same part"));
            }
            if rel.kind == format!("{office_ns}/chartsheet") {
                let chart_sheet = read_xml(&sources, &path)?;
                let nodes = elements(chart_sheet)?;
                let root = nodes.first().ok_or_else(|| error("Empty chart sheet"))?;
                let root_xml = &chart_sheet[root.start..root.open_end];
                let related = sources.get(relation_path(&path).as_str()).map(|bytes| {
                    relationships(std::str::from_utf8(bytes).map_err(|_| error("Invalid chart-sheet relationships"))?)
                }).transpose()?.unwrap_or_default();
                for drawing in nodes.iter().filter(|node| local_name(&node.name) == "drawing") {
                    let id = relationship_id(&chart_sheet[drawing.start..drawing.open_end], root_xml, office_ns).ok();
                    let valid = related.iter().any(|rel| Some(&rel.id) == id.as_ref() && !rel.external && rel.kind.ends_with("/drawing")
                        && resolve(&path, &rel.target).is_ok_and(|target| sources.contains_key(target.as_str())));
                    if !valid { invalid_dependencies.insert(path.clone()); }
                }
                chart_sheets.push((name, path));
                row_dependents = true;
                continue;
            }
            if rel.kind != format!("{office_ns}/worksheet") {
                return Err(error(format!("Unsupported sheet relationship for {name}")));
            }
            if overrides.get(&format!("/{path}")).map(String::as_str)
                != Some("application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml")
            {
                return Err(error(format!("Missing worksheet content type: {path}")));
            }
            let xml = read_xml(&sources, &path)?;
            check_root(xml, "worksheet", sheet_ns)?;
            let sheet_elements = elements(xml)?;
            let sheet_rels_path = relation_path(&path);
            let sheet_rels = sources
                .get(sheet_rels_path.as_str())
                .map(|bytes| {
                    relationships(
                        std::str::from_utf8(bytes)
                            .map_err(|_| error("Worksheet relationships are not UTF-8"))?,
                    )
                })
                .transpose()?
                .unwrap_or_default();
            for rel in &sheet_rels {
                if rel.external {
                    continue;
                }
                let target = resolve(&path, &rel.target)?;
                if !sources.contains_key(target.as_str()) {
                    return Err(error(format!("Missing worksheet dependency: {target}")));
                }
            }
            let root_raw = &xml[sheet_elements[0].start..sheet_elements[0].open_end];
            for node in sheet_elements.iter().filter(|node| {
                matches!(
                    local_name(&node.name),
                    "drawing" | "legacyDrawing" | "tablePart" | "pivotTablePart"
                )
            }) {
                let raw = &xml[node.start..node.open_end];
                let rid = match relationship_id(raw, root_raw, office_ns) {
                    Ok(id) => id,
                    Err(_) => {
                        invalid_dependencies.insert(path.clone());
                        continue;
                    }
                };
                let Some(rel) = sheet_rels.iter().find(|rel| rel.id == rid) else {
                    invalid_dependencies.insert(path.clone());
                    continue;
                };
                let expected = match local_name(&node.name) {
                    "drawing" => "/drawing",
                    "legacyDrawing" => "/vmlDrawing",
                    "tablePart" => "/table",
                    _ => "/pivotTable",
                };
                if !rel.kind.ends_with(expected) || rel.external {
                    invalid_dependencies.insert(path.clone());
                }
            }
            let row_prefix = prefix(&sheet_elements[0].name);
            let rows = direct_children(&sheet_elements, &sheet_elements[0])
                .into_iter()
                .find(|e| e.name == format!("{row_prefix}sheetData"))
                .map(|data| {
                    direct_children(&sheet_elements, data)
                        .into_iter()
                        .filter(|e| e.name == format!("{row_prefix}row"))
                        .filter_map(|e| {
                            read_attribute(&xml[e.start..e.open_end], "r")?.parse().ok()
                        })
                        .collect()
                })
                .unwrap_or_default();
            match sheet::Sheet::parse(xml, &shared, options) {
                Ok(sheet) => {
                    sheets.push((WorkbookSheet { name, part: path }, sheet));
                    row_numbers.push(rows);
                }
                Err(error) => errors.push(error.in_part(&path)),
            }
        }
        if !errors.is_empty() {
            return Err(FillError::many(errors));
        }
        let mut calc_chains = BTreeSet::new();
        for rel in &rels {
            if rel.kind == format!("{office_ns}/calcChain") {
                if rel.external {
                    return Err(error("Calculation chains must be internal"));
                }
                let path = resolve(&workbook, &rel.target)?;
                if !sources.contains_key(path.as_str()) {
                    return Err(error("Missing calculation chain part"));
                }
                calc_chains.insert(path);
            }
        }
        let date_1904 = elements(workbook_xml)?
            .iter()
            .find(|e| local_name(&e.name) == "workbookPr")
            .is_some_and(|e| {
                matches!(
                    read_attribute(&workbook_xml[e.start..e.open_end], "date1904"),
                    Some("1" | "true")
                )
            });
        if sources.keys().any(|name| {
            name.eq_ignore_ascii_case("EncryptionInfo")
                || name.eq_ignore_ascii_case("EncryptedPackage")
        }) {
            return Err(error("Encrypted workbooks cannot be filled"));
        }
        if parts
            .iter()
            .any(|(name, _)| name.to_lowercase().contains("vbaprojectsignature"))
        {
            return Err(error("Digitally signed VBA projects cannot be edited"));
        }
        let locked_structure = wb_elements
            .iter()
            .find(|e| local_name(&e.name) == "workbookProtection")
            .is_some_and(|e| {
                matches!(
                    read_attribute(&workbook_xml[e.start..e.open_end], "lockStructure"),
                    Some("1" | "true")
                )
            });
        Ok(Self {
            parts,
            workbook,
            relationships: relationships_path,
            shared_strings,
            sheets,
            row_numbers,
            calc_chains,
            row_dependents,
            date_1904,
            edits: workbook::WorkbookEdits::default(),
            options: options.clone(),
            limits: WorkbookLimits::default(),
            strict,
            invalid_dependencies,
            chart_sheets,
            locked_structure,
            row_height_mode: RowHeightMode::Preserve,
        })
    }

    /// Worksheet names and package paths in workbook order. Chart sheets are preserved separately.
    pub fn sheets(&self) -> Vec<&WorkbookSheet> {
        self.sheets.iter().map(|(info, _)| info).collect()
    }

    /// Keeps only the named worksheets in the next render.
    pub fn retain_sheets(
        &mut self,
        names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<(), FillError> {
        if self.locked_structure {
            return Err(error(
                "Workbook structure is protected and worksheet retention is disabled",
            ));
        }
        self.edits.retain(names.into_iter().map(Into::into))?;
        let known: BTreeSet<_> = self
            .sheets
            .iter()
            .map(|(sheet, _)| sheet.name.to_lowercase())
            .collect();
        if let Some(missing) = self
            .edits
            .retained
            .as_ref()
            .unwrap()
            .difference(&known)
            .next()
        {
            return Err(error(format!(
                "Unknown worksheet in retain_sheets: {missing}"
            )));
        }
        Ok(())
    }

    /// Adds copies of a worksheet, one for each data object.
    pub fn repeat_sheet(
        &mut self,
        source: &str,
        instances: Vec<SheetInstance>,
    ) -> Result<(), FillError> {
        if self.locked_structure {
            return Err(error(
                "Workbook structure is protected and worksheet repetition is disabled",
            ));
        }
        if !self
            .sheets
            .iter()
            .any(|(sheet, _)| sheet.name.eq_ignore_ascii_case(source))
        {
            return Err(error(format!("Unknown worksheet to repeat: {source}")));
        }
        self.edits.repeat(source, instances)
    }

    /// Sets output limits up to the hard Excel and package limits.
    pub fn set_limits(&mut self, limits: WorkbookLimits) -> Result<(), FillError> {
        let hard = WorkbookLimits::default();
        if limits.max_output_rows == 0
            || limits.max_output_rows > hard.max_output_rows
            || limits.max_output_cells == 0
            || limits.max_output_cells > hard.max_output_cells
            || limits.max_output_bytes == 0
            || limits.max_output_bytes > hard.max_output_bytes
            || limits.max_loop_depth == 0
            || limits.max_loop_depth > hard.max_loop_depth
        {
            return Err(error(
                "Workbook limits must be nonzero and no greater than the supported maximums",
            ));
        }
        self.limits = limits;
        Ok(())
    }

    /// Selects how generated row heights are estimated.
    pub fn set_row_height_mode(&mut self, mode: RowHeightMode) {
        self.row_height_mode = mode;
    }

    /// Fills a workbook, rejecting invalid values and unsupported structural edits.
    pub fn render(&self, data: &Value) -> Result<PackageParts<'_>, FillError> {
        self.render_with(data, &mut NoHooks)
    }

    /// Fills with the same expression, missing-value and parser hooks used by DOCX templates.
    pub fn render_with(
        &self,
        data: &Value,
        hooks: &mut dyn FillHooks,
    ) -> Result<PackageParts<'_>, FillError> {
        let mut changed = BTreeMap::new();
        let mut errors = Vec::new();
        let mut structural = false;
        let mut worksheet_maps = BTreeMap::new();
        let mut all_effects = Vec::new();
        let mut rendered_names = BTreeMap::new();
        let mut reserved_names: BTreeSet<String> = self
            .chart_sheets
            .iter()
            .map(|(name, _)| name.to_lowercase())
            .collect();
        let mut copy_data = BTreeMap::new();
        let mut copy_names = BTreeMap::new();
        let mut rendered_cells = 0usize;
        let mut rendered_xml_bytes = 0usize;
        for ((info, sheet), original_rows) in self.sheets.iter().zip(&self.row_numbers) {
            if !self.edits.selection_allows(&info.name) {
                continue;
            }
            let result = (|| {
                let name = render_sheet_name(
                    &info.name,
                    &self.options,
                    data,
                    hooks,
                    self.date_1904,
                    &info.part,
                )?;
                if self.locked_structure && name != info.name {
                    return Err(error(
                        "Workbook structure is protected and worksheet renaming is disabled",
                    ));
                }
                if !reserved_names.insert(name.to_lowercase()) {
                    return Err(error(format!(
                        "Rendered worksheet name is duplicated: {name}"
                    )));
                }
                rendered_names.insert(info.name.clone(), name);
                let rendered = sheet.render_with_limits(
                    data,
                    hooks,
                    &info.part,
                    self.date_1904,
                    &self.limits,
                )?;
                rendered_cells = rendered_cells
                    .checked_add(rendered.cell_map.len())
                    .ok_or_else(|| error("Rendered workbook cell count overflow"))?;
                rendered_xml_bytes = rendered_xml_bytes
                    .checked_add(rendered.xml.len())
                    .ok_or_else(|| error("Rendered workbook byte count overflow"))?;
                if rendered_cells > self.limits.max_output_cells
                    || rendered_xml_bytes > self.limits.max_output_bytes
                {
                    return Err(
                        error("Rendered workbook exceeds the configured output limits")
                            .with_id(FillErrorId::SpreadsheetLimit),
                    );
                }
                all_effects.extend(rendered.effects.iter().cloned());
                let map = references::WorksheetMap::new(
                    &rendered.row_map,
                    rendered.cell_map.clone(),
                    original_rows.clone(),
                );
                worksheet_maps.insert(info.part.clone(), map.clone());
                let resized = rendered.row_map.len() != original_rows.len()
                    || rendered.row_map.iter().any(|(old, new)| old != new)
                    || rendered.cell_map.iter().any(|cell| {
                        cell.source_column != cell.target_column
                            || cell.source_row != cell.target_row
                    });
                if resized && self.invalid_dependencies.contains(&info.part) {
                    return Err(error(
                        "Worksheet has a drawing or related part without a valid relationship",
                    )
                    .with_id(FillErrorId::SpreadsheetReference));
                }
                structural |= resized;
                if resized
                    && elements(&rendered.xml)?.iter().any(|item| {
                        local_name(&item.name) == "pivotTablePart"
                            || local_name(&item.name) == "extLst"
                    })
                {
                    return Err(error(
                        "Row loops with pivot tables or worksheet extensions are not supported",
                    )
                    .with_id(FillErrorId::SpreadsheetReference));
                }
                let xml = if resized {
                    let xml = references::update_sheet_with_map(&rendered.xml, &map)?;
                    references::transform_layout(&xml, &map)?
                } else {
                    rendered.xml
                };
                let original = self.part(&info.part);
                if xml.as_bytes() != original {
                    changed.insert(info.part.clone(), xml.into_bytes());
                }
                Ok(())
            })();
            if let Err(error) = result {
                errors.push(error.in_part(&info.part));
            }
        }
        if !errors.is_empty() {
            return Err(FillError::many(errors));
        }
        let mut package: BTreeMap<String, Vec<u8>> = self
            .parts
            .iter()
            .map(|(name, bytes)| (name.clone(), bytes.clone()))
            .collect();
        for (name, bytes) in &changed {
            package.insert(name.clone(), bytes.clone());
        }
        if !rendered_names.is_empty() || self.edits.retained.is_some() {
            let current = package
                .get(&self.workbook)
                .map(Vec::as_slice)
                .unwrap_or_else(|| self.part(&self.workbook));
            let xml =
                std::str::from_utf8(current).map_err(|_| error("Invalid rendered workbook XML"))?;
            package.insert(
                self.workbook.clone(),
                edit_sheet_names(xml, &self.sheets, &self.edits, &rendered_names)?.into_bytes(),
            );
        }
        if let Some(retained) = &self.edits.retained {
            prune_unretained_sheets(
                &mut package,
                &self.workbook,
                &self.relationships,
                &self.sheets,
                retained,
            )?;
        }
        if !self.edits.copies.is_empty() {
            let mut taken: BTreeSet<String> = reserved_names.clone();
            taken.extend(rendered_names.values().map(|name| name.to_lowercase()));
            for (source_name, instances) in &self.edits.copies {
                if !self.edits.selection_allows(source_name) {
                    return Err(error(format!(
                        "Cannot repeat worksheet {source_name:?} because it is not retained"
                    )));
                }
                let (source_info, template) = self
                    .sheets
                    .iter()
                    .find(|(info, _)| info.name.eq_ignore_ascii_case(source_name))
                    .expect("repeat source validated");
                for (index, instance) in instances.iter().enumerate() {
                    let child_data = merge_instance_data(data, &instance.data);
                    let name = render_sheet_name(
                        &instance.name,
                        &self.options,
                        &child_data,
                        hooks,
                        self.date_1904,
                        &source_info.part,
                    )?;
                    if !taken.insert(name.to_lowercase()) {
                        return Err(error(format!(
                            "Rendered repeated worksheet name is duplicated: {name}"
                        )));
                    }
                    let destination = unique_part_path(&package, &source_info.part);
                    let mut cloned = BTreeMap::new();
                    let mut table_names = BTreeMap::new();
                    clone_part_graph(
                        &mut package,
                        &source_info.part,
                        &destination,
                        &mut cloned,
                        &mut table_names,
                    )?;
                    if package.len() > MAX_PARTS {
                        return Err(error("Repeated workbook exceeds the package part limit")
                            .with_id(FillErrorId::SpreadsheetLimit));
                    }
                    let rendered = template.render_with_limits(
                        &child_data,
                        hooks,
                        &destination,
                        self.date_1904,
                        &self.limits,
                    )?;
                    rendered_cells = rendered_cells
                        .checked_add(rendered.cell_map.len())
                        .ok_or_else(|| error("Rendered workbook cell count overflow"))?;
                    rendered_xml_bytes = rendered_xml_bytes
                        .checked_add(rendered.xml.len())
                        .ok_or_else(|| error("Rendered workbook byte count overflow"))?;
                    if rendered_cells > self.limits.max_output_cells
                        || rendered_xml_bytes > self.limits.max_output_bytes
                    {
                        return Err(error(
                            "Rendered workbook exceeds the configured output limits",
                        )
                        .with_id(FillErrorId::SpreadsheetLimit));
                    }
                    copy_data.insert(destination.clone(), child_data);
                    let map = references::WorksheetMap::new(
                        &rendered.row_map,
                        rendered.cell_map.clone(),
                        self.row_numbers[self
                            .sheets
                            .iter()
                            .position(|(info, _)| info.part == source_info.part)
                            .unwrap()]
                        .clone(),
                    );
                    worksheet_maps.insert(destination.clone(), map.clone());
                    copy_names.insert(destination.clone(), name.clone());
                    let mut xml = if rendered
                        .row_map
                        .iter()
                        .any(|(source, target)| source != target)
                        || rendered.row_map.len() != map.original_rows.len()
                    {
                        references::update_sheet_with_map(&rendered.xml, &map)?
                    } else {
                        rendered.xml
                    };
                    xml = references::transform_anchors(&xml, &map, &rendered.cell_map)?;
                    package.insert(destination.clone(), xml.into_bytes());
                    let copy_name_map = BTreeMap::from([(source_info.name.clone(), name.clone())]);
                    for cloned_part in cloned.values() {
                        if cloned_part.ends_with(".xml") && let Some(bytes) = package.get(cloned_part) {
                            let xml = std::str::from_utf8(bytes)
                                .map_err(|_| error("Invalid cloned dependency XML"))?;
                            let xml = rename_formula_sheet_names(xml, &copy_name_map)?;
                            let xml = rewrite_table_formula_names(&xml, &table_names)?;
                            package.insert(cloned_part.clone(), xml.into_bytes());
                        }
                    }
                    all_effects.extend(rendered.effects);
                    add_workbook_sheet(
                        &mut package,
                        &self.workbook,
                        &self.relationships,
                        &destination,
                        &name,
                    )?;
                    let _ = index;
                }
            }
        }
        let surface_or_template_edits = self
            .parts
            .iter()
            .filter(|(path, _)| path.ends_with(".xml"))
            .filter_map(|(_, bytes)| std::str::from_utf8(bytes).ok())
            .any(|xml| xml.contains(&self.options.delimiters.start));
        let package_changed = package
            .iter()
            .any(|(name, bytes)| self.part_opt(name) != Some(bytes.as_slice()));
        if self.strict
            && (package_changed
                || !self.edits.copies.is_empty()
                || !all_effects.is_empty()
                || surface_or_template_edits)
        {
            normalize_strict_package(&mut package)?;
        }
        let mut maps_by_name = BTreeMap::new();
        for (index, (info, _)) in self.sheets.iter().enumerate() {
            let map = worksheet_maps.get(&info.part).cloned().unwrap_or_else(|| {
                let rows = self.row_numbers[index]
                    .iter()
                    .map(|row| (*row, *row))
                    .collect::<Vec<_>>();
                references::WorksheetMap::new(&rows, Vec::new(), self.row_numbers[index].clone())
            });
            maps_by_name.insert(info.name.clone(), map.clone());
            if let Some(name) = rendered_names.get(&info.name) {
                maps_by_name.insert(name.clone(), map);
            }
        }
        for (part, name) in &copy_names {
            if let Some(map) = worksheet_maps.get(part) {
                maps_by_name.insert(name.clone(), map.clone());
            }
        }
        let formula_index = references::QualifiedFormulaIndex::new(&maps_by_name);
        let formula_parts = package
            .keys()
            .filter(|part| part.ends_with(".xml") && *part != &self.workbook)
            .cloned()
            .collect::<Vec<_>>();
        for part in formula_parts {
            let current = std::str::from_utf8(
                package
                    .get(&part)
                    .ok_or_else(|| error(format!("Missing XML part: {part}")))?,
            )
            .map_err(|_| error(format!("Invalid XML part: {part}")))?
            .to_string();
            let updated = transform_qualified_formulas(&current, &formula_index)?;
            package.insert(
                part,
                rename_formula_sheet_names(&updated, &rendered_names)?.into_bytes(),
            );
        }
        let workbook_xml = std::str::from_utf8(
            package
                .get(&self.workbook)
                .ok_or_else(|| error("Missing workbook XML"))?,
        )
        .map_err(|_| error("Invalid workbook XML"))?
        .to_string();
        let updated = transform_defined_names(&workbook_xml, &formula_index)?;
        package.insert(
            self.workbook.clone(),
            rename_formula_sheet_names(&updated, &rendered_names)?.into_bytes(),
        );
        for (part, map) in &worksheet_maps {
            if map.rows.values().any(|rows| rows.len() > 1)
                || map
                    .cells
                    .iter()
                    .any(|cell| cell.source_column != cell.target_column)
            {
                let rel_path = relation_path(part);
                if let Some(rels) = package.get(&rel_path) {
                    let rels = relationships(
                        std::str::from_utf8(rels)
                            .map_err(|_| error("Invalid worksheet relationships"))?,
                    )?;
                    for rel in rels {
                        if rel.kind.ends_with("/table") && !rel.external {
                            let path = resolve(part, &rel.target)?;
                            if let Some(bytes) = package.get(&path) {
                                let xml = std::str::from_utf8(bytes)
                                    .map_err(|_| error("Invalid table XML"))?;
                                let sheet_xml = std::str::from_utf8(
                                    package
                                        .get(part)
                                        .ok_or_else(|| error("Missing worksheet XML"))?,
                                )
                                .map_err(|_| error("Invalid worksheet XML"))?;
                                package.insert(
                                    path,
                                    references::update_table_part_with_sheet(xml, map, sheet_xml)?
                                        .into_bytes(),
                                );
                            }
                        } else if rel.kind.ends_with("/drawing") && !rel.external {
                            let path = resolve(part, &rel.target)?;
                            if let Some(bytes) = package.get(&path) {
                                let xml = std::str::from_utf8(bytes)
                                    .map_err(|_| error("Invalid drawing XML"))?;
                                package.insert(
                                    path,
                                    references::transform_anchors(xml, map, &map.cells)?
                                        .into_bytes(),
                                );
                            }
                        }
                    }
                }
            }
            let rel_path = relation_path(part);
            if let Some(rels) = package.get(&rel_path) {
                let rels = relationships(std::str::from_utf8(rels)
                    .map_err(|_| error("Invalid worksheet relationships"))?)?;
                let resized = map.rows.iter().any(|(source, targets)| targets.as_slice() != [*source])
                    || map.original_rows.iter().any(|row| !map.rows.contains_key(row))
                    || map.cells.iter().any(|cell| cell.source_column != cell.target_column);
                if resized && rels.iter().any(|rel| rel.kind.ends_with("/threadedComment")) {
                    return Err(error("Structural edits with threaded comments are not supported")
                        .with_id(FillErrorId::SpreadsheetReference).in_part(part));
                }
            }
            content::transform_existing(&mut package, part, map)?;
        }
        let mut processed = BTreeSet::new();
        for ((info, _), _) in self.sheets.iter().zip(&self.row_numbers) {
            if self.edits.selection_allows(&info.name) {
                processed.insert(info.part.clone());
            }
        }
        processed.extend(copy_names.keys().cloned());
        processed.extend(all_effects.iter().map(|effect| effect.part.clone()));
        styles::apply(&mut package, &self.workbook, &all_effects)?;
        let mut postprocess_errors = Vec::new();
        for part in processed {
            let effects: Vec<_> = all_effects
                .iter()
                .filter(|effect| effect.part == part)
                .cloned()
                .collect();
            let result = (|| {
                content::apply(&mut package, &self.workbook, &part, &effects)?;
                media::apply(&mut package, &self.workbook, &part, &effects, hooks)?;
                surfaces::apply(
                    &mut package,
                    &part,
                    &self.options,
                    copy_data.get(&part).unwrap_or(data),
                    hooks,
                )?;
                layout::apply(&mut package, &self.workbook, &part, self.row_height_mode)
            })();
            if let Err(error) = result {
                postprocess_errors.push(error.in_part(&part));
            }
        }
        if !postprocess_errors.is_empty() {
            return Err(FillError::many(postprocess_errors));
        }
        for (name, bytes) in &package {
            if self.part_opt(name) != Some(bytes.as_slice()) {
                changed.insert(name.clone(), bytes.clone());
            }
        }
        if changed.is_empty() {
            return Ok(self
                .parts
                .iter()
                .map(|(name, bytes)| (name.clone(), Cow::Borrowed(bytes.as_slice())))
                .collect());
        }
        if structural {
            if self.chart_sheets.iter().any(|(_, path)| self.invalid_dependencies.contains(path)) {
                return Err(error("A chart sheet has an invalid drawing relationship").with_id(FillErrorId::SpreadsheetReference));
            }
            let xml =
                std::str::from_utf8(self.part(&self.workbook)).expect("validated workbook XML");
            if self.row_dependents
                && elements(xml)?
                    .iter()
                    .any(|element| local_name(&element.name) == "pivotCaches")
            {
                return Err(
                    error("Row loops in workbooks with pivot caches are not supported")
                        .with_id(FillErrorId::SpreadsheetReference)
                        .in_part(&self.workbook),
                );
            }
        }
        // Existing formula results are stale after input cells change. Explicit results on
        // newly generated formula cells remain useful until the spreadsheet recalculates.
        for (info, _) in &self.sheets {
            if !self.edits.selection_allows(&info.name) || !changed.contains_key(&info.part) {
                continue;
            }
            let original =
                std::str::from_utf8(self.part(&info.part)).expect("validated worksheet XML");
            let rendered = package
                .get(&info.part)
                .map(Vec::as_slice)
                .unwrap_or_else(|| self.part(&info.part));
            let rendered = std::str::from_utf8(rendered).expect("rendered worksheet XML");
            let updated = clear_formula_caches(original, rendered, &worksheet_maps[&info.part])?;
            if updated != rendered {
                package.insert(info.part.clone(), updated.into_bytes());
            }
        }
        let workbook = std::str::from_utf8(
            package
                .get(&self.workbook)
                .map(Vec::as_slice)
                .unwrap_or_else(|| self.part(&self.workbook)),
        )
        .map_err(|_| error("Invalid rendered workbook XML"))?;
        package.insert(self.workbook.clone(), recalculate(workbook)?.into_bytes());
        if !self.calc_chains.is_empty() {
            let rels = package
                .get(&self.relationships)
                .map(Vec::as_slice)
                .unwrap_or_else(|| self.part(&self.relationships));
            let rels =
                std::str::from_utf8(rels).map_err(|_| error("Invalid rendered relationships"))?;
            package.insert(
                self.relationships.clone(),
                remove_elements(rels, |name, raw| {
                    local_name(name) == "Relationship"
                        && read_attribute(raw, "Type")
                            .is_some_and(|kind| kind.ends_with("/calcChain"))
                })?
                .into_bytes(),
            );
            let types = std::str::from_utf8(
                package
                    .get("[Content_Types].xml")
                    .map(Vec::as_slice)
                    .unwrap_or_else(|| self.part("[Content_Types].xml")),
            )
            .map_err(|_| error("Invalid rendered content types"))?;
            package.insert(
                "[Content_Types].xml".into(),
                remove_elements(types, |name, raw| {
                    local_name(name) == "Override"
                        && read_attribute(raw, "PartName")
                            .map(unescape)
                            .is_some_and(|path| {
                                self.calc_chains.contains(path.trim_start_matches('/'))
                            })
                })?
                .into_bytes(),
            );
        }
        if let Some(path) = &self.shared_strings {
            let xml = std::str::from_utf8(
                package
                    .get(path)
                    .map(Vec::as_slice)
                    .unwrap_or_else(|| self.part(path)),
            )
            .map_err(|_| error("Invalid rendered shared strings"))?;
            let root = elements(xml)?.remove(0);
            let raw = &xml[root.start..root.open_end];
            let without_count = remove_attribute(raw, "count");
            let mut updated = xml.to_string();
            updated.replace_range(root.start..root.open_end, &without_count);
            if updated != xml {
                package.insert(path.clone(), updated.into_bytes());
            }
        }
        let mut bytes = 0usize;
        for (name, part) in &package {
            if self.calc_chains.contains(name) {
                continue;
            }
            bytes = bytes
                .checked_add(part.len())
                .ok_or_else(|| error("Rendered workbook size overflow"))?;
            if bytes > self.limits.max_output_bytes {
                return Err(error("Rendered workbook exceeds the configured byte limit")
                    .with_id(FillErrorId::SpreadsheetLimit));
            }
        }
        let mut output = Vec::with_capacity(package.len());
        for (name, bytes) in &self.parts {
            if self.calc_chains.contains(name) {
                continue;
            }
            let Some(rendered) = package.remove(name) else {
                continue;
            };
            output.push((
                name.clone(),
                if rendered.as_slice() == bytes.as_slice() {
                    Cow::Borrowed(bytes.as_slice())
                } else {
                    Cow::Owned(rendered)
                },
            ));
        }
        for path in &self.calc_chains {
            package.remove(path);
        }
        output.extend(
            package
                .into_iter()
                .map(|(name, bytes)| (name, Cow::Owned(bytes))),
        );
        Ok(output)
    }

    fn part(&self, name: &str) -> &[u8] {
        &self
            .parts
            .iter()
            .find(|(path, _)| path == name)
            .expect("validated workbook part")
            .1
    }
    fn part_opt(&self, name: &str) -> Option<&[u8]> {
        self.parts
            .iter()
            .find(|(path, _)| path == name)
            .map(|(_, bytes)| bytes.as_slice())
    }
}

fn validate_sheet_name(name: &str) -> Result<(), FillError> {
    if name.is_empty()
        || name.encode_utf16().count() > 31
        || name.starts_with('\'')
        || name.ends_with('\'')
        || name
            .chars()
            .any(|c| c.is_control() || ":\\/?*[]".contains(c))
    {
        return Err(error(format!("Invalid Excel worksheet name: {name:?}")));
    }
    Ok(())
}

fn render_sheet_name(
    source: &str,
    options: &FillOptions,
    data: &Value,
    hooks: &mut dyn FillHooks,
    date_1904: bool,
    part: &str,
) -> Result<String, FillError> {
    let mut delimiters = options.delimiters.clone();
    let tags = text::parse(source, options, &mut delimiters).map_err(|e| e.in_part(part))?;
    if tags.is_empty() {
        validate_sheet_name(source)?;
        return Ok(source.into());
    }
    let mut context = text::Context::new(part, hooks, date_1904);
    let mut output = String::new();
    let mut at = 0;
    for tag in tags {
        output.push_str(&source[at..tag.start]);
        match tag.kind {
            text::Kind::Value => {
                let value = context
                    .value(&tag, &[data], &[])
                    .map(|value| crate::render::print_value(&value))
                    .or_else(|| {
                        context.hooks.missing_value(
                            part,
                            crate::MissingValue {
                                kind: crate::MissingKind::Value,
                                name: &tag.name,
                                written: &tag.written,
                            },
                        )
                    })
                    .unwrap_or_default();
                output.push_str(&value);
            }
            text::Kind::Change => {}
            _ => return Err(error("Worksheet names support value tags only")),
        }
        at = tag.end;
    }
    output.push_str(&source[at..]);
    validate_sheet_name(&output)?;
    Ok(output)
}

fn edit_sheet_names(
    xml: &str,
    sheets: &[(WorkbookSheet, sheet::Sheet)],
    edits: &workbook::WorkbookEdits,
    names: &BTreeMap<String, String>,
) -> Result<String, FillError> {
    let all = elements(xml)?;
    let root = all
        .first()
        .ok_or_else(|| error("Workbook XML has no root"))?;
    let container = direct_children(&all, root)
        .into_iter()
        .find(|e| local_name(&e.name) == "sheets")
        .ok_or_else(|| error("Workbook has no sheets collection"))?;
    let known: BTreeMap<_, _> = sheets
        .iter()
        .map(|(info, _)| (info.name.as_str(), info))
        .collect();
    let nodes = direct_children(&all, container)
        .into_iter()
        .filter(|e| local_name(&e.name) == "sheet")
        .collect::<Vec<_>>();
    let mut index_map = BTreeMap::<usize, Option<usize>>::new();
    let mut retained_names = BTreeSet::new();
    let mut next_index = 0usize;
    let mut edits_xml = Vec::new();
    for (old_index, item) in nodes.iter().enumerate() {
        let raw = &xml[item.start..item.open_end];
        let original = unescape(required(raw, "name")?);
        if known.contains_key(original.as_str()) && !edits.selection_allows(&original) {
            index_map.insert(old_index, None);
            edits_xml.push((item.start, item.end, String::new()));
            continue;
        }
        index_map.insert(old_index, Some(next_index));
        next_index += 1;
        let name = names
            .get(&original)
            .map(String::as_str)
            .unwrap_or(&original);
        if !retained_names.insert(name.to_lowercase()) {
            return Err(error(format!(
                "Rendered worksheet names are not unique: {name}"
            )));
        }
        if name != original {
            let updated = crate::properties::set_attribute(raw, "name", &xmlutil::escaped(name));
            edits_xml.push((item.start, item.open_end, updated));
        }
    }
    if edits.retained.is_some() {
        for node in all
            .iter()
            .filter(|node| matches!(local_name(&node.name), "workbookView" | "definedName"))
        {
            let raw = &xml[node.start..node.open_end];
            if local_name(&node.name) == "workbookView" {
                if let Some(old) =
                    read_attribute(raw, "activeTab").and_then(|value| value.parse::<usize>().ok())
                {
                    let new = index_map.get(&old).copied().flatten().unwrap_or(0);
                    if new != old {
                        edits_xml.push((
                            node.start,
                            node.open_end,
                            crate::properties::set_attribute(raw, "activeTab", &new.to_string()),
                        ));
                    }
                }
            } else if let Some(old) =
                read_attribute(raw, "localSheetId").and_then(|value| value.parse::<usize>().ok())
            {
                match index_map.get(&old).copied().flatten() {
                    Some(new) if new != old => edits_xml.push((
                        node.start,
                        node.open_end,
                        crate::properties::set_attribute(raw, "localSheetId", &new.to_string()),
                    )),
                    None => edits_xml.push((node.start, node.end, String::new())),
                    _ => {}
                }
            }
        }
    }
    let mut out = xml.to_string();
    edits_xml.sort_by_key(|edit| edit.0);
    for (start, end, value) in edits_xml.into_iter().rev() {
        out.replace_range(start..end, &value);
    }
    Ok(out)
}

fn merge_instance_data(root: &Value, instance: &Value) -> Value {
    match (root, instance) {
        (Value::Object(parent), Value::Object(child)) => {
            let mut merged = parent.clone();
            merged.extend(child.clone());
            Value::Object(merged)
        }
        (_, value) => value.clone(),
    }
}

fn unique_part_path(parts: &BTreeMap<String, Vec<u8>>, original: &str) -> String {
    let (directory, file) = original
        .rsplit_once('/')
        .map_or(("", original), |(dir, file)| (dir, file));
    let (stem, extension) = file.rsplit_once('.').unwrap_or((file, "xml"));
    (1..)
        .map(|n| {
            if directory.is_empty() {
                format!("{stem}_copy{n}.{extension}")
            } else {
                format!("{directory}/{stem}_copy{n}.{extension}")
            }
        })
        .find(|path| !parts.contains_key(path))
        .expect("finite package paths")
}

fn clone_part_graph(
    parts: &mut BTreeMap<String, Vec<u8>>,
    source: &str,
    destination: &str,
    cloned: &mut BTreeMap<String, String>,
    table_names: &mut BTreeMap<String, String>,
) -> Result<(), FillError> {
    clone_part_graph_at(parts, source, destination, cloned, table_names, 0)
}

fn clone_part_graph_at(
    parts: &mut BTreeMap<String, Vec<u8>>,
    source: &str,
    destination: &str,
    cloned: &mut BTreeMap<String, String>,
    table_names: &mut BTreeMap<String, String>,
    depth: usize,
) -> Result<(), FillError> {
    if depth > 128 {
        return Err(error("Worksheet dependency graph exceeds 128 levels"));
    }
    if cloned.contains_key(source) {
        return Ok(());
    }
    cloned.insert(source.to_string(), destination.to_string());
    let bytes = parts
        .get(source)
        .ok_or_else(|| error(format!("Missing relationship target: {source}")))?
        .clone();
    parts.insert(destination.to_string(), bytes);
    copy_content_override(parts, source, destination)?;
    let source_rels_path = relation_path(source);
    let Some(source_rels) = parts.get(&source_rels_path).cloned() else {
        return Ok(());
    };
    let rels = std::str::from_utf8(&source_rels)
        .map_err(|_| error("Invalid package relationships"))?
        .to_string();
    let mut updated = rels.clone();
    let nodes = elements(&rels)?;
    let mut replacements = Vec::new();
    for node in nodes
        .iter()
        .filter(|node| local_name(&node.name) == "Relationship")
    {
        let raw = &rels[node.start..node.open_end];
        let kind = unescape(required(raw, "Type")?);
        if kind.contains("pivot") {
            return Err(error(
                "Repeated worksheets with pivot dependencies are not supported",
            ));
        }
        let target = unescape(required(raw, "Target")?);
        if read_attribute(raw, "TargetMode") == Some("External") {
            if kind.ends_with("/hyperlink") || kind.ends_with("/externalLink") {
                continue;
            }
            return Err(error(format!(
                "Cannot safely clone external dependency {kind}"
            )));
        }
        let old_path = resolve(source, &target)?;
        let shared = [
            "/image",
            "/styles",
            "/sharedStrings",
            "/theme",
            "/vbaProject",
            "/printerSettings",
            "/customXml",
            "/oleObject",
        ]
        .iter()
        .any(|suffix| kind.ends_with(suffix));
        let new_path = if shared {
            old_path.clone()
        } else if let Some(existing) = cloned.get(&old_path) {
            existing.clone()
        } else {
            let new_path = unique_part_path(parts, &old_path);
            clone_part_graph_at(parts, &old_path, &new_path, cloned, table_names, depth + 1)?;
            if kind.ends_with("/table") {
                let (old_name, new_name) = uniquify_table(parts, &new_path)?;
                table_names.insert(old_name, new_name);
            }
            new_path
        };
        let relative = package_util::relative(destination, &new_path);
        let replacement =
            crate::properties::set_attribute(raw, "Target", &xmlutil::escaped(&relative));
        replacements.push((node.start, node.open_end, replacement));
    }
    for (start, end, replacement) in replacements.into_iter().rev() {
        updated.replace_range(start..end, &replacement);
    }
    parts.insert(relation_path(destination), updated.into_bytes());
    Ok(())
}

fn copy_content_override(
    parts: &mut BTreeMap<String, Vec<u8>>,
    source: &str,
    destination: &str,
) -> Result<(), FillError> {
    let content_types = std::str::from_utf8(
        parts
            .get("[Content_Types].xml")
            .ok_or_else(|| error("Workbook has no content types"))?,
    )
    .map_err(|_| error("Invalid content types XML"))?
    .to_string();
    let found = elements(&content_types)?.into_iter().find(|node| {
        local_name(&node.name) == "Override"
            && read_attribute(&content_types[node.start..node.open_end], "PartName")
                .map(unescape)
                .as_deref()
                == Some(&format!("/{source}"))
    });
    if let Some(node) = found {
        let mime = unescape(required(
            &content_types[node.start..node.open_end],
            "ContentType",
        )?);
        package_util::type_override(parts, destination, &mime)?;
    }
    Ok(())
}

fn uniquify_table(
    parts: &mut BTreeMap<String, Vec<u8>>,
    path: &str,
) -> Result<(String, String), FillError> {
    let xml = std::str::from_utf8(
        parts
            .get(path)
            .ok_or_else(|| error("Missing cloned table"))?,
    )
    .map_err(|_| error("Invalid table XML"))?
    .to_string();
    let all = elements(&xml)?;
    let root = all
        .first()
        .ok_or_else(|| error("Cloned table XML is empty"))?;
    let old_name = unescape(required(&xml[root.start..root.open_end], "name")?);
    let mut raw = xml[root.start..root.open_end].to_string();
    let id = (1..)
        .find(|id| {
            parts
                .iter()
                .filter(|(name, _)| name.contains("table") && name.ends_with(".xml"))
                .filter_map(|(_, bytes)| std::str::from_utf8(bytes).ok())
                .flat_map(|other| {
                    elements(other)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|node| local_name(&node.name) == "table")
                        .filter_map(|node| {
                            read_attribute(&other[node.start..node.open_end], "id")
                                .and_then(|value| value.parse::<u32>().ok())
                        })
                })
                .all(|existing| existing != *id)
        })
        .expect("finite table IDs");
    raw = crate::properties::set_attribute(&raw, "id", &id.to_string());
    let unique = xmlutil::escaped(&format!("DocxFillTable{id}"));
    let new_name = format!("DocxFillTable{id}");
    raw = crate::properties::set_attribute(&raw, "name", &unique);
    raw = crate::properties::set_attribute(&raw, "displayName", &unique);
    let mut output = xml;
    output.replace_range(root.start..root.open_end, &raw);
    parts.insert(path.to_string(), output.into_bytes());
    Ok((old_name, new_name))
}

fn rewrite_table_formula_names(
    xml: &str,
    names: &BTreeMap<String, String>,
) -> Result<String, FillError> {
    let mut output = xml.to_string();
    let mut edits = Vec::new();
    for node in elements(xml)?
        .into_iter()
        .filter(|node| local_name(&node.name) == "f")
    {
        let formula = unescape(&xml[node.open_end..node.close_start]);
        let updated = rewrite_structured_formula(&formula, names);
        if updated != formula {
            edits.push((node.open_end, node.close_start, xmlutil::escaped(&updated)));
        }
    }
    for (start, end, replacement) in edits.into_iter().rev() {
        output.replace_range(start..end, &replacement);
    }
    Ok(output)
}

fn rewrite_structured_formula(formula: &str, names: &BTreeMap<String, String>) -> String {
    let mut output = String::with_capacity(formula.len());
    let mut at = 0usize;
    let mut in_string = false;
    let mut ordered: Vec<_> = names.iter().collect();
    ordered.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
    while at < formula.len() {
        let ch = formula[at..]
            .chars()
            .next()
            .expect("valid formula boundary");
        if ch == '"' {
            output.push(ch);
            at += 1;
            in_string = !in_string;
            continue;
        }
        if !in_string {
            let before_ok = at == 0
                || !formula[..at]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
            if before_ok && let Some((old, new)) = ordered.iter().find(|(old, _)| {
                    formula[at..].starts_with(old.as_str())
                        && formula[at + old.len()..].starts_with('[')
                }) {
                output.push_str(new);
                at += old.len();
                continue;
            }
        }
        output.push(ch);
        at += ch.len_utf8();
    }
    output
}

fn rename_formula_sheet_names(
    xml: &str,
    names: &BTreeMap<String, String>,
) -> Result<String, FillError> {
    if names.is_empty() {
        return Ok(xml.to_string());
    }
    let mut output = xml.to_string();
    let mut edits = Vec::new();
    for node in elements(xml)?
        .into_iter()
        .filter(|node| matches!(local_name(&node.name), "f" | "definedName"))
    {
        let formula = unescape(&xml[node.open_end..node.close_start]);
        let updated = rename_sheet_references(&formula, names);
        if updated != formula {
            edits.push((node.open_end, node.close_start, xmlutil::escaped(&updated)));
        }
    }
    for (start, end, replacement) in edits.into_iter().rev() {
        output.replace_range(start..end, &replacement);
    }
    Ok(output)
}

fn rename_sheet_references(formula: &str, names: &BTreeMap<String, String>) -> String {
    let mut output = String::with_capacity(formula.len());
    let mut at = 0usize;
    let mut in_string = false;
    let mut ordered: Vec<_> = names.iter().filter(|(old, new)| old != new).collect();
    ordered.sort_by_key(|(old, _)| std::cmp::Reverse(old.len()));
    while at < formula.len() {
        let ch = formula[at..]
            .chars()
            .next()
            .expect("valid formula boundary");
        if ch == '"' {
            output.push(ch);
            at += 1;
            in_string = !in_string;
            continue;
        }
        if !in_string {
            let before_ok = at == 0
                || !formula[..at]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
            let mut replacement = None;
            for (old, new) in &ordered {
                let quoted = format!("'{}'!", old.replace('\'', "''"));
                if formula[at..].starts_with(&quoted) {
                    replacement = Some((quoted.len(), format!("'{}'!", new.replace('\'', "''"))));
                    break;
                }
                let simple = old
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
                if simple
                    && before_ok
                    && formula[at..].starts_with(old.as_str())
                    && formula[at + old.len()..].starts_with('!')
                {
                    replacement = Some((old.len() + 1, format!("'{}'!", new.replace('\'', "''"))));
                    break;
                }
            }
            if let Some((length, replacement)) = replacement {
                output.push_str(&replacement);
                at += length;
                continue;
            }
        }
        output.push(ch);
        at += ch.len_utf8();
    }
    output
}

fn add_workbook_sheet(
    parts: &mut BTreeMap<String, Vec<u8>>,
    workbook: &str,
    _rels_path: &str,
    target: &str,
    name: &str,
) -> Result<(), FillError> {
    let workbook_xml = std::str::from_utf8(
        parts
            .get(workbook)
            .ok_or_else(|| error("Missing workbook XML"))?,
    )
    .map_err(|_| error("Invalid workbook XML"))?
    .to_string();
    let namespace = root_namespace(&workbook_xml)?;
    let office_ns = if namespace == STRICT_SHEET_NS {
        "http://purl.oclc.org/ooxml/officeDocument/relationships"
    } else {
        OFFICE_NS
    };
    let relation_id = package_util::add_relation(
        parts,
        workbook,
        &format!("{office_ns}/worksheet"),
        &package_util::relative(workbook, target),
        false,
    )?;
    let all = elements(&workbook_xml)?;
    let sheet_container = all
        .iter()
        .find(|node| local_name(&node.name) == "sheets")
        .ok_or_else(|| error("Workbook has no sheets collection"))?;
    let max_id = all
        .iter()
        .filter(|node| local_name(&node.name) == "sheet")
        .filter_map(|node| {
            read_attribute(&workbook_xml[node.start..node.open_end], "sheetId")?
                .parse::<u32>()
                .ok()
        })
        .max()
        .unwrap_or(0);
    let next_id = max_id
        .checked_add(1)
        .filter(|id| *id <= 65_534)
        .ok_or_else(|| error("Repeated workbook exceeds Excel's worksheet ID limit"))?;
    let root = all.first().ok_or_else(|| error("Workbook XML is empty"))?;
    let p = prefix(&root.name);
    let rel_prefix =
        find_namespace_prefix(&workbook_xml[root.start..root.open_end], office_ns).unwrap_or("r");
    let node = format!(
        "<{p}sheet name=\"{}\" sheetId=\"{}\" {rel_prefix}:id=\"{}\"/>",
        xmlutil::escaped(name),
        next_id,
        relation_id
    );
    let updated = package_util::append(&workbook_xml, local_name(&sheet_container.name), &node)?;
    parts.insert(workbook.to_string(), updated.into_bytes());
    Ok(())
}

fn find_namespace_prefix<'a>(root: &'a str, namespace: &str) -> Option<&'a str> {
    for attr in root
        .split_whitespace()
        .filter(|part| part.starts_with("xmlns:"))
    {
        let (key, value) = attr.split_once('=')?;
        if value.trim_matches(['\'', '"']) == namespace {
            return key.strip_prefix("xmlns:");
        }
    }
    None
}

fn transform_qualified_formulas(
    xml: &str,
    index: &references::QualifiedFormulaIndex,
) -> Result<String, FillError> {
    let mut output = xml.to_string();
    let mut edits = Vec::new();
    for node in elements(xml)?
        .into_iter()
        .filter(|node| local_name(&node.name) == "f")
    {
        let formula = unescape(&xml[node.open_end..node.close_start]);
        let updated = references::transform_qualified_formula_refs_with_index(&formula, index)?;
        if updated != formula {
            edits.push((node.open_end, node.close_start, xmlutil::escaped(&updated)));
        }
    }
    for (start, end, replacement) in edits.into_iter().rev() {
        output.replace_range(start..end, &replacement);
    }
    Ok(output)
}

fn transform_defined_names(
    xml: &str,
    index: &references::QualifiedFormulaIndex,
) -> Result<String, FillError> {
    let mut output = xml.to_string();
    let mut edits = Vec::new();
    for node in elements(xml)?
        .into_iter()
        .filter(|node| local_name(&node.name) == "definedName")
    {
        let formula = unescape(&xml[node.open_end..node.close_start]);
        let updated = references::transform_qualified_formula_refs_with_index(&formula, index)?;
        if updated != formula {
            edits.push((node.open_end, node.close_start, xmlutil::escaped(&updated)));
        }
    }
    for (start, end, replacement) in edits.into_iter().rev() {
        output.replace_range(start..end, &replacement);
    }
    Ok(output)
}

fn normalize_strict_package(parts: &mut BTreeMap<String, Vec<u8>>) -> Result<(), FillError> {
    const STRICT_OFFICE_REL: &str = "http://purl.oclc.org/ooxml/officeDocument/relationships";
    const NAMESPACES: &[(&str, &str)] = &[
        (
            "http://purl.oclc.org/ooxml/spreadsheetml/main",
            "http://schemas.openxmlformats.org/spreadsheetml/2006/main",
        ),
        (
            "http://purl.oclc.org/ooxml/drawingml/main",
            "http://schemas.openxmlformats.org/drawingml/2006/main",
        ),
        (
            "http://purl.oclc.org/ooxml/drawingml/chart",
            "http://schemas.openxmlformats.org/drawingml/2006/chart",
        ),
        (
            "http://purl.oclc.org/ooxml/drawingml/spreadsheetDrawing",
            "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing",
        ),
        (
            "http://purl.oclc.org/ooxml/officeDocument/extendedProperties",
            "http://schemas.openxmlformats.org/officeDocument/2006/extended-properties",
        ),
        (
            "http://purl.oclc.org/ooxml/officeDocument/docPropsVTypes",
            "http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes",
        ),
    ];
    for (path, bytes) in parts.iter_mut() {
        if !(path.ends_with(".xml") || path.ends_with(".rels")) {
            continue;
        }
        let xml = std::str::from_utf8(bytes)
            .map_err(|_| error(format!("Invalid UTF-8 in strict XML part {path}")))?;
        if !xml.contains(STRICT_OFFICE_REL)
            && !NAMESPACES.iter().any(|(strict, _)| xml.contains(strict))
        {
            continue;
        }
        let mut updated = xml.to_string();
        let mut edits = Vec::new();
        for node in elements(xml)? {
            let raw = &xml[node.start..node.open_end];
            let normalized = normalize_strict_start_tag(raw, NAMESPACES, STRICT_OFFICE_REL);
            if normalized != raw {
                edits.push((node.start, node.open_end, normalized));
            }
        }
        for (start, end, replacement) in edits.into_iter().rev() {
            updated.replace_range(start..end, &replacement);
        }
        *bytes = updated.into_bytes();
    }
    Ok(())
}

fn normalize_strict_start_tag(raw: &str, namespaces: &[(&str, &str)], office_ns: &str) -> String {
    let mut output = raw.to_string();
    let mut cursor = 1usize;
    let bytes = raw.as_bytes();
    while cursor < bytes.len() {
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || matches!(bytes[cursor], b'>' | b'/') {
            break;
        }
        let key_start = cursor;
        while cursor < bytes.len()
            && !bytes[cursor].is_ascii_whitespace()
            && !matches!(bytes[cursor], b'=' | b'>' | b'/')
        {
            cursor += 1;
        }
        let key_end = cursor;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b'=') {
            break;
        }
        cursor += 1;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let Some(quote @ (b'\'' | b'"')) = bytes.get(cursor).copied() else {
            break;
        };
        cursor += 1;
        let value_start = cursor;
        while cursor < bytes.len() && bytes[cursor] != quote {
            cursor += 1;
        }
        let value_end = cursor;
        let key = &raw[key_start..key_end];
        let value = &raw[value_start..value_end];
        let replacement = if key == "xmlns" || key.starts_with("xmlns:") {
            namespaces.iter().find_map(|(strict, transitional)| {
                (value == *strict).then(|| transitional.to_string())
            })
        } else if key == "Type" && value.starts_with(office_ns) {
            Some(value.replacen(office_ns, OFFICE_NS, 1))
        } else {
            None
        };
        if let Some(replacement) = replacement {
            output.replace_range(value_start..value_end, &replacement);
        }
        cursor = value_end.saturating_add(1);
    }
    output
}

fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.ends_with('/')
        && !path.contains(['\\', ':', '?', '#'])
        && path
            .split('/')
            .all(|segment| !matches!(segment, "" | "." | ".."))
}

fn read_xml<'a>(sources: &BTreeMap<&str, &'a [u8]>, path: &str) -> Result<&'a str, FillError> {
    let bytes = sources
        .get(path)
        .ok_or_else(|| error(format!("Missing workbook part: {path}")))?;
    if bytes.len() > MAX_XML {
        return Err(error("Spreadsheet XML exceeds 16 MiB").in_part(path));
    }
    std::str::from_utf8(bytes).map_err(|_| error("Spreadsheet XML is not UTF-8").in_part(path))
}

fn required<'a>(raw: &'a str, name: &str) -> Result<&'a str, FillError> {
    read_attribute(raw, name).ok_or_else(|| error(format!("Missing spreadsheet attribute: {name}")))
}

fn check_root<'a>(xml: &'a str, expected: &str, namespace: &str) -> Result<&'a str, FillError> {
    let all = elements(xml)?;
    let root = all.first().ok_or_else(|| error("Empty spreadsheet XML"))?;
    let raw = &xml[root.start..root.open_end];
    let attr = if prefix(&root.name).is_empty() {
        "xmlns".into()
    } else {
        format!("xmlns:{}", prefix(&root.name).trim_end_matches(':'))
    };
    if local_name(&root.name) != expected || read_attribute(raw, &attr) != Some(namespace) {
        return Err(error(format!(
            "Expected {expected} in the standard OOXML namespace"
        )));
    }
    for outer in [&xml[..root.start], &xml[root.end..]] {
        for token in crate::xml::tokenize(outer).map_err(error)? {
            match token {
                crate::xml::Token::Text(text) if text.trim().is_empty() => {}
                crate::xml::Token::Element {
                    kind: crate::xml::ElementKind::Other,
                    raw,
                    ..
                } if raw.starts_with("<?") || raw.starts_with("<!--") => {}
                _ => return Err(error("Unexpected content outside spreadsheet XML root")),
            }
        }
    }
    if xml.contains("<!DOCTYPE") || xml.contains("<!ENTITY") {
        return Err(error(
            "DTD declarations are not supported in spreadsheet XML",
        ));
    }
    Ok(raw)
}

fn check_root_either<'a>(
    xml: &'a str,
    expected: &str,
    namespaces: &[&str],
) -> Result<&'a str, FillError> {
    let namespace = root_namespace(xml)?;
    if !namespaces.contains(&namespace) {
        return Err(error(format!(
            "Expected {expected} in a supported OOXML namespace"
        )));
    }
    check_root(xml, expected, namespace)
}

fn root_namespace(xml: &str) -> Result<&str, FillError> {
    let all = elements(xml)?;
    let root = all.first().ok_or_else(|| error("Empty spreadsheet XML"))?;
    let raw = &xml[root.start..root.open_end];
    let attr = if prefix(&root.name).is_empty() {
        "xmlns".into()
    } else {
        format!("xmlns:{}", prefix(&root.name).trim_end_matches(':'))
    };
    read_attribute(raw, &attr).ok_or_else(|| error("Spreadsheet XML root has no namespace"))
}

fn direct_children<'a>(
    all: &'a [xmlutil::Element],
    parent: &xmlutil::Element,
) -> Vec<&'a xmlutil::Element> {
    let mut children = Vec::new();
    let mut skip = parent.open_end;
    for item in all
        .iter()
        .filter(|e| e.start >= parent.open_end && e.end <= parent.close_start)
    {
        if item.start < skip {
            continue;
        }
        children.push(item);
        skip = item.end;
    }
    children
}

fn relationships(xml: &str) -> Result<Vec<Relationship>, FillError> {
    check_root(xml, "Relationships", REL_NS)?;
    let mut ids = BTreeSet::new();
    let mut result = Vec::new();
    for item in elements(xml)?
        .into_iter()
        .filter(|e| local_name(&e.name) == "Relationship")
    {
        let raw = &xml[item.start..item.open_end];
        let id = unescape(required(raw, "Id")?);
        if id.is_empty() || !ids.insert(id.clone()) {
            return Err(error("Empty or duplicate relationship ID"));
        }
        let mode = read_attribute(raw, "TargetMode");
        if mode.is_some_and(|mode| mode != "External" && mode != "Internal") {
            return Err(error("Invalid relationship TargetMode"));
        }
        result.push(Relationship {
            id,
            kind: unescape(required(raw, "Type")?),
            target: unescape(required(raw, "Target")?),
            external: mode == Some("External"),
        });
    }
    Ok(result)
}

fn relationship_id(raw: &str, root: &str, office_ns: &str) -> Result<String, FillError> {
    // The relationship namespace can use any prefix declared on the root or sheet.
    for text in [raw, root] {
        for fragment in text.split_whitespace() {
            let Some(rest) = fragment.strip_prefix("xmlns:") else {
                continue;
            };
            let ns_prefix = rest.split(['=', ' ', '\t']).next().unwrap_or_default();
            let attr = format!("xmlns:{ns_prefix}");
            if read_attribute(raw, &attr).or_else(|| read_attribute(root, &attr)) == Some(office_ns)
                && let Some(value) = read_attribute(raw, &format!("{ns_prefix}:id"))
            {
                return Ok(unescape(value));
            }
        }
    }
    Err(error("Worksheet has no office relationship ID"))
}

fn relation_path(source: &str) -> String {
    match source.rsplit_once('/') {
        Some((folder, file)) => format!("{folder}/_rels/{file}.rels"),
        None => format!("_rels/{source}.rels"),
    }
}

fn resolve(source: &str, target: &str) -> Result<String, FillError> {
    if target.is_empty() || target.contains(['\\', ':', '?', '#', '%']) {
        return Err(error("Unsupported or unsafe relationship target"));
    }
    let mut segments: Vec<_> = if target.starts_with('/') {
        Vec::new()
    } else {
        source
            .rsplit_once('/')
            .map_or(Vec::new(), |(folder, _)| folder.split('/').collect())
    };
    for segment in target.trim_start_matches('/').split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if segments.pop().is_none() {
                    return Err(error("Relationship target escapes the package"));
                }
            }
            _ => segments.push(segment),
        }
    }
    let path = segments.join("/");
    if !valid_path(&path) {
        return Err(error("Invalid relationship target"));
    }
    Ok(path)
}

fn recalculate(xml: &str) -> Result<String, FillError> {
    let all = elements(xml)?;
    let root = all
        .first()
        .ok_or_else(|| error("Missing workbook XML root"))?;
    let tag = all.iter().find(|item| local_name(&item.name) == "calcPr");
    let mut out = xml.to_string();
    if let Some(item) = tag {
        let mut raw = xml[item.start..item.open_end].to_string();
        for (name, value) in [
            ("calcMode", "auto"),
            ("fullCalcOnLoad", "1"),
            ("forceFullCalc", "1"),
        ] {
            raw = set_attribute(&raw, name, value);
        }
        out.replace_range(item.start..item.open_end, &raw);
    } else {
        let at = all
            .iter()
            .find(|item| {
                matches!(
                    local_name(&item.name),
                    "oleSize"
                        | "customWorkbookViews"
                        | "pivotCaches"
                        | "smartTagPr"
                        | "smartTagTypes"
                        | "webPublishing"
                        | "fileRecoveryPr"
                        | "webPublishObjects"
                        | "extLst"
                )
            })
            .map_or(root.close_start, |item| item.start);
        out.insert_str(
            at,
            &format!(
                "<{}calcPr calcMode=\"auto\" fullCalcOnLoad=\"1\" forceFullCalc=\"1\"/>",
                prefix(&root.name)
            ),
        );
    }
    Ok(out)
}

fn remove_elements(
    xml: &str,
    mut remove: impl FnMut(&str, &str) -> bool,
) -> Result<String, FillError> {
    let mut out = xml.to_string();
    for item in elements(xml)?.into_iter().rev() {
        if remove(&item.name, &xml[item.start..item.open_end]) {
            out.replace_range(item.start..item.end, "");
        }
    }
    Ok(out)
}

fn prune_unretained_sheets(
    parts: &mut BTreeMap<String, Vec<u8>>,
    workbook: &str,
    workbook_rels_path: &str,
    sheets: &[(WorkbookSheet, sheet::Sheet)],
    retained: &BTreeSet<String>,
) -> Result<(), FillError> {
    let removed: BTreeSet<_> = sheets
        .iter()
        .filter(|(info, _)| !retained.contains(&info.name.to_lowercase()))
        .map(|(info, _)| info.part.clone())
        .collect();
    if removed.is_empty() {
        return Ok(());
    }
    let mut candidates = reachable_from_parts(parts, removed.iter().cloned())?;
    let rel_xml = std::str::from_utf8(
        parts
            .get(workbook_rels_path)
            .ok_or_else(|| error("Missing workbook relationships"))?,
    )
    .map_err(|_| error("Invalid workbook relationships"))?
    .to_string();
    let mut relationship_ids = BTreeSet::new();
    for rel in relationships(&rel_xml)? {
        if !rel.external
            && rel.kind.ends_with("/worksheet")
            && removed.contains(&resolve(workbook, &rel.target)?)
        {
            relationship_ids.insert(rel.id);
        }
    }
    if !relationship_ids.is_empty() {
        let updated = remove_elements(&rel_xml, |name, raw| {
            local_name(name) == "Relationship"
                && read_attribute(raw, "Id").is_some_and(|id| relationship_ids.contains(id))
        })?;
        parts.insert(workbook_rels_path.to_string(), updated.into_bytes());
    }

    let root_rels_path = "_rels/.rels";
    let root_rels = std::str::from_utf8(
        parts
            .get(root_rels_path)
            .ok_or_else(|| error("Missing root relationships"))?,
    )
    .map_err(|_| error("Invalid root relationships"))?;
    let roots = relationships(root_rels)?
        .into_iter()
        .filter(|rel| !rel.external)
        .map(|rel| resolve("", &rel.target))
        .collect::<Result<Vec<_>, _>>()?;
    let mut reachable = reachable_from_parts(parts, roots)?;
    reachable.insert(root_rels_path.to_string());
    reachable.insert("[Content_Types].xml".to_string());
    candidates.remove(root_rels_path);
    candidates.remove("[Content_Types].xml");
    let removed_parts: BTreeSet<_> = candidates.difference(&reachable).cloned().collect();
    for path in &removed_parts {
        parts.remove(path);
    }
    let content_types = std::str::from_utf8(
        parts
            .get("[Content_Types].xml")
            .ok_or_else(|| error("Missing content types"))?,
    )
    .map_err(|_| error("Invalid content types"))?
    .to_string();
    let updated = remove_elements(&content_types, |name, raw| {
        local_name(name) == "Override"
            && read_attribute(raw, "PartName")
                .map(unescape)
                .is_some_and(|path| removed_parts.contains(path.trim_start_matches('/')))
    })?;
    parts.insert("[Content_Types].xml".to_string(), updated.into_bytes());
    Ok(())
}

fn reachable_from_parts(
    parts: &BTreeMap<String, Vec<u8>>,
    starts: impl IntoIterator<Item = String>,
) -> Result<BTreeSet<String>, FillError> {
    let mut reachable = BTreeSet::new();
    let mut queue: VecDeque<String> = starts.into_iter().collect();
    while let Some(path) = queue.pop_front() {
        if !reachable.insert(path.clone()) {
            continue;
        }
        let rel_path = relation_path(&path);
        let Some(bytes) = parts.get(&rel_path) else {
            continue;
        };
        reachable.insert(rel_path);
        let rel_xml =
            std::str::from_utf8(bytes).map_err(|_| error("Invalid package relationships"))?;
        for rel in relationships(rel_xml)? {
            if !rel.external {
                queue.push_back(resolve(&path, &rel.target)?);
            }
        }
    }
    Ok(reachable)
}

fn remove_attribute(raw: &str, name: &str) -> String {
    let Some(value) = read_attribute(raw, name) else {
        return raw.to_string();
    };
    let value_at = value.as_ptr() as usize - raw.as_ptr() as usize;
    let start = raw[..value_at - 1].rfind(name).unwrap_or(value_at - 1);
    let mut out = raw.to_string();
    out.replace_range(start..value_at + value.len() + 1, "");
    out
}

fn clear_formula_caches(
    original: &str,
    rendered: &str,
    map: &references::WorksheetMap,
) -> Result<String, FillError> {
    let mut formula_cells = BTreeSet::new();
    let mut cell = None;
    for item in elements(original)? {
        if local_name(&item.name) == "c" {
            cell = Some(item);
            continue;
        }
        let Some(current) = &cell else { continue };
        if local_name(&item.name) != "f" || item.start >= current.end {
            continue;
        }
        if let Some(reference) = read_attribute(&original[current.start..current.open_end], "r") {
            formula_cells.insert(reference.to_string());
        }
        cell = None;
    }
    if formula_cells.is_empty() {
        return Ok(rendered.to_string());
    }
    let target_sources: BTreeMap<_, _> = map
        .cells
        .iter()
        .map(|cell| {
            (
                (cell.target_row, cell.target_column),
                (cell.source_column, cell.source_row),
            )
        })
        .collect();
    let mut removals = Vec::new();
    let rendered_elements = elements(rendered)?;
    type FormulaCacheCell = ((u32, u32), usize, bool, Vec<(usize, usize)>);
    let mut current: Option<FormulaCacheCell> = None;
    for node in &rendered_elements {
        if local_name(&node.name) == "c" {
            if let Some((coordinate, _, has_formula, values)) = current.take() && has_formula {
                collect_formula_values(coordinate, values, &target_sources, &formula_cells, &mut removals);
            }
            let raw = &rendered[node.start..node.open_end];
            let coordinate = read_attribute(raw, "r").and_then(parse_cell_address);
            current = coordinate.map(|coordinate| (coordinate, node.end, false, Vec::new()));
            continue;
        }
        if let Some((_, cell_end, has_formula, values)) = current.as_mut() {
            if node.start >= *cell_end {
                continue;
            }
            match local_name(&node.name) {
                "f" => *has_formula = true,
                "v" => values.push((node.start, node.end)),
                _ => {}
            }
        }
    }
    if let Some((coordinate, _, has_formula, values)) = current && has_formula {
        collect_formula_values(coordinate, values, &target_sources, &formula_cells, &mut removals);
    }
    let mut out = String::with_capacity(rendered.len());
    let mut at = 0;
    for (start, end) in removals {
        out.push_str(&rendered[at..start]);
        at = end;
    }
    out.push_str(&rendered[at..]);
    Ok(out)
}

fn collect_formula_values(
    coordinate: (u32, u32),
    values: Vec<(usize, usize)>,
    target_sources: &BTreeMap<(u32, u32), (u32, u32)>,
    formula_cells: &BTreeSet<String>,
    removals: &mut Vec<(usize, usize)>,
) {
    let (target_column, target_row) = coordinate;
    let (source_column, source_row) = target_sources
        .get(&(target_row, target_column))
        .copied()
        .unwrap_or(coordinate);
    let source_reference = format!("{}{}", column_letters(source_column), source_row);
    if formula_cells.contains(&source_reference) {
        removals.extend(values);
    }
}

fn parse_cell_address(reference: &str) -> Option<(u32, u32)> {
    let split = reference.find(|ch: char| ch.is_ascii_digit())?;
    let mut column = 0u32;
    for byte in reference[..split].bytes() {
        if !byte.is_ascii_alphabetic() {
            return None;
        }
        column = column
            .checked_mul(26)?
            .checked_add(u32::from(byte.to_ascii_uppercase() - b'A' + 1))?;
    }
    let row = reference[split..].parse::<u32>().ok()?;
    Some((column, row))
}

fn column_letters(mut number: u32) -> String {
    let mut output = Vec::new();
    while number > 0 {
        number -= 1;
        output.push((b'A' + (number % 26) as u8) as char);
        number /= 26;
    }
    output.iter().rev().collect()
}
