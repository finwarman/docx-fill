use std::collections::BTreeMap;
use resvg::usvg::roxmltree::{Document, Node, ParsingOptions};

use super::{MAX_RENDER_DEPTH, MAX_SVG_NODES};

pub(super) const MAX_ORDER: u32 = 64;
pub(super) const MAX_SIGMA: f32 = 4096.0;
pub(super) const MAX_RADIUS: f32 = 16384.0;
type UrlSpans = Vec<(usize, usize, String)>;
const SVG_NAMESPACE: &str = "http://www.w3.org/2000/svg";
const XLINK_NAMESPACE: &str = "http://www.w3.org/1999/xlink";
const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";
const MAX_SELECTOR_BYTES: usize = 1024;
const MAX_CSS_MATCHES: usize = 1_000_000;
const MAX_CSS_BYTES: usize = 8 * 1024 * 1024;
// usvg lays text out in quadratic time.
const MAX_TEXT_BYTES: usize = 64 * 1024;
const RESOURCE_PROPERTIES: &[&str] = &["fill", "stroke", "filter", "mask", "clip-path", "marker", "marker-start", "marker-mid", "marker-end"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Inheritance { Default, Explicit }

#[derive(Clone, Copy, PartialEq, Eq)]
enum Normalization { Apply, Skip }

#[derive(Clone, Copy, PartialEq, Eq)]
enum Importance { Normal, Important }

#[derive(Clone, Copy)]
struct StyleValue<'a> { value: &'a str, importance: Importance }
type Styles<'a> = BTreeMap<&'static str, StyleValue<'a>>;
struct Ancestors<'a, 'b> { parent: Option<usize>, parents: &'b [Option<usize>], styles: &'b [Styles<'a>] }

const PAINTED: &[&str] = &["path", "rect", "circle", "ellipse", "polygon", "polyline", "line", "text", "tspan", "textPath", "use"];

pub(super) fn prepare(bytes: &[u8]) -> Option<Vec<u8>> {
    let prepared = inspect(bytes, Normalization::Apply)?;
    if prepared.len() > super::MAX_SVG_BYTES { return None; }
    if prepared != bytes { inspect(&prepared, Normalization::Skip)?; }
    Some(prepared)
}

fn inspect(bytes: &[u8], normalization: Normalization) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(bytes).ok()?;
    if text.contains("<!DOCTYPE") || text.contains("<!ENTITY") { return None; }
    let document = Document::parse_with_options(text, ParsingOptions { nodes_limit: MAX_SVG_NODES, ..Default::default() }).ok()?;
    let nodes = document.descendants().filter(Node::is_element).collect::<Vec<_>>();
    let positions = nodes.iter().enumerate().map(|(index, node)| (node.id().get(), index)).collect::<BTreeMap<_, _>>();
    let parents = nodes.iter().map(|node| node.parent_element().and_then(|parent| positions.get(&parent.id().get()).copied())).collect::<Vec<_>>();
    let mut max_depth = 1usize;
    for node in &nodes {
        let depth = node.ancestors().take(MAX_RENDER_DEPTH + 1).count();
        if depth > MAX_RENDER_DEPTH { return None; }
        max_depth = max_depth.max(depth);
    }
    let text_bytes = document.descendants()
        .filter(|node| node.is_text() && node.ancestors().any(|parent| parent.tag_name().name() == "text"))
        .try_fold(0usize, |total, node| total.checked_add(node.text().map_or(0, str::len)))?;
    if text_bytes > MAX_TEXT_BYTES { return None; }
    let mut ids = BTreeMap::new();
    for (index, node) in nodes.iter().enumerate() {
        for attribute in node.attributes().filter(|attribute| attribute.name() == "id") {
            if ids.insert(attribute.value(), index).is_some_and(|previous| previous != index) { return None; }
        }
    }
    let mut sheet = simplecss::StyleSheet::new();
    for node in &nodes {
        if node.tag_name().name() == "style" && matches!(node.attribute("type"), None | Some("text/css"))
            && let Some(text) = node.text() {
            clean_css(text)?;
            sheet.parse_more(text);
        }
    }
    let mut css_work = 0usize;
    for rule in &sheet.rules {
        let selector = rule.selector.to_string();
        if selector.len() > MAX_SELECTOR_BYTES { return None; }
        let parts = selector.split_whitespace().filter(|part| !matches!(*part, ">" | "+")).count().max(1);
        let cost = max_depth.checked_pow(u32::try_from(parts - 1).ok()?)?.checked_mul(nodes.len())?;
        css_work = css_work.checked_add(cost)?;
        if css_work > MAX_CSS_MATCHES { return None; }
    }
    let mut style_bytes = 0usize;
    let mut styles = Vec::new();
    for (index, node) in nodes.iter().enumerate() {
        let ancestors = Ancestors { parent: parents[index], parents: &parents, styles: &styles };
        let mut values = Styles::new();
        for attribute in node.attributes() {
            if !accepted_ns(attribute.namespace()) || values.contains_key(attribute.name()) { continue; }
            set_style(attribute.name(), attribute.value(), Importance::Normal, &ancestors, &mut values);
        }
        for rule in &sheet.rules {
            if !rule.selector.matches(&CssNode(*node)) { continue; }
            for declaration in &rule.declarations {
                if !RESOURCE_PROPERTIES.contains(&declaration.name) { continue; }
                style_bytes = style_bytes.checked_add(declaration.value.len())?;
                if style_bytes > MAX_CSS_BYTES { return None; }
                let importance = if declaration.important { Importance::Important } else { Importance::Normal };
                set_style(declaration.name, declaration.value, importance, &ancestors, &mut values);
            }
        }
        if let Some(inline) = node.attribute("style") {
            for declaration in simplecss::DeclarationTokenizer::from(inline) {
                let importance = if declaration.important { Importance::Important } else { Importance::Normal };
                set_style(declaration.name, declaration.value, importance, &ancestors, &mut values);
            }
        }
        styles.push(values);
    }
    let mut edges = vec![Vec::new(); nodes.len()];
    for (index, node) in nodes.iter().enumerate() {
        let name = node.tag_name().name();
        if matches!(name, "foreignObject" | "script") { return None; }
        validate_filter(*node)?;
        for child in node.children().filter(Node::is_element) { edges[index].push(positions[&child.id().get()]); }
        let href = node.attributes().find(|attribute| attribute.name() == "href" && attribute.namespace().is_none())
            .or_else(|| node.attributes().find(|attribute| attribute.name() == "href" && attribute.namespace() == Some(XLINK_NAMESPACE)))
            .map(|attribute| attribute.value());
        if name != "a" && let Some(href) = href {
            let href = href.trim();
            let embedded = matches!(name, "image" | "feImage") && href.get(..5).is_some_and(|scheme| scheme.eq_ignore_ascii_case("data:"));
            if !embedded && let Some(target) = ids.get(fragment(href)?) { edges[index].push(*target); }
        }
        for value in styles[index].values() {
            for target in references(value.value)? {
                if let Some(target) = ids.get(target.as_str()) { edges[index].push(*target); }
            }
        }
        // Paint is inherited into resource content as well as ordinary descendants.
        if PAINTED.contains(&name) {
            for property in ["fill", "stroke", "marker", "marker-start", "marker-mid", "marker-end"] {
                for target in inherited_paint(index, property, Inheritance::Default, &styles, &parents)? {
                    if let Some(target) = ids.get(target.as_str()) { edges[index].push(*target); }
                }
            }
        }
        for property in ["filter", "mask", "clip-path"] {
            for target in inherited_paint(index, property, Inheritance::Explicit, &styles, &parents)? {
                if let Some(target) = ids.get(target.as_str()) { edges[index].push(*target); }
            }
        }
        edges[index].sort_unstable();
        edges[index].dedup();
    }
    acyclic(&edges)?;
    if normalization == Normalization::Skip { return Some(bytes.to_vec()); }
    let mut edits = Vec::new();
    for node in &nodes {
        if node.tag_name().name() == "style" && let Some(css) = node.text() {
            let value = normalize_urls(css)?;
            if value != css {
                let range = node.range();
                let raw = &text[range.clone()];
                let first = crate::xml::tokenize(raw).ok()?.first().copied()?;
                let crate::xml::Token::Element { raw: opening, .. } = first else { return None };
                let closing = raw.rfind("</")?;
                let mut escaped = String::new();
                crate::xml::escape(&value, &mut escaped);
                edits.push((range.start + opening.len()..range.start + closing, escaped));
            }
        }
        for attribute in node.attributes().filter(|attribute| attribute.name() != "href") {
            let value = normalize_urls(attribute.value())?;
            if value == attribute.value() { continue; }
            let mut escaped = String::new();
            crate::xml::escape(&value, &mut escaped);
            edits.push((attribute.range_value(), escaped));
        }
    }
    let mut prepared = text.to_string();
    edits.sort_by_key(|(range, _)| range.start);
    for (range, value) in edits.into_iter().rev() { prepared.replace_range(range, &value); }
    Some(prepared.into_bytes())
}

fn accepted_ns(namespace: Option<&str>) -> bool {
    matches!(namespace, None | Some(SVG_NAMESPACE | XLINK_NAMESPACE | XML_NAMESPACE))
}

fn set_style<'a>(name: &str, value: &'a str, importance: Importance, ancestors: &Ancestors<'a, '_>, styles: &mut Styles<'a>) {
    if name == "marker" {
        for name in ["marker-start", "marker-mid", "marker-end"] { set_style(name, value, importance, ancestors, styles); }
        return;
    }
    let Some(name) = RESOURCE_PROPERTIES.iter().copied().find(|property| *property == name) else { return };
    if styles.get(name).is_some_and(|value| value.importance == Importance::Important) { return; }
    let value = if value == "inherit" {
        let mut parent = ancestors.parent;
        let mut found = None;
        while let Some(index) = parent {
            if let Some(value) = ancestors.styles[index].get(name) { found = Some(*value); break; }
            if matches!(name, "filter" | "clip-path" | "mask") { break; }
            parent = ancestors.parents[index];
        }
        found.unwrap_or(StyleValue { value: if name == "fill" { "black" } else { "none" }, importance: Importance::Normal })
    } else { StyleValue { value, importance } };
    styles.insert(name, value);
}

fn inherited_paint(mut index: usize, property: &str, inheritance: Inheritance, styles: &[Styles<'_>], parents: &[Option<usize>]) -> Option<Vec<String>> {
    loop {
        if let Some(value) = styles[index].get(property) { return references(value.value); }
        if inheritance == Inheritance::Explicit { return Some(Vec::new()); }
        index = match parents[index] { Some(parent) => parent, None => return Some(Vec::new()) };
    }
}

fn clean_css(value: &str) -> Option<String> {
    // Escaped resource names and context paint need a separate dependency graph.
    if value.contains('\\') || value.contains("context-fill") || value.contains("context-stroke") { return None; }
    let mut clean = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("/*") {
        clean.push_str(&rest[..start]);
        let end = rest[start + 2..].find("*/")? + start + 4;
        rest = &rest[end..];
    }
    clean.push_str(rest);
    Some(clean)
}

fn references(value: &str) -> Option<Vec<String>> { Some(urls(value)?.1.into_iter().map(|(_, _, target)| target).collect()) }

fn urls(value: &str) -> Option<(String, UrlSpans)> {
    let value = clean_css(value)?;
    let lower = value.to_ascii_lowercase();
    let mut rest = lower.as_str();
    let mut offset = 0;
    let mut out = Vec::new();
    while let Some(start) = rest.find("url") {
        let after = start + 3;
        let tail = rest[after..].trim_start();
        if !tail.starts_with('(') { offset += after; rest = &lower[offset..]; continue; }
        let open = after + rest[after..].len() - tail.len() + 1;
        let close = rest[open..].find(')')? + open;
        let mut target = value[offset + open..offset + close].trim();
        if target.starts_with(['\'', '"']) {
            let quote = target.chars().next()?;
            target = target.strip_prefix(quote)?.strip_suffix(quote)?;
        }
        out.push((offset + start, offset + close + 1, fragment(target)?.to_string()));
        offset += close + 1;
        rest = &lower[offset..];
    }
    Some((value, out))
}

fn normalize_urls(value: &str) -> Option<String> {
    let (mut clean, spans) = urls(value)?;
    if spans.is_empty() { return Some(value.to_string()); }
    for (start, end, target) in spans.into_iter().rev() { clean.replace_range(start..end, &format!("url(#{target})")); }
    Some(clean)
}

struct CssNode<'a, 'input>(Node<'a, 'input>);

impl simplecss::Element for CssNode<'_, '_> {
    fn parent_element(&self) -> Option<Self> { self.0.parent_element().map(Self) }
    fn prev_sibling_element(&self) -> Option<Self> { self.0.prev_sibling_element().map(Self) }
    fn has_local_name(&self, name: &str) -> bool { self.0.tag_name().name() == name }
    fn attribute_matches(&self, name: &str, operator: simplecss::AttributeOperator) -> bool { self.0.attribute(name).is_some_and(|value| operator.matches(value)) }
    fn pseudo_class_matches(&self, class: simplecss::PseudoClass) -> bool { matches!(class, simplecss::PseudoClass::FirstChild) && self.0.prev_sibling_element().is_none() }
}

fn fragment(value: &str) -> Option<&str> {
    let value = value.strip_prefix('#')?;
    (!value.is_empty() && !value.contains(['%', '\\', '\'', '"']) && !value.chars().any(char::is_whitespace)).then_some(value)
}

fn validate_filter(node: Node<'_, '_>) -> Option<()> {
    let bounded = |attribute: &str, max: f32| -> Option<()> {
        for value in node.attributes().filter(|item| item.name() == attribute).map(|item| item.value()) {
            let values = value.split(|character: char| character == ',' || character.is_ascii_whitespace()).filter(|part| !part.is_empty()).collect::<Vec<_>>();
            if values.is_empty() || values.len() > 2 { return None; }
            for value in values {
                let value = value.parse::<f32>().ok()?;
                if !value.is_finite() || value < 0.0 || value > max { return None; }
            }
        }
        Some(())
    };
    match node.tag_name().name() {
        "feConvolveMatrix" => for value in node.attributes().filter(|attribute| attribute.name() == "order").map(|attribute| attribute.value()) {
            let values = value.split(|character: char| character == ',' || character.is_ascii_whitespace()).filter(|part| !part.is_empty()).collect::<Vec<_>>();
            if values.is_empty() || values.len() > 2 { return None; }
            for value in values { if !(1..=MAX_ORDER).contains(&value.parse::<u32>().ok()?) { return None; } }
        },
        "feGaussianBlur" | "feDropShadow" => bounded("stdDeviation", MAX_SIGMA)?,
        "feMorphology" => bounded("radius", MAX_RADIUS)?,
        _ => {}
    }
    Some(())
}

fn acyclic(edges: &[Vec<usize>]) -> Option<()> {
    let mut finished = vec![false; edges.len()];
    let mut visiting = vec![false; edges.len()];
    let mut sizes = vec![0u64; edges.len()];
    let mut depths = vec![0usize; edges.len()];
    for start in 0..edges.len() {
        if finished[start] { continue; }
        let mut pending = vec![(start, 0)];
        visiting[start] = true;
        while !pending.is_empty() {
            if pending.len() > MAX_RENDER_DEPTH { return None; }
            let (node, next) = pending.last_mut()?;
            let Some(target) = edges[*node].get(*next).copied() else {
                let mut size = 1u64;
                let mut depth = 1usize;
                for child in &edges[*node] {
                    size = size.checked_add(sizes[*child])?;
                    depth = depth.max(depths[*child] + 1);
                }
                if size > u64::from(MAX_SVG_NODES) || depth > MAX_RENDER_DEPTH { return None; }
                sizes[*node] = size;
                depths[*node] = depth;
                visiting[*node] = false;
                finished[*node] = true;
                pending.pop();
                continue;
            };
            *next += 1;
            if visiting[target] { return None; }
            if !finished[target] { visiting[target] = true; pending.push((target, 0)); }
        }
    }
    Some(())
}
