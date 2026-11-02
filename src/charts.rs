use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::properties::{find_element_end, find_starts, set_child};
use crate::xml::{ElementKind, Token, escape, read_attribute, tokenize, unescape};
use crate::{FillOptions, CONTENT_TYPES_PART, add_relationship, insert_before_last};

const CHART_RELATIONSHIP: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart";
const CHART_CONTENT_TYPE: &str = "application/vnd.openxmlformats-officedocument.drawingml.chart+xml";
const SERIES_ORDER: &[&str] = &["c:idx", "c:order", "c:tx", "c:spPr", "c:invertIfNegative", "c:pictureOptions", "c:dPt", "c:dLbls", "c:trendline", "c:errBars", "c:cat", "c:val", "c:shape", "c:extLst"];

#[derive(Debug)]
pub(crate) struct ChartUse {
    occurrence: usize,
    data: ChartData,
}

#[derive(Debug)]
struct ChartData {
    categories: Vec<String>,
    series: Vec<Series>,
}

#[derive(Debug)]
struct Series {
    title: Option<String>,
    values: Vec<Option<String>>,
}

enum DataKind<'a> { Text, Number(&'a str) }

impl ChartData {
    fn read(value: &Value) -> Option<Self> {
        let categories = value.get("categories")?.as_array()?.iter().map(|value| match value {
            Value::String(text) => Some(text.clone()),
            Value::Number(number) => Some(number.to_string()),
            Value::Null => Some(String::new()),
            _ => None,
        }).collect::<Option<Vec<_>>>()?;
        let mut series = Vec::new();
        for value in value.get("series")?.as_array()? {
            let title = match value.get("title") { None | Some(Value::Null) => None, Some(title) => Some(title.as_str()?.to_string()) };
            let values = value.get("data")?.as_array()?.iter().map(|value| match value {
                Value::Null => Some(None),
                Value::Number(number) => Some(Some(number.to_string())),
                _ => None,
            }).collect::<Option<Vec<_>>>()?;
            if values.len() != categories.len() { return None; }
            series.push(Series { title, values });
        }
        Some(Self { categories, series })
    }
}

/// Keep occurrence indexes so each chart repeated by a loop can have different data.
pub(crate) fn queue(xml: &str, uses: &mut Vec<ChartUse>, value: Option<&Value>) {
    let Some(data) = value.and_then(ChartData::read) else { return };
    let Some(occurrence) = find_starts(xml, "c:chart").len().checked_sub(1) else { return };
    if let Some(used) = uses.iter_mut().find(|used| used.occurrence == occurrence) {
        used.data = data;
    } else {
        uses.push(ChartUse { occurrence, data });
    }
}

/// Make chart-title tags visible to the normal parser and its loop scopes.
pub(crate) fn prepare(parts: &mut [(String, Vec<u8>)], options: &FillOptions) -> BTreeMap<String, String> {
    if options.delimiters.start.is_empty() || options.delimiters.end.is_empty() { return BTreeMap::new(); }
    let sources: BTreeMap<_, _> = parts.iter().map(|(name, bytes)| (name.as_str(), bytes.as_slice())).collect();
    let titles: BTreeMap<_, _> = sources.iter().filter_map(|(name, bytes)| Some(((*name).to_string(), title_tag(std::str::from_utf8(bytes).ok()?, options)?))).collect();
    let mut changes = Vec::new();
    for (index, (name, bytes)) in parts.iter().enumerate() {
        let Ok(xml) = std::str::from_utf8(bytes) else { continue };
        if !xml.contains("<c:chart ") { continue; }
        let relations = relationships(name, &sources);
        let mut edits = Vec::new();
        for start in find_starts(xml, "w:p") {
            let Some(end) = find_element_end(xml, start, "w:p") else { continue };
            let paragraph = &xml[start..end];
            if find_starts(paragraph, "c:chart").len() != 1 { continue; }
            let mut added = String::new();
            for chart in find_starts(paragraph, "c:chart") {
                let Some(close) = paragraph[chart..].find('>').map(|at| chart + at + 1) else { continue };
                let Some(id) = read_attribute(&paragraph[chart..close], "r:id") else { continue };
                let Some(literal) = relations.get(id).and_then(|path| titles.get(path)) else { continue };
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

pub(crate) fn apply(parts: &mut Vec<(String, Cow<[u8]>)>, uses: &[(String, ChartUse)], titles: &BTreeMap<String, String>) {
    fill_charts(parts, uses, titles);
    // A chart left unfilled still shows its title, so its tag goes too.
    for (path, literal) in titles {
        let Some((_, bytes)) = parts.iter_mut().find(|(name, _)| name == path) else { continue };
        let Ok(xml) = std::str::from_utf8(bytes) else { continue };
        *bytes = Cow::Owned(strip_title_tag(xml, literal).into_bytes());
    }
}

fn fill_charts(parts: &mut Vec<(String, Cow<[u8]>)>, uses: &[(String, ChartUse)], titles: &BTreeMap<String, String>) {
    let mut next = 1;
    for (part, used) in uses {
        let sources: BTreeMap<_, _> = parts.iter().map(|(name, bytes)| (name.as_str(), bytes.as_ref())).collect();
        let relations = relationships(part, &sources);
        let Some(bytes) = sources.get(part.as_str()) else { continue };
        let Ok(xml) = std::str::from_utf8(bytes) else { continue };
        let Some(start) = find_starts(xml, "c:chart").get(used.occurrence).copied() else { continue };
        let Some(end) = xml[start..].find('>').map(|at| start + at + 1) else { continue };
        let Some(id) = read_attribute(&xml[start..end], "r:id") else { continue };
        let Some(source_path) = relations.get(id) else { continue };
        let Some(source) = sources.get(source_path.as_str()).and_then(|bytes| std::str::from_utf8(bytes).ok()) else { continue };
        let Some(filled) = fill_chart(source, &used.data, titles.get(source_path).map(String::as_str)) else { continue };
        let folder = source_path.rsplit_once('/').map_or("", |(folder, _)| folder);
        let rels_path = relation_path(part);
        let occupied: BTreeSet<_> = sources.get(rels_path.as_str()).and_then(|bytes| std::str::from_utf8(bytes).ok()).and_then(|xml| tokenize(xml).ok()).unwrap_or_default().into_iter().filter_map(|token| match token {
            Token::Element { name: "Relationship", raw, .. } => read_attribute(raw, "Id"),
            _ => None,
        }).collect();
        let (path, id) = loop {
            let path = format!("{folder}/docxFillChart{next}.xml");
            let id = format!("rIdDocxFillChart{next}");
            next += 1;
            if !sources.contains_key(path.as_str()) && !occupied.contains(id.as_str()) { break (path, id); }
        };
        let source_rels = relation_path(source_path);
        let copied_rels = sources.get(source_rels.as_str()).map(|bytes| bytes.to_vec());
        let target = relative_target(part, &path);
        let mut changed = xml.to_string();
        changed.replace_range(start..end, &crate::properties::set_attribute(&xml[start..end], "r:id", &id));
        if let Some((_, bytes)) = parts.iter_mut().find(|(name, _)| name == part) { *bytes = Cow::Owned(changed.into_bytes()); }
        parts.push((path.clone(), Cow::Owned(filled.into_bytes())));
        if let Some(bytes) = copied_rels { parts.push((relation_path(&path), Cow::Owned(bytes))); }
        let mut escaped = String::new(); escape(&target, &mut escaped);
        add_relationship(parts, part, &id, &format!("<Relationship Id=\"{id}\" Type=\"{CHART_RELATIONSHIP}\" Target=\"{escaped}\"/>"));
        if let Some((_, bytes)) = parts.iter_mut().find(|(name, _)| name == CONTENT_TYPES_PART) {
            let mut escaped = String::new(); escape(&path, &mut escaped);
            *bytes = Cow::Owned(insert_before_last(&String::from_utf8_lossy(bytes), "</Types>", &format!("<Override PartName=\"/{escaped}\" ContentType=\"{CHART_CONTENT_TYPE}\"/>")).into_bytes());
        }
    }
}

fn fill_chart(xml: &str, data: &ChartData, title_tag: Option<&str>) -> Option<String> {
    if tokenize(xml).ok()?.iter().any(|token| matches!(token, Token::Element { name, .. } if name.ends_with("Chart") && *name != "c:barChart")) { return None; }
    let plots = find_starts(xml, "c:barChart");
    if plots.len() != 1 { return None; }
    let start = plots[0];
    let end = find_element_end(xml, start, "c:barChart")?;
    let plot = &xml[start..end];
    let templates: Vec<_> = find_starts(plot, "c:ser").into_iter().filter_map(|start| find_element_end(plot, start, "c:ser").map(|end| (start, end))).filter(|&(start, end)| plot[start..end].ends_with("</c:ser>")).collect();
    let &(first, _) = templates.first()?;
    let mut series = String::new();
    for (index, value) in data.series.iter().enumerate() {
        let &(start, end) = templates.get(index).unwrap_or(&templates[0]);
        let template = &plot[start..end];
        let open = template.find('>')? + 1;
        let mut inner = template[open..template.len() - "</c:ser>".len()].to_string();
        // An added series takes the theme's next colour, not a copy of the first one's.
        if index >= templates.len() { inner = remove_children(&inner, &["c:spPr", "c:dPt", "c:extLst"]); }
        inner = set_child(&inner, "c:idx", &format!("<c:idx val=\"{index}\"/>"), SERIES_ORDER);
        inner = set_child(&inner, "c:order", &format!("<c:order val=\"{index}\"/>"), SERIES_ORDER);
        if let Some(title) = value.title.clone().or_else(|| series_title(template)) {
            let mut escaped = String::new(); escape(&title, &mut escaped);
            inner = set_child(&inner, "c:tx", &format!("<c:tx><c:v>{escaped}</c:v></c:tx>"), SERIES_ORDER);
        }
        let points: Vec<_> = data.categories.iter().cloned().map(Some).collect();
        inner = set_child(&inner, "c:cat", &format!("<c:cat>{}</c:cat>", literal(DataKind::Text, &points)), SERIES_ORDER);
        let format = find_starts(template, "c:val").first().and_then(|&at| element_text(&template[at..], "c:formatCode")).unwrap_or_else(|| "General".to_string());
        inner = set_child(&inner, "c:val", &format!("<c:val>{}</c:val>", literal(DataKind::Number(&format), &value.values)), SERIES_ORDER);
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
    let start = *find_starts(xml, "c:title").first()?;
    let end = find_element_end(xml, start, "c:title")?;
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
            Token::Element { kind, name, raw } => { if name == "a:t" { in_text = kind == ElementKind::Open; } offset += raw.len(); }
            Token::Text(text) => { if in_text { spans.push((offset, offset + text.len(), unescape(text))); } offset += text.len(); }
        }
    }
    spans
}

fn strip_title_tag(xml: &str, literal: &str) -> String {
    // The title's tag has been moved into its drawing paragraph. Clear its text in the clone.
    let Some(start) = find_starts(xml, "c:title").first().copied() else { return xml.to_string() };
    let Some(end) = find_element_end(xml, start, "c:title") else { return xml.to_string() };
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
    tokenize(xml).unwrap_or_default().into_iter().filter_map(|token| {
        let Token::Element { name: "Relationship", raw, .. } = token else { return None };
        if read_attribute(raw, "Type") != Some(CHART_RELATIONSHIP) || read_attribute(raw, "TargetMode") == Some("External") { return None; }
        Some((read_attribute(raw, "Id")?.to_string(), resolve(folder, &unescape(read_attribute(raw, "Target")?))?))
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
