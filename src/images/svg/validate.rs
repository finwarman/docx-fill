use std::collections::BTreeMap;
use resvg::usvg::roxmltree::{Document, Node, ParsingOptions};

use super::{MAX_RENDER_DEPTH, MAX_SVG_NODES};

pub(super) const MAX_ORDER: u32 = 64;
pub(super) const MAX_SIGMA: f32 = 4096.0;
pub(super) const MAX_RADIUS: f32 = 16384.0;
type UrlSpans = Vec<(usize, usize, String)>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Inheritance { Default, Explicit }

const PAINTED: &[&str] = &["path", "rect", "circle", "ellipse", "polygon", "polyline", "line", "text", "tspan", "textPath", "use"];

pub(super) fn prepare(bytes: &[u8]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(bytes).ok()?;
    if text.contains("<!DOCTYPE") || text.contains("<!ENTITY") { return None; }
    let document = Document::parse_with_options(text, ParsingOptions { nodes_limit: MAX_SVG_NODES, ..Default::default() }).ok()?;
    let nodes = document.descendants().filter(Node::is_element).collect::<Vec<_>>();
    let positions = nodes.iter().enumerate().map(|(index, node)| (node.id().get(), index)).collect::<BTreeMap<_, _>>();
    let mut ids = BTreeMap::new();
    for (index, node) in nodes.iter().enumerate() {
        for attribute in node.attributes().filter(|attribute| attribute.name() == "id") {
            if ids.insert(attribute.value(), index).is_some_and(|previous| previous != index) { return None; }
        }
    }
    let mut edges = vec![Vec::new(); nodes.len()];
    for (index, node) in nodes.iter().enumerate() {
        let name = node.tag_name().name();
        if matches!(name, "foreignObject" | "script") { return None; }
        validate_filter(*node)?;
        for child in node.children().filter(Node::is_element) { edges[index].push(positions[&child.id().get()]); }
        if name == "style" {
            let css = node.children().filter_map(|child| child.text()).collect::<String>();
            let css = clean_css(&css)?;
            let keywords = css.split(|character: char| !character.is_ascii_alphabetic() && character != '-');
            if keywords.clone().any(|word| matches!(word.to_ascii_lowercase().as_str(), "inherit" | "unset" | "revert")) || !references(&css)?.is_empty() { return None; }
        }
        for attribute in node.attributes() {
            if attribute.name() == "href" && name != "a" {
                let href = attribute.value().trim();
                if matches!(name, "image" | "feImage") && href.get(..5).is_some_and(|scheme| scheme.eq_ignore_ascii_case("data:")) { continue; }
                let target = fragment(href)?;
                if let Some(target) = ids.get(target) { edges[index].push(*target); }
            }
            for target in references(attribute.value())? {
                if let Some(target) = ids.get(target.as_str()) { edges[index].push(*target); }
            }
        }
        // Paint is inherited into resource content as well as ordinary descendants.
        if PAINTED.contains(&name) {
            for property in ["fill", "stroke", "marker", "marker-start", "marker-mid", "marker-end"] {
                for target in inherited_paint(*node, property, Inheritance::Default)? {
                    if let Some(target) = ids.get(target.as_str()) { edges[index].push(*target); }
                }
            }
        }
        for property in ["filter", "mask", "clip-path"] {
            for target in inherited_paint(*node, property, Inheritance::Explicit)? {
                if let Some(target) = ids.get(target.as_str()) { edges[index].push(*target); }
            }
        }
        edges[index].sort_unstable();
        edges[index].dedup();
    }
    acyclic(&edges)?;
    let mut edits = Vec::new();
    for node in &nodes {
        for attribute in node.attributes().filter(|attribute| attribute.name() != "href") {
            let (mut value, urls) = urls(attribute.value())?;
            if urls.is_empty() { continue; }
            for (start, end, target) in urls.into_iter().rev() { value.replace_range(start..end, &format!("url(#{target})")); }
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

fn inherited_paint(mut node: Node<'_, '_>, property: &str, inheritance: Inheritance) -> Option<Vec<String>> {
    let mut out = Vec::new();
    loop {
        let mut values = node.attributes().filter(|attribute| attribute.name() == property).map(|attribute| attribute.value().to_string()).collect::<Vec<_>>();
        if let Some(style) = node.attribute("style") {
            let style = clean_css(style)?;
            for declaration in style.split(';') {
                if let Some((name, content)) = declaration.split_once(':')
                    && name.trim().eq_ignore_ascii_case(property) { values.push(content.trim().to_string()); }
            }
        }
        let mut inherits = values.is_empty() && inheritance == Inheritance::Default;
        for value in values {
            let keyword = value.split('!').next()?.trim().to_ascii_lowercase();
            inherits |= matches!(keyword.as_str(), "inherit" | "unset" | "revert");
            out.extend(references(&value)?);
        }
        if !inherits { return Some(out); }
        node = match node.parent_element() { Some(parent) => parent, None => return Some(out) };
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
