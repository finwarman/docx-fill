use std::collections::{BTreeMap, BTreeSet};

use super::{dimension, replace_dimension, string_points};
use super::super::{MapData, namespaces};
use crate::properties::{find_element_end, find_starts};
use crate::xml::{ElementKind, Token, check_balanced, is_xml_char, read_attribute, tokenize, unescape};

const NAMESPACE: &str = "http://schemas.microsoft.com/office/drawing/2014/chartex";
const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NAMESPACE: &str = "http://www.w3.org/2000/xmlns/";
const ENTITY_TYPES: &str = "Address AdminDistrict AdminDistrict2 AdminDistrict3 Continent CountryRegion Locality Ocean Planet PostalCode Region Unsupported";

pub(super) struct Prepared {
    pub data: String,
    pub geography: Option<String>,
}

pub(super) fn prepare(xml: &str, categories: &[String], supplied: Option<&MapData>) -> Option<Prepared> {
    if let Some(map) = supplied {
        let geography = validate(&map.geography, &map.ids)?;
        if !supported_dimensions(xml) { return None; }
        let data = replace_strings(xml, "cat", categories);
        return Some(Prepared { data: replace_strings(&data, "entityId", &map.ids), geography: Some(geography) });
    }
    let original = strings(xml, "cat")?;
    if original == categories { return Some(Prepared { data: xml.to_string(), geography: None }); }
    let positions: BTreeMap<_, _> = original.iter().enumerate().map(|(index, name)| (name, index)).collect();
    if positions.len() != original.len() || categories.iter().collect::<BTreeSet<_>>().len() != categories.len() || !supported_dimensions(xml) { return None; }
    let ids = strings(xml, "entityId")?;
    if ids.len() != original.len() || ids.iter().any(String::is_empty) || ids.iter().collect::<BTreeSet<_>>().len() != ids.len() { return None; }
    let reordered = categories.iter().map(|name| positions.get(name).map(|index| ids[*index].clone())).collect::<Option<Vec<_>>>()?;
    let data = replace_strings(xml, "cat", categories);
    Some(Prepared { data: replace_strings(&data, "entityId", &reordered), geography: None })
}

fn supported_dimensions(xml: &str) -> bool {
    [("cx:strDim", &["cat", "entityId"][..]), ("cx:numDim", &["colorVal"][..])].iter().all(|(element, kinds)| {
        let mut seen = BTreeSet::new();
        find_starts(xml, element).iter().all(|start| read_attribute(&xml[*start..], "type").is_some_and(|kind| kinds.contains(&kind) && seen.insert(kind)))
    })
}

fn replace_strings(xml: &str, kind: &str, values: &[String]) -> String {
    replace_dimension(xml, "cx:strDim", kind, &format!("<cx:strDim type=\"{kind}\"><cx:lvl ptCount=\"{}\">{}</cx:lvl></cx:strDim>", values.len(), string_points(values)))
}

fn strings(xml: &str, kind: &str) -> Option<Vec<String>> {
    let (start, end) = dimension(xml, "cx:strDim", kind)?;
    let xml = &xml[start..end];
    let levels = find_starts(xml, "cx:lvl");
    let [start] = levels.as_slice() else { return None };
    let end = find_element_end(xml, *start, "cx:lvl")?;
    let xml = &xml[*start..end];
    let count = read_attribute(xml, "ptCount")?.parse::<usize>().ok()?;
    if count > xml.len() { return None; }
    let mut values = vec![None; count];
    for start in find_starts(xml, "cx:pt") {
        let end = find_element_end(xml, start, "cx:pt")?;
        let index = read_attribute(&xml[start..end], "idx")?.parse::<usize>().ok()?;
        let value = values.get_mut(index)?;
        if value.is_some() { return None; }
        *value = super::super::element_text(&xml[start..end], "cx:pt");
    }
    values.into_iter().collect()
}

struct Rule {
    attributes: &'static str,
    children: &'static str,
    repeated: bool,
}

fn rule(name: &str) -> Option<Rule> {
    let (attributes, children, repeated) = match name.strip_prefix("cx:")? {
        "geography" => ("projectionType viewedRegionType !cultureLanguage !cultureRegion !attribution", "geoCache", false),
        "geoCache" => ("!provider", "binary clear", false),
        "clear" => ("", "geoLocationQueryResults geoDataEntityQueryResults geoDataPointToEntityQueryResults geoChildEntitiesQueryResults geoParentEntitiesQueryResults", false),
        "geoLocationQueryResults" => ("", "geoLocationQueryResult", true),
        "geoLocationQueryResult" => ("", "geoLocationQuery geoLocations", false),
        "geoLocationQuery" => ("countryRegion adminDistrict1 adminDistrict2 postalCode !entityType", "", false),
        "geoLocations" => ("", "geoLocation", true),
        "geoLocation" => ("latitude longitude !entityName !entityType", "address", false),
        "address" => ("address address1 countryRegion adminDistrict1 adminDistrict2 postalCode locality isoCountryCode", "", false),
        "geoDataEntityQueryResults" => ("", "geoDataEntityQueryResult", true),
        "geoDataEntityQueryResult" => ("", "geoDataEntityQuery geoData", false),
        "geoDataEntityQuery" => ("!entityType !entityId", "", false),
        "geoData" => ("!entityName !entityId !east !west !north !south", "geoPolygons copyrights", false),
        "geoPolygons" => ("", "geoPolygon", true),
        "geoPolygon" => ("!polygonId !numPoints !pcaRings", "", false),
        "copyrights" => ("", "copyright", true),
        "geoDataPointToEntityQueryResults" => ("", "geoDataPointToEntityQueryResult", true),
        "geoDataPointToEntityQueryResult" => ("", "geoDataPointQuery geoDataPointToEntityQuery", false),
        "geoDataPointQuery" => ("!entityType !latitude !longitude", "", false),
        "geoDataPointToEntityQuery" => ("!entityType !entityId", "", false),
        "geoChildEntitiesQueryResults" => ("", "geoChildEntitiesQueryResult", true),
        "geoChildEntitiesQueryResult" => ("", "geoChildEntitiesQuery geoChildEntities", false),
        "geoChildEntitiesQuery" => ("!entityId", "geoChildTypes", false),
        "geoChildTypes" => ("", "entityType", true),
        "geoChildEntities" => ("", "geoHierarchyEntity", true),
        "geoHierarchyEntity" => ("!entityName !entityId !entityType", "", false),
        "geoParentEntitiesQueryResults" => ("", "geoParentEntitiesQueryResult", true),
        "geoParentEntitiesQueryResult" => ("", "geoParentEntitiesQuery geoEntity geoParentEntity", false),
        "geoParentEntitiesQuery" | "geoParentEntity" => ("!entityId", "", false),
        "geoEntity" => ("!entityName !entityType", "", false),
        "binary" | "copyright" | "entityType" => ("", "", false),
        _ => return None,
    };
    Some(Rule { attributes, children, repeated })
}

struct Frame<'a> {
    name: &'a str,
    children: Vec<&'a str>,
    text: String,
    entity: Option<String>,
}

fn validate(xml: &str, ids: &[String]) -> Option<String> {
    // Normalize within a ChartEx root, then validate only the supplied geography subtree.
    let wrapped = format!("<cx:chartSpace xmlns:cx=\"{NAMESPACE}\">{xml}</cx:chartSpace>");
    check_balanced(&tokenize(&wrapped).ok()?).ok()?;
    let wrapped = namespaces::normalize(&wrapped).unwrap_or(wrapped);
    let xml = &wrapped[wrapped.find('>')? + 1..wrapped.len() - "</cx:chartSpace>".len()];
    let mut frames: Vec<Frame> = Vec::new();
    let mut roots = 0;
    let mut binary = false;
    let mut geometry = BTreeSet::new();
    let mut entities = BTreeSet::new();
    for token in tokenize(xml).ok()? {
        match token {
            Token::Element { kind: ElementKind::Other, raw, .. } if raw.starts_with("<!--") => {}
            Token::Element { kind: ElementKind::Other, .. } => return None,
            Token::Text(text) => {
                let text = decoded(text)?;
                if let Some(frame) = frames.last_mut() { frame.text.push_str(&text); }
                else if !text.trim_matches(xml_space).is_empty() { return None; }
            }
            Token::Element { kind: ElementKind::Close, name, .. } => {
                let frame = frames.pop()?;
                if frame.name != name || !finish(&frame)? { return None; }
            }
            Token::Element { kind, name, raw } => {
                let current = rule(name)?;
                let attributes = attributes(raw, name)?;
                if !check_attributes(&current, &attributes) { return None; }
                if let Some(parent) = frames.last_mut() {
                    let parent_rule = rule(parent.name)?;
                    let children = parent_rule.children.split_whitespace().collect::<Vec<_>>();
                    let index = children.iter().position(|child| name == format!("cx:{child}"))?;
                    if let Some(previous) = parent.children.last() {
                        let previous_index = children.iter().position(|child| *previous == format!("cx:{child}"))?;
                        if index < previous_index || index == previous_index && !parent_rule.repeated { return None; }
                    }
                    parent.children.push(name);
                } else {
                    roots += 1;
                    if roots != 1 || name != "cx:geography" { return None; }
                }
                let entity = attributes.get("entityId").cloned();
                if name == "cx:geoData" && !entities.insert(entity.clone()?) { return None; }
                if name == "cx:geoPolygon" {
                    let parent = frames.iter().rev().find(|frame| frame.name == "cx:geoData")?;
                    geometry.insert(parent.entity.clone()?);
                }
                if name == "cx:binary" { binary = true; }
                let frame = Frame { name, children: Vec::new(), text: String::new(), entity };
                if kind == ElementKind::Open { frames.push(frame); }
                else if !finish(&frame)? { return None; }
            }
        }
    }
    if roots != 1 || !frames.is_empty() || !binary && ids.iter().any(|id| !geometry.contains(id)) { return None; }
    Some(xml.to_string())
}

fn finish(frame: &Frame) -> Option<bool> {
    match frame.name {
        "cx:geography" => Some(frame.children == ["cx:geoCache"] && frame.text.trim_matches(xml_space).is_empty()),
        "cx:geoCache" => Some(frame.children.len() == 1 && frame.text.trim_matches(xml_space).is_empty()),
        "cx:binary" => Some(base64(&frame.text)),
        "cx:entityType" => Some(ENTITY_TYPES.split_whitespace().any(|kind| kind == frame.text.trim_matches(xml_space))),
        "cx:copyright" => Some(true),
        _ => Some(frame.text.trim_matches(xml_space).is_empty()),
    }
}

fn base64(text: &str) -> bool {
    let bytes = text.bytes().filter(|byte| !byte.is_ascii_whitespace()).collect::<Vec<_>>();
    if bytes.is_empty() || bytes.len() % 4 != 0 { return false; }
    let length = bytes.len();
    let padding = bytes.iter().rev().take_while(|byte| **byte == b'=').count();
    padding <= 2 && bytes[..length - padding].iter().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/'))
}

fn check_attributes(rule: &Rule, values: &BTreeMap<&str, String>) -> bool {
    let allowed = rule.attributes.split_whitespace().collect::<Vec<_>>();
    if allowed.iter().any(|name| name.starts_with('!') && values.get(&name[1..]).is_none_or(|value| value.trim().is_empty())) { return false; }
    values.iter().all(|(name, value)| {
        if let Some(prefix) = name.strip_prefix("xmlns:") {
            return prefix != "xmlns" && !value.is_empty() && value != XMLNS_NAMESPACE && (prefix == "xml") == (value == XML_NAMESPACE);
        }
        if *name == "xmlns" { return value != XMLNS_NAMESPACE && value != XML_NAMESPACE; }
        if !allowed.iter().any(|allowed| allowed.trim_start_matches('!') == *name) { return false; }
        match *name {
            "latitude" | "north" | "south" => value.parse::<f64>().ok().is_some_and(|number| number.is_finite() && (-90.0..=90.0).contains(&number)),
            "longitude" | "east" | "west" => value.parse::<f64>().ok().is_some_and(|number| number.is_finite() && (-180.0..=180.0).contains(&number)),
            "numPoints" => value.parse::<u64>().is_ok_and(|number| number > 0),
            "entityType" => ENTITY_TYPES.split_whitespace().any(|kind| kind == value),
            "projectionType" => matches!(value.as_str(), "mercator" | "miller" | "robinson" | "albers"),
            "viewedRegionType" => matches!(value.as_str(), "dataOnly" | "postalCode" | "county" | "state" | "countryRegion" | "countryRegionList" | "world"),
            _ => true,
        }
    })
}

fn attributes<'a>(raw: &'a str, name: &str) -> Option<BTreeMap<&'a str, String>> {
    let mut rest = &raw[name.len() + 1..];
    let mut values = BTreeMap::new();
    loop {
        rest = rest.trim_start_matches(xml_space);
        if matches!(rest, ">" | "/>") { return Some(values); }
        let end = rest.find(|character: char| xml_space(character) || character == '=')?;
        let name = &rest[..end];
        rest = rest[end..].trim_start_matches(xml_space).strip_prefix('=')?.trim_start_matches(xml_space);
        let quote = rest.chars().next()?;
        if quote != '\'' && quote != '"' { return None; }
        rest = &rest[1..];
        let end = rest.find(quote)?;
        let value = &rest[..end];
        if value.contains('<') || values.insert(name, decoded(value)?).is_some() { return None; }
        rest = &rest[end + 1..];
        if !rest.starts_with(xml_space) && !matches!(rest, ">" | "/>") { return None; }
    }
}

fn decoded(text: &str) -> Option<String> {
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        rest = &rest[at..];
        let end = rest.find(';')? + 1;
        if unescape(&rest[..end]) == rest[..end] { return None; }
        rest = &rest[end..];
    }
    let value = unescape(text);
    value.chars().all(is_xml_char).then_some(value)
}

fn xml_space(character: char) -> bool { matches!(character, ' ' | '\t' | '\r' | '\n') }
