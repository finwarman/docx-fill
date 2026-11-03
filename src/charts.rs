#[cfg(feature = "chart-workbook")]
mod workbook;

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::properties::{find_element_end, find_starts, set_child};
use crate::xml::{ElementKind, Token, escape, read_attribute, tokenize, unescape};
use crate::{ChartDataMode, FillError, FillErrorId, FillOptions, CONTENT_TYPES_PART, add_relationship};

mod extended;
mod namespaces;

const CHART_RELATIONSHIP: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart";
const CHART_CONTENT_TYPE: &str = "application/vnd.openxmlformats-officedocument.drawingml.chart+xml";
const EXTENDED_RELATIONSHIP: &str = "http://schemas.microsoft.com/office/2014/relationships/chartEx";
const EXTENDED_CONTENT_TYPE: &str = "application/vnd.ms-office.chartex+xml";
const MAX_HIERARCHY_DEPTH: usize = 128;
const MAX_MAP_ID_BYTES: usize = 32767;
const MAX_GEOGRAPHY_BYTES: usize = 8 * 1024 * 1024;
const BAR_ORDER: &[&str] = &["c:idx", "c:order", "c:tx", "c:spPr", "c:invertIfNegative", "c:pictureOptions", "c:dPt", "c:dLbls", "c:trendline", "c:errBars", "c:cat", "c:val", "c:shape", "c:extLst"];
const LINE_ORDER: &[&str] = &["c:idx", "c:order", "c:tx", "c:spPr", "c:marker", "c:pictureOptions", "c:dPt", "c:dLbls", "c:trendline", "c:errBars", "c:cat", "c:val", "c:smooth", "c:extLst"];
const AREA_ORDER: &[&str] = &["c:idx", "c:order", "c:tx", "c:spPr", "c:pictureOptions", "c:dPt", "c:dLbls", "c:trendline", "c:errBars", "c:cat", "c:val", "c:extLst"];
const RADAR_ORDER: &[&str] = &["c:idx", "c:order", "c:tx", "c:spPr", "c:pictureOptions", "c:marker", "c:dPt", "c:dLbls", "c:cat", "c:val", "c:extLst"];
const PIE_ORDER: &[&str] = &["c:idx", "c:order", "c:tx", "c:spPr", "c:pictureOptions", "c:explosion", "c:dPt", "c:dLbls", "c:cat", "c:val", "c:extLst"];
const SURFACE_ORDER: &[&str] = &["c:idx", "c:order", "c:tx", "c:spPr", "c:cat", "c:val", "c:extLst"];
const SCATTER_ORDER: &[&str] = &["c:idx", "c:order", "c:tx", "c:spPr", "c:marker", "c:dPt", "c:dLbls", "c:trendline", "c:errBars", "c:xVal", "c:yVal", "c:smooth", "c:extLst"];
const BUBBLE_ORDER: &[&str] = &["c:idx", "c:order", "c:tx", "c:spPr", "c:pictureOptions", "c:invertIfNegative", "c:dPt", "c:dLbls", "c:trendline", "c:errBars", "c:xVal", "c:yVal", "c:bubbleSize", "c:bubble3D", "c:extLst"];
const LABEL_ORDER: &[&str] = &["c:idx", "c:delete", "c:layout", "c:tx", "c:numFmt", "c:spPr", "c:txPr", "c:dLblPos", "c:showLegendKey", "c:showVal", "c:showCatName", "c:showSerName", "c:showPercent", "c:showBubbleSize", "c:separator", "c:extLst"];

#[derive(Clone, Copy)]
enum ChartKind { Bar, Line, Area, Radar, Pie, Doughnut, Surface, Scatter, Bubble }

#[derive(Clone, Copy)]
enum ChartFormat { Standard, Extended }

impl ChartFormat {
    fn relationship(self) -> &'static str { match self { Self::Standard => CHART_RELATIONSHIP, Self::Extended => EXTENDED_RELATIONSHIP } }
    fn content_type(self) -> &'static str { match self { Self::Standard => CHART_CONTENT_TYPE, Self::Extended => EXTENDED_CONTENT_TYPE } }
}

impl ChartKind {
    fn read(name: &str) -> Option<Self> {
        match name {
            "c:barChart" | "c:bar3DChart" => Some(Self::Bar),
            "c:lineChart" | "c:line3DChart" => Some(Self::Line),
            "c:areaChart" | "c:area3DChart" => Some(Self::Area),
            "c:radarChart" => Some(Self::Radar),
            "c:pieChart" | "c:pie3DChart" | "c:ofPieChart" => Some(Self::Pie),
            "c:doughnutChart" => Some(Self::Doughnut),
            "c:surfaceChart" | "c:surface3DChart" => Some(Self::Surface),
            "c:scatterChart" => Some(Self::Scatter),
            "c:bubbleChart" => Some(Self::Bubble),
            _ => None,
        }
    }

    fn series_order(self) -> &'static [&'static str] {
        match self {
            Self::Bar => BAR_ORDER, Self::Line => LINE_ORDER, Self::Area => AREA_ORDER,
            Self::Radar => RADAR_ORDER, Self::Pie | Self::Doughnut => PIE_ORDER, Self::Surface => SURFACE_ORDER,
            Self::Scatter => SCATTER_ORDER, Self::Bubble => BUBBLE_ORDER,
        }
    }
}

#[derive(Debug)]
pub(crate) struct ChartUse {
    occurrence: usize,
    data: ChartData,
}

#[derive(Debug)]
struct ChartData {
    categories: Option<Vec<String>>,
    paths: Option<Vec<Vec<String>>>,
    series: Vec<Series>,
    map: Option<MapData>,
}

#[derive(Debug)]
struct MapData {
    ids: Vec<String>,
    geography: String,
}

impl MapData {
    fn read(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        if object.keys().any(|key| !matches!(key.as_str(), "entityIds" | "geography")) { return None; }
        let geography = value.get("geography")?.as_str()?;
        if geography.len() > MAX_GEOGRAPHY_BYTES { return None; }
        let ids = value.get("entityIds")?.as_array()?.iter().map(|id| {
            let id = id.as_str()?;
            (!id.trim().is_empty() && id.len() <= MAX_MAP_ID_BYTES && id.chars().all(crate::xml::is_xml_char)).then(|| id.to_string())
        }).collect::<Option<Vec<_>>>()?;
        if ids.iter().collect::<BTreeSet<_>>().len() != ids.len() { return None; }
        Some(Self { ids, geography: geography.to_string() })
    }
}

#[derive(Debug)]
struct Series {
    title: Option<String>,
    values: Vec<Option<String>>,
    x_values: Option<Vec<Option<String>>>,
    sizes: Option<Vec<Option<String>>>,
    names: Option<Vec<Option<String>>>,
}

enum DataKind<'a> { Text, Number(&'a str) }

impl ChartData {
    fn read(value: &Value) -> Option<Self> {
        if let Some(categories) = value.get("categories").and_then(Value::as_array)
            && categories.iter().any(Value::is_object) {
            if value.get("map").is_some() { return None; }
            return Self::hierarchy(categories);
        }
        let categories = match value.get("categories") {
            Some(value) => Some(value.as_array()?.iter().map(|value| match value {
                Value::String(text) => Some(text.clone()),
                Value::Number(number) => Some(number.to_string()),
                Value::Null => Some(String::new()),
                _ => None,
            }).collect::<Option<Vec<_>>>()?),
            None => None,
        };
        let mut series = Vec::new();
        let series_values = match value.get("series") {
            Some(series) => series.as_array()?.iter().collect::<Vec<_>>(),
            None if value.get("data").is_some() => vec![value],
            None => return None,
        };
        for value in series_values {
            let title = match value.get("title") { None | Some(Value::Null) => None, Some(title) => Some(title.as_str()?.to_string()) };
            let points = value.get("data")?.as_array()?;
            let is_xy = categories.is_none() || points.iter().any(Value::is_object);
            let mut values = Vec::new();
            let mut x_values = is_xy.then(Vec::new);
            let mut sizes = points.iter().any(|point| point.get("size").is_some()).then(Vec::new);
            let mut names = points.iter().any(|point| point.get("name").is_some()).then(Vec::new);
            for point in points {
                if !is_xy { values.push(number(point)?); continue; }
                if point.is_null() {
                    values.push(None);
                    x_values.as_mut()?.push(None);
                    if let Some(sizes) = &mut sizes { sizes.push(None); }
                    if let Some(names) = &mut names { names.push(None); }
                    continue;
                }
                let mut y = number(point.get("y")?)?;
                let mut x = number(point.get("x")?)?;
                let size = if sizes.is_some() { number(point.get("size")?)? } else { None };
                if x.is_none() || y.is_none() || (sizes.is_some() && size.is_none()) { x = None; y = None; }
                values.push(y);
                x_values.as_mut()?.push(x);
                if let Some(sizes) = &mut sizes { sizes.push(size); }
                if let Some(names) = &mut names { names.push(match point.get("name") { None | Some(Value::Null) => None, Some(name) => Some(name.as_str()?.to_string()) }); }
            }
            series.push(Series { title, values, x_values, sizes, names });
        }
        let map = match value.get("map") { Some(value) => Some(MapData::read(value)?), None => None };
        if map.as_ref().is_some_and(|map| categories.as_ref().is_none_or(|categories| categories.is_empty() || categories.len() != map.ids.len())) { return None; }
        Some(Self { categories, paths: None, series, map })
    }

    fn hierarchy(categories: &[Value]) -> Option<Self> {
        let mut pending = categories.iter().rev().map(|value| (value, Vec::new())).collect::<Vec<_>>();
        let mut paths = Vec::new();
        let mut values = Vec::new();
        while let Some((node, mut path)) = pending.pop() {
            if path.len() >= MAX_HIERARCHY_DEPTH { return None; }
            path.push(node.get("name")?.as_str()?.to_string());
            match node.get("categories") {
                Some(children) if !children.as_array()?.is_empty() => {
                    for child in children.as_array()?.iter().rev() { pending.push((child, path.clone())); }
                }
                _ => {
                    let value = node.get("value")?;
                    if value.as_f64().is_some_and(|number| number < 0.0) { return None; }
                    values.push(number(value)?);
                    paths.push(path);
                }
            }
        }
        let categories = paths.iter().map(|path| path.last().cloned().unwrap_or_default()).collect();
        Some(Self { categories: Some(categories), paths: Some(paths), series: vec![Series { title: None, values, x_values: None, sizes: None, names: None }], map: None })
    }

    fn matches(&self, kind: ChartKind) -> bool {
        if self.paths.is_some() { return false; }
        if matches!(kind, ChartKind::Pie) && self.series.len() > 1 { return false; }
        if !matches!(kind, ChartKind::Scatter | ChartKind::Bubble) && self.categories.is_none() { return false; }
        self.series.iter().all(|series| match kind {
            ChartKind::Scatter => series.x_values.is_some() && series.sizes.is_none(),
            ChartKind::Bubble => series.x_values.is_some() && (series.sizes.is_some() || series.values.iter().all(Option::is_none)),
            _ => self.categories.as_ref().is_some_and(|categories| series.x_values.is_none() && categories.len() == series.values.len()),
        })
    }
}

fn number(value: &Value) -> Option<Option<String>> {
    match value { Value::Null => Some(None), Value::Number(number) => Some(Some(number.to_string())), _ => None }
}

/// Keep occurrence indexes so each chart repeated by a loop can have different data.
pub(crate) fn queue(xml: &str, uses: &mut Vec<ChartUse>, value: Option<&Value>) {
    let Some(data) = value.and_then(ChartData::read) else { return };
    let (references, active) = namespaces::references(xml);
    if !active { return; }
    let Some(occurrence) = references.len().checked_sub(1) else { return };
    if let Some(used) = uses.iter_mut().find(|used| used.occurrence == occurrence) {
        used.data = data;
    } else {
        uses.push(ChartUse { occurrence, data });
    }
}

/// Make chart-title tags visible to the normal parser and its loop scopes.
pub(crate) fn prepare(parts: &mut [(String, Vec<u8>)], options: &FillOptions) -> BTreeMap<String, String> {
    if options.delimiters.start.is_empty() || options.delimiters.end.is_empty() { return BTreeMap::new(); }
    for (_, bytes) in parts.iter_mut() {
        if let Ok(xml) = std::str::from_utf8(bytes)
            && let Some(normalized) = namespaces::normalize(xml) { *bytes = normalized.into_bytes(); }
    }
    let sources: BTreeMap<_, _> = parts.iter().map(|(name, bytes)| (name.as_str(), bytes.as_slice())).collect();
    let titles: BTreeMap<_, _> = sources.iter().filter_map(|(name, bytes)| Some(((*name).to_string(), title_tag(std::str::from_utf8(bytes).ok()?, options)?))).collect();
    let mut changes = Vec::new();
    for (index, (name, bytes)) in parts.iter().enumerate() {
        let Ok(xml) = std::str::from_utf8(bytes) else { continue };
        let references = references(xml);
        if references.is_empty() { continue; }
        let relations = relationships(name, &sources);
        let mut edits = Vec::new();
        for start in find_starts(xml, "w:p") {
            let Some(end) = find_element_end(xml, start, "w:p") else { continue };
            let references = references.iter().copied().filter(|at| *at >= start && *at < end).collect::<Vec<_>>();
            if references.len() != 1 { continue; }
            let mut added = String::new();
            for chart in references {
                let Some(close) = xml[chart..].find('>').map(|at| chart + at + 1) else { continue };
                let Some(id) = read_attribute(&xml[chart..close], "r:id") else { continue };
                let Some(literal) = relations.get(&unescape(id)).and_then(|path| titles.get(path)) else { continue };
                let inner = literal.strip_prefix(&options.delimiters.start).and_then(|text| text.strip_suffix(&options.delimiters.end)).unwrap_or(literal).trim();
                let normalized = format!("{}{}{}", options.delimiters.start, inner, options.delimiters.end);
                let mut escaped = String::new(); escape(&normalized, &mut escaped);
                added.push_str(&format!("<w:r><w:t>{escaped}</w:t></w:r>"));
            }
            if !added.is_empty() { edits.push((end - "</w:p>".len(), added)); }
        }
        if edits.is_empty() { continue; }
        let mut changed = xml.to_string();
        for (at, added) in edits.into_iter().rev() { changed.insert_str(at, &added); }
        changes.push((index, changed.into_bytes()));
    }
    for (index, bytes) in changes { parts[index].1 = bytes; }
    titles
}

pub(crate) fn apply(parts: &mut Vec<(String, Cow<[u8]>)>, uses: &[(String, ChartUse)], titles: &BTreeMap<String, String>, mode: ChartDataMode) -> Vec<FillError> {
    let errors = fill_charts(parts, uses, titles, mode);
    // A chart left unfilled still shows its title, so its tag goes too.
    for (path, literal) in titles {
        let Some((_, bytes)) = parts.iter_mut().find(|(name, _)| name == path) else { continue };
        let Ok(xml) = std::str::from_utf8(bytes) else { continue };
        *bytes = Cow::Owned(strip_title_tag(xml, literal).into_bytes());
    }
    errors
}

struct ChartChange {
    owner_xml: String,
    path: String,
    chart: Vec<u8>,
    relations: Option<Vec<u8>>,
    workbook: Option<(String, Vec<u8>)>,
    types: Option<Vec<u8>>,
    id: String,
    target: String,
    format: ChartFormat,
}

fn fill_charts(parts: &mut Vec<(String, Cow<[u8]>)>, uses: &[(String, ChartUse)], titles: &BTreeMap<String, String>, mode: ChartDataMode) -> Vec<FillError> {
    let mut next = 1;
    let mut errors = Vec::new();
    for (part, used) in uses {
        let change = match chart_change(parts, part, used, titles, mode, &mut next) {
            Ok(change) => change,
            Err(error) => { if mode == ChartDataMode::Workbook { errors.push(error); } continue; }
        };
        if let Some((_, bytes)) = parts.iter_mut().find(|(name, _)| name == part) { *bytes = Cow::Owned(change.owner_xml.into_bytes()); }
        parts.push((change.path.clone(), Cow::Owned(change.chart)));
        if let Some(bytes) = change.relations { parts.push((relation_path(&change.path), Cow::Owned(bytes))); }
        if let Some(workbook) = change.workbook { parts.push((workbook.0, Cow::Owned(workbook.1))); }
        add_chart_relationship(parts, part, &change.id, &format!("<Relationship Id=\"{}\" Type=\"{}\" Target=\"{}\"/>", change.id, change.format.relationship(), change.target));
        if let Some(types) = change.types
            && let Some((_, bytes)) = parts.iter_mut().find(|(name, _)| name == CONTENT_TYPES_PART) { *bytes = Cow::Owned(types); }
    }
    errors
}

fn chart_change(parts: &[(String, Cow<[u8]>)], part: &str, used: &ChartUse, titles: &BTreeMap<String, String>, mode: ChartDataMode, next: &mut usize) -> Result<ChartChange, FillError> {
    let invalid = |message: &str| FillError::new(message).with_id(FillErrorId::ChartWorkbook).in_part(part);
    let sources: BTreeMap<_, _> = parts.iter().map(|(name, bytes)| (name.as_str(), bytes.as_ref())).collect();
    let relations = relationships(part, &sources);
    let xml = sources.get(part).and_then(|bytes| std::str::from_utf8(bytes).ok()).ok_or_else(|| invalid("Missing chart owner XML"))?;
    let start = references(xml).get(used.occurrence).copied().ok_or_else(|| invalid("Missing chart drawing"))?;
    let end = xml[start..].find('>').map(|at| start + at + 1).ok_or_else(|| invalid("Invalid chart drawing"))?;
    let id = unescape(read_attribute(&xml[start..end], "r:id").ok_or_else(|| invalid("Missing chart relationship ID"))?);
    let source_path = relations.get(&id).ok_or_else(|| invalid("Missing internal chart relationship"))?;
    let source = sources.get(source_path.as_str()).and_then(|bytes| std::str::from_utf8(bytes).ok()).ok_or_else(|| invalid("Missing chart XML"))?;
    let format = if find_starts(source, "cx:chartSpace").is_empty() { ChartFormat::Standard } else { ChartFormat::Extended };
    let filled = fill_chart(source, &used.data, titles.get(source_path).map(String::as_str)).ok_or_else(|| invalid("Unsupported chart type or data"))?;
    let folder = source_path.rsplit_once('/').map_or("", |(folder, _)| folder);
    let rels_path = relation_path(part);
    let occupied: BTreeSet<_> = sources.get(rels_path.as_str()).and_then(|bytes| std::str::from_utf8(bytes).ok()).map(namespaces::relations).unwrap_or_default().into_iter().map(|relation| relation.id).collect();
    let (path, id) = loop {
        let path = format!("{folder}/docxFillChart{next}.xml");
        let id = format!("rIdDocxFillChart{next}");
        *next += 1;
        if !sources.contains_key(path.as_str()) && !occupied.contains(id.as_str()) { break (path, id); }
    };
    let mut change = ChartChange {
        owner_xml: xml.to_string(), path: path.clone(), chart: filled.as_bytes().to_vec(),
        relations: sources.get(relation_path(source_path).as_str()).map(|bytes| bytes.to_vec()),
        workbook: None, types: None, id: id.clone(), target: String::new(), format,
    };
    let workbook_type = if mode == ChartDataMode::Workbook {
        #[cfg(feature = "chart-workbook")]
        {
            let prepared = workbook::prepare(&sources, source_path, source, &filled, &path).map_err(|error| error.in_part(source_path))?;
            change.chart = prepared.chart.into_bytes();
            change.relations = Some(prepared.relations);
            change.workbook = Some((prepared.path, prepared.bytes));
            prepared.content_type
        }
        #[cfg(not(feature = "chart-workbook"))]
        return Err(invalid("Workbook chart data needs the chart-workbook cargo feature"));
    } else { String::new() };
    change.owner_xml.replace_range(start..end, &crate::properties::set_attribute(&xml[start..end], "r:id", &id));
    let mut escaped = String::new(); escape(&relative_target(part, &path), &mut escaped);
    change.target = escaped;
    if let Some(bytes) = sources.get(CONTENT_TYPES_PART) {
        let types = std::str::from_utf8(bytes).map_err(|_| invalid("Invalid package content types"))?;
        let mut escaped = String::new(); escape(&path, &mut escaped);
        let updated = namespaces::append_type(types, &format!("<Override PartName=\"/{escaped}\" ContentType=\"{}\"/>{workbook_type}", format.content_type())).ok_or_else(|| invalid("Missing package content types root"))?;
        change.types = Some(updated.into_bytes());
    } else if mode == ChartDataMode::Workbook { return Err(invalid("Missing package content types")); }
    Ok(change)
}

fn fill_chart(xml: &str, data: &ChartData, title_tag: Option<&str>) -> Option<String> {
    if !find_starts(xml, "cx:chartSpace").is_empty() { return extended::fill(xml, data, title_tag); }
    if data.map.is_some() { return None; }
    let names: Vec<_> = tokenize(xml).ok()?.iter().filter_map(|token| match token {
        Token::Element { kind: ElementKind::Open | ElementKind::SelfClosing, name, .. } if name.starts_with("c:") && name.ends_with("Chart") => Some(*name),
        _ => None,
    }).collect();
    let [name] = names.as_slice() else { return None };
    let kind = ChartKind::read(name)?;
    if !data.matches(kind) { return None; }
    let order = kind.series_order();
    let plots = find_starts(xml, name);
    if plots.len() != 1 { return None; }
    let start = plots[0];
    let end = find_element_end(xml, start, name)?;
    let plot = &xml[start..end];
    let templates: Vec<_> = find_starts(plot, "c:ser").into_iter().filter_map(|start| find_element_end(plot, start, "c:ser").map(|end| (start, end))).filter(|&(start, end)| plot[start..end].ends_with("</c:ser>")).collect();
    let &(first, _) = templates.first()?;
    // Series hidden by a filter keep their ids, which must stay unique.
    let taken: BTreeSet<usize> = find_starts(plot, "c:idx").into_iter()
        .filter(|at| !templates.iter().any(|&(start, end)| start < *at && *at < end))
        .filter_map(|at| read_attribute(&plot[at..at + plot[at..].find('>')?], "val")?.parse().ok())
        .collect();
    let ids: Vec<usize> = (0..).filter(|id| !taken.contains(id)).take(data.series.len()).collect();
    let mut series = String::new();
    for ((index, value), id) in data.series.iter().enumerate().zip(ids) {
        let &(start, end) = templates.get(index).unwrap_or(&templates[0]);
        let template = &plot[start..end];
        let open = template.find('>')? + 1;
        let mut inner = template[open..template.len() - "</c:ser>".len()].to_string();
        // An added series takes the theme's next colour, not a copy of the first one's.
        if index >= templates.len() { inner = remove_children(&inner, &["c:spPr", "c:dPt", "c:extLst"]); }
        inner = set_child(&inner, "c:idx", &format!("<c:idx val=\"{id}\"/>"), order);
        inner = set_child(&inner, "c:order", &format!("<c:order val=\"{id}\"/>"), order);
        if let Some(title) = value.title.clone().or_else(|| series_title(template)) {
            let mut escaped = String::new(); escape(&title, &mut escaped);
            inner = set_child(&inner, "c:tx", &format!("<c:tx><c:v>{escaped}</c:v></c:tx>"), order);
        }
        if let Some(x_values) = &value.x_values {
            inner = set_numbers(&inner, template, "c:xVal", x_values, order);
            inner = set_numbers(&inner, template, "c:yVal", &value.values, order);
            if matches!(kind, ChartKind::Bubble) {
                let empty = vec![None; value.values.len()];
                inner = set_numbers(&inner, template, "c:bubbleSize", value.sizes.as_deref().unwrap_or(&empty), order);
            }
            if let Some(names) = &value.names { inner = set_labels(&inner, names, order); }
        } else {
            let points: Vec<_> = data.categories.as_ref()?.iter().cloned().map(Some).collect();
            inner = set_child(&inner, "c:cat", &format!("<c:cat>{}</c:cat>", literal(DataKind::Text, &points)), order);
            inner = set_numbers(&inner, template, "c:val", &value.values, order);
        }
        series.push_str(&format!("{}{}{}", &template[..open], inner, "</c:ser>"));
    }
    let mut changed = plot.to_string();
    for &(start, end) in templates.iter().rev() { changed.replace_range(start..end, ""); }
    changed.insert_str(first, &series);
    let mut out = format!("{}{}{}", &xml[..start], changed, &xml[end..]);
    // Literal chart data has no workbook to refresh from.
    for start in find_starts(&out, "c:externalData").into_iter().rev() {
        if let Some(end) = find_element_end(&out, start, "c:externalData") { out.replace_range(start..end, ""); }
    }
    if let Some(literal) = title_tag { out = strip_title_tag(&out, literal); }
    Some(out)
}

fn set_numbers(inner: &str, template: &str, name: &str, values: &[Option<String>], order: &[&str]) -> String {
    let format = find_starts(template, name).first().and_then(|&at| find_element_end(template, at, name).and_then(|end| element_text(&template[at..end], "c:formatCode"))).unwrap_or_else(|| "General".to_string());
    set_child(inner, name, &format!("<{name}>{}</{name}>", literal(DataKind::Number(&format), values)), order)
}

fn set_labels(inner: &str, names: &[Option<String>], order: &[&str]) -> String {
    let mut labels = String::new();
    if let Some(start) = find_starts(inner, "c:dLbls").first().copied()
        && let Some(end) = find_element_end(inner, start, "c:dLbls") {
        let element = &inner[start..end];
        if let Some(open) = element.find('>') && element.ends_with("</c:dLbls>") { labels = element[open + 1..element.len() - "</c:dLbls>".len()].to_string(); }
    }
    for (index, name) in names.iter().enumerate() {
        let Some(name) = name else { continue };
        let mut escaped = String::new(); escape(name, &mut escaped);
        let text = format!(r#"<c:tx><c:rich xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>{escaped}</a:t></a:r></a:p></c:rich></c:tx>"#);
        let range = find_starts(&labels, "c:dLbl").into_iter().find_map(|start| {
            let end = find_element_end(&labels, start, "c:dLbl")?;
            let index_start = *find_starts(&labels[start..end], "c:idx").first()? + start;
            (read_attribute(&labels[index_start..end], "val").and_then(|value| value.parse::<usize>().ok()) == Some(index)).then_some((start, end))
        });
        let mut label = range.map_or_else(|| format!("<c:idx val=\"{index}\"/>"), |(start, end)| {
            let open = labels[start..end].find('>').unwrap_or(0) + start + 1;
            labels[open..end - "</c:dLbl>".len()].to_string()
        });
        label = set_child(&label, "c:tx", &text, LABEL_ORDER);
        for name in ["c:showLegendKey", "c:showVal", "c:showCatName", "c:showSerName", "c:showPercent", "c:showBubbleSize"] {
            label = set_child(&label, name, &format!("<{name} val=\"0\"/>"), LABEL_ORDER);
        }
        let label = format!("<c:dLbl>{label}</c:dLbl>");
        if let Some((start, end)) = range { labels.replace_range(start..end, &label); } else { labels.insert_str(0, &label); }
    }
    set_child(inner, "c:dLbls", &format!("<c:dLbls>{labels}</c:dLbls>"), order)
}

/// `xml` without its own `names` children; descendants of other children stay.
fn remove_children(xml: &str, names: &[&str]) -> String {
    let Ok(tokens) = tokenize(xml) else { return xml.to_string() };
    let mut out = String::new();
    let (mut depth, mut skip_from) = (0usize, None);
    for token in tokens {
        let raw = match &token { Token::Element { raw, .. } => *raw, Token::Text(text) => *text };
        if let Token::Element { kind, name, .. } = &token {
            let is_removed = depth == 0 && names.contains(name);
            match kind {
                ElementKind::Open => { if is_removed { skip_from = Some(depth); } depth += 1; }
                ElementKind::Close => { depth -= 1; if skip_from == Some(depth) { skip_from = None; continue; } }
                ElementKind::SelfClosing if is_removed => continue,
                _ => {}
            }
        }
        if skip_from.is_none() { out.push_str(raw); }
    }
    out
}

fn literal(kind: DataKind, values: &[Option<String>]) -> String {
    let (name, format) = match kind { DataKind::Text => ("str", None), DataKind::Number(format) => ("num", Some(format)) };
    let mut xml = format!("<c:{name}Lit>");
    if let Some(format) = format {
        let mut escaped = String::new(); escape(format, &mut escaped);
        xml.push_str(&format!("<c:formatCode>{escaped}</c:formatCode>"));
    }
    xml.push_str(&format!("<c:ptCount val=\"{}\"/>", values.len()));
    for (index, value) in values.iter().enumerate() {
        let Some(value) = value else { continue };
        let mut escaped = String::new(); escape(value, &mut escaped);
        xml.push_str(&format!("<c:pt idx=\"{index}\"><c:v>{escaped}</c:v></c:pt>"));
    }
    xml.push_str(&format!("</c:{name}Lit>"));
    xml
}

fn series_title(xml: &str) -> Option<String> {
    let start = *find_starts(xml, "c:tx").first()?;
    let end = find_element_end(xml, start, "c:tx")?;
    element_text(&xml[start..end], "c:v")
}

fn element_text(xml: &str, name: &str) -> Option<String> {
    let start = *find_starts(xml, name).first()?;
    let end = find_element_end(xml, start, name)?;
    let open = xml[start..end].find('>')? + start + 1;
    let close = xml[start..end].rfind('<')? + start;
    if close < open { return None; }
    Some(unescape(&xml[open..close]))
}

fn title_tag(xml: &str, options: &FillOptions) -> Option<String> {
    let name = if find_starts(xml, "cx:chartSpace").is_empty() { "c:title" } else { "cx:title" };
    let start = *find_starts(xml, name).first()?;
    let end = find_element_end(xml, start, name)?;
    let text: String = text_spans(&xml[start..end]).into_iter().map(|(_, _, text)| text).collect();
    let open = &options.delimiters.start;
    let close = &options.delimiters.end;
    let mut from = 0;
    while let Some(start) = text[from..].find(open).map(|start| from + start) {
        let end = text[start + open.len()..].find(close)? + start + open.len();
        let inner = text[start + open.len()..end].trim();
        if inner.starts_with('$') && !inner[1..].trim().is_empty() && !options.module_prefixes.iter().any(|prefix| inner.starts_with(prefix)) {
            return Some(text[start..end + close.len()].to_string());
        }
        from = end + close.len();
    }
    None
}

fn text_spans(xml: &str) -> Vec<(usize, usize, String)> {
    let mut spans = Vec::new();
    let mut offset = 0;
    let mut in_text = false;
    for token in tokenize(xml).unwrap_or_default() {
        match token {
            Token::Element { kind, name, raw } => { if matches!(name, "a:t" | "c:v" | "cx:v") { in_text = kind == ElementKind::Open; } offset += raw.len(); }
            Token::Text(text) => { if in_text { spans.push((offset, offset + text.len(), unescape(text))); } offset += text.len(); }
        }
    }
    spans
}

fn strip_title_tag(xml: &str, literal: &str) -> String {
    // The title's tag has been moved into its drawing paragraph. Clear its text in the clone.
    let name = if find_starts(xml, "cx:chartSpace").is_empty() { "c:title" } else { "cx:title" };
    let Some(start) = find_starts(xml, name).first().copied() else { return xml.to_string() };
    let Some(end) = find_element_end(xml, start, name) else { return xml.to_string() };
    let spans = text_spans(&xml[start..end]);
    let text: String = spans.iter().map(|(_, _, text)| text.as_str()).collect();
    let Some(tag_start) = text.find(literal) else { return xml.to_string() };
    let tag_end = tag_start + literal.len();
    let mut out = xml.to_string();
    let mut consumed = text.len();
    for (from, to, value) in spans.into_iter().rev() {
        consumed -= value.len();
        let remove_start = tag_start.saturating_sub(consumed).min(value.len());
        let remove_end = tag_end.saturating_sub(consumed).min(value.len());
        let mut changed = value;
        changed.replace_range(remove_start..remove_end, "");
        let mut escaped = String::new(); escape(&changed, &mut escaped);
        out.replace_range(start + from..start + to, &escaped);
    }
    out
}

fn relationships(part: &str, sources: &BTreeMap<&str, &[u8]>) -> BTreeMap<String, String> {
    let path = relation_path(part);
    let Some(xml) = sources.get(path.as_str()).and_then(|bytes| std::str::from_utf8(bytes).ok()) else { return BTreeMap::new() };
    let folder = part.rsplit_once('/').map_or("", |(folder, _)| folder);
    namespaces::relations(xml).into_iter().filter_map(|relation| {
        if !matches!(relation.kind.as_str(), CHART_RELATIONSHIP | EXTENDED_RELATIONSHIP) || relation.external { return None; }
        Some((relation.id, resolve(folder, &relation.target)?))
    }).collect()
}

fn relation_path(part: &str) -> String {
    let (folder, file) = part.rsplit_once('/').unwrap_or(("", part));
    if folder.is_empty() { format!("_rels/{file}.rels") } else { format!("{folder}/_rels/{file}.rels") }
}

fn resolve(folder: &str, target: &str) -> Option<String> {
    let mut components = if target.starts_with('/') { Vec::new() } else { folder.split('/').filter(|part| !part.is_empty()).collect::<Vec<_>>() };
    for part in target.split('/') { match part { "" | "." => {}, ".." => { components.pop()?; }, _ => components.push(part) } }
    Some(components.join("/"))
}

fn relative_target(part: &str, target: &str) -> String {
    let folder = part.rsplit_once('/').map_or("", |(folder, _)| folder);
    let source: Vec<_> = folder.split('/').filter(|part| !part.is_empty()).collect();
    let destination: Vec<_> = target.split('/').collect();
    let shared = source.iter().zip(&destination).take_while(|(a, b)| a == b).count();
    std::iter::repeat_n("..", source.len() - shared).chain(destination[shared..].iter().copied()).collect::<Vec<_>>().join("/")
}

fn references(xml: &str) -> Vec<usize> {
    namespaces::references(xml).0
}

pub(crate) fn chart_parts(parts: &[crate::template::Part]) -> BTreeSet<usize> {
    let mut xml = String::new();
    let mut offsets = BTreeMap::new();
    for (index, part) in parts.iter().enumerate() {
        match part {
            crate::template::Part::Element { raw, .. } => { offsets.insert(xml.len(), index); xml.push_str(raw); }
            crate::template::Part::Text(text) => xml.push_str(text),
            crate::template::Part::Tag(_) => {},
        }
    }
    references(&xml).into_iter().filter_map(|at| offsets.get(&at).copied()).collect()
}

fn add_chart_relationship(parts: &mut Vec<(String, Cow<[u8]>)>, part: &str, id: &str, relationship: &str) {
    let path = relation_path(part);
    if let Some((_, bytes)) = parts.iter_mut().find(|(name, _)| *name == path) {
        let xml = String::from_utf8_lossy(bytes);
        if let Some(changed) = namespaces::append_relation(&xml, relationship) { *bytes = Cow::Owned(changed.into_bytes()); }
    } else { add_relationship(parts, part, id, relationship); }
}

/// Reuse chart data validation and cache rendering for a chart owned by an XLSX package.
pub(crate) fn fill_spreadsheet_chart(xml: &str, value: &Value) -> Result<String, FillError> {
    let normalized = namespaces::normalize_chart(xml);
    let xml = normalized.as_deref().unwrap_or(xml);
    let data = ChartData::read(value).ok_or_else(|| {
        FillError::new("Invalid spreadsheet chart data").with_id(FillErrorId::SpreadsheetCell)
    })?;
    let points: usize = data.series.iter().map(|series| series.values.len()).sum();
    if points > 1_000_000 {
        return Err(
            FillError::new("Spreadsheet chart exceeds one million points")
                .with_id(FillErrorId::SpreadsheetCell),
        );
    }
    fill_chart(xml, &data, None).ok_or_else(|| {
        FillError::new("Unsupported spreadsheet chart layout or data")
            .with_id(FillErrorId::SpreadsheetCell)
    })
}
