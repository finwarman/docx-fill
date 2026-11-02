use super::{ChartData, element_text};
use crate::properties::{find_element_end, find_starts, set_child};
use crate::xml::{escape, read_attribute, unescape};

const SERIES_ORDER: &[&str] = &["cx:tx", "cx:spPr", "cx:valueColors", "cx:valueColorPositions", "cx:dataPt", "cx:dataLabels", "cx:dataId", "cx:layoutPr", "cx:axisId", "cx:extLst"];

enum Layout { Hierarchy, Waterfall, Map }

pub(super) fn fill(xml: &str, data: &ChartData, title_tag: Option<&str>) -> Option<String> {
    let categories = data.categories.as_ref()?;
    let [series] = data.series.as_slice() else { return None };
    if series.x_values.is_some() || categories.len() != series.values.len() { return None; }
    let series_starts = find_starts(xml, "cx:series");
    let [start] = series_starts.as_slice() else { return None };
    let start = *start;
    let end = find_element_end(xml, start, "cx:series")?;
    let series_xml = &xml[start..end];
    let layout = match read_attribute(series_xml, "layoutId")? {
        "sunburst" | "treemap" => Layout::Hierarchy,
        "waterfall" => Layout::Waterfall,
        "regionMap" => Layout::Map,
        _ => return None,
    };
    if matches!(layout, Layout::Hierarchy) && series.values.iter().flatten().any(|value| value.parse::<f64>().map_or(true, |value| value < 0.0)) { return None; }
    if !matches!(layout, Layout::Hierarchy) && data.paths.is_some() { return None; }
    let id_start = *find_starts(series_xml, "cx:dataId").first()?;
    let id = read_attribute(&series_xml[id_start..], "val")?;
    let data_start = find_starts(xml, "cx:data").into_iter().find(|start| read_attribute(&xml[*start..], "id") == Some(id))?;
    let data_end = find_element_end(xml, data_start, "cx:data")?;
    let original = &xml[data_start..data_end];
    if matches!(layout, Layout::Map) && !matches_regions(original, categories) { return None; }
    let levels = match &data.paths {
        Some(paths) => {
            let depth = paths.iter().map(Vec::len).max().unwrap_or(1);
            (0..depth).rev().map(|level| paths.iter().map(|path| path.get(level).cloned().unwrap_or_default()).collect::<Vec<_>>()).collect::<Vec<_>>()
        }
        None => vec![categories.clone()],
    };
    let numeric_type = match layout { Layout::Hierarchy => "size", Layout::Waterfall => "val", Layout::Map => "colorVal" };
    let old_numeric = dimension(original, "cx:numDim", numeric_type).map(|(start, end)| &original[start..end]);
    let format = old_numeric.and_then(|xml| find_starts(xml, "cx:lvl").first().and_then(|start| read_attribute(&xml[*start..], "formatCode"))).unwrap_or("General");
    let mut format = unescape(format);
    let mut escaped = String::new(); escape(&format, &mut escaped); format = escaped;
    let numeric = format!("<cx:numDim type=\"{numeric_type}\"><cx:lvl ptCount=\"{}\" formatCode=\"{format}\">{}</cx:lvl></cx:numDim>", series.values.len(), number_points(&series.values));
    let mut updated = replace_dimension(original, "cx:numDim", numeric_type, &numeric);
    if !matches!(layout, Layout::Map) {
        let strings = levels.iter().map(|level| format!("<cx:lvl ptCount=\"{}\">{}</cx:lvl>", level.len(), string_points(level))).collect::<String>();
        updated = replace_dimension(&updated, "cx:strDim", "cat", &format!("<cx:strDim type=\"cat\">{strings}</cx:strDim>"));
    }
    let mut out = xml.to_string();
    out.replace_range(data_start..data_end, &updated);
    if let Some(title) = &series.title {
        let start = *find_starts(&out, "cx:series").first()?;
        let end = find_element_end(&out, start, "cx:series")?;
        let open = out[start..end].find('>')? + start + 1;
        let mut title_xml = String::new(); escape(title, &mut title_xml);
        let inner = set_child(&out[open..end - "</cx:series>".len()], "cx:tx", &format!("<cx:tx><cx:txData><cx:v>{title_xml}</cx:v></cx:txData></cx:tx>"), SERIES_ORDER);
        out.replace_range(open..end - "</cx:series>".len(), &inner);
    }
    // Literal dimensions no longer refer to the source workbook.
    for start in find_starts(&out, "cx:externalData").into_iter().rev() {
        if let Some(end) = find_element_end(&out, start, "cx:externalData") { out.replace_range(start..end, ""); }
    }
    if let Some(literal) = title_tag { out = super::strip_title_tag(&out, literal); }
    Some(out)
}

fn dimension(xml: &str, name: &str, kind: &str) -> Option<(usize, usize)> {
    find_starts(xml, name).into_iter().find_map(|start| {
        if read_attribute(&xml[start..], "type") != Some(kind) { return None; }
        Some((start, find_element_end(xml, start, name)?))
    })
}

fn replace_dimension(xml: &str, name: &str, kind: &str, replacement: &str) -> String {
    let mut out = xml.to_string();
    if let Some((start, end)) = dimension(xml, name, kind) { out.replace_range(start..end, replacement); }
    else if let Some(end) = find_starts(&out, "cx:extLst").first().copied().or_else(|| out.rfind("</cx:data>")) { out.insert_str(end, replacement); }
    out
}

fn number_points(values: &[Option<String>]) -> String {
    values.iter().enumerate().filter_map(|(index, value)| value.as_ref().map(|value| format!("<cx:pt idx=\"{index}\">{value}</cx:pt>"))).collect()
}

fn string_points(values: &[String]) -> String {
    values.iter().enumerate().map(|(index, value)| {
        let mut escaped = String::new(); escape(value, &mut escaped);
        format!("<cx:pt idx=\"{index}\">{escaped}</cx:pt>")
    }).collect()
}

fn matches_regions(xml: &str, wanted: &[String]) -> bool {
    let Some((start, end)) = dimension(xml, "cx:strDim", "cat") else { return false };
    let xml = &xml[start..end];
    let Some(start) = find_starts(xml, "cx:lvl").first().copied() else { return false };
    if read_attribute(&xml[start..], "ptCount").and_then(|value| value.parse::<usize>().ok()) != Some(wanted.len()) { return false; }
    let Some(end) = find_element_end(xml, start, "cx:lvl") else { return false };
    let xml = &xml[start..end];
    let mut found = vec![String::new(); wanted.len()];
    for start in find_starts(xml, "cx:pt") {
        let Some(end) = find_element_end(xml, start, "cx:pt") else { return false };
        let Some(index) = read_attribute(&xml[start..end], "idx").and_then(|value| value.parse::<usize>().ok()).filter(|index| *index < found.len()) else { return false };
        let Some(value) = element_text(&xml[start..end], "cx:pt") else { return false };
        found[index] = value;
    }
    found == wanted
}
