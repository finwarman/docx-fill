//! Selector matching and declaration precedence for the supported HTML styles.

use std::collections::BTreeMap;

const MAX_SELECTOR_STEPS: usize = 64;
const VOID_TAGS: [&str; 14] = ["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"];

#[derive(Clone, Copy)]
enum Relation { Descendant, Child, Adjacent }

enum AttrMatch { Present, Equal, Includes, Dash, Prefix, Suffix, Contains }

struct Attribute { name: String, value: String, how: AttrMatch }
struct Simple { tag: Option<String>, ids: Vec<String>, classes: Vec<String>, attributes: Vec<Attribute> }
struct Step { simple: Simple, relation: Relation }
struct Selector { steps: Vec<Step>, specificity: (u32, u32, u32) }
struct Declaration { name: String, value: String, important: bool }
struct Rule { selector: Selector, declarations: Vec<Declaration> }
struct Node { tag: String, attributes: Vec<(String, String)>, parent: Option<usize>, previous: Option<usize> }

pub(super) struct Cascade {
    rules: Vec<Rule>,
    nodes: Vec<Node>,
    stack: Vec<usize>,
    last_children: BTreeMap<Option<usize>, usize>,
}

impl Cascade {
    pub(super) fn new(sheets: &[&str]) -> Self {
        Self { rules: sheets.iter().flat_map(|sheet| read_rules(sheet).unwrap_or_default()).collect(), nodes: Vec::new(), stack: Vec::new(), last_children: BTreeMap::new() }
    }

    pub(super) fn open(&mut self, tag: &str, attributes: &[(String, String)]) -> String {
        let parent = self.stack.last().copied();
        let index = self.nodes.len();
        self.nodes.push(Node { tag: tag.to_string(), attributes: attributes.to_vec(), parent, previous: self.last_children.insert(parent, index) });
        if !VOID_TAGS.contains(&tag) { self.stack.push(index); }
        let mut values = BTreeMap::new();
        for (order, rule) in self.rules.iter().enumerate() {
            if !matches_selector(&rule.selector, &self.nodes, index) { continue; }
            for (at, declaration) in rule.declarations.iter().enumerate() {
                let rank = (declaration.important, rule.selector.specificity, order, at);
                if values.get(&declaration.name).is_none_or(|(previous, _)| rank >= *previous) {
                    values.insert(declaration.name.clone(), (rank, declaration.value.clone()));
                }
            }
        }
        if let Some((_, inline)) = attributes.iter().find(|(name, _)| name == "style") {
            for (at, declaration) in read_declarations(inline).unwrap_or_default().iter().enumerate() {
                let rank = (declaration.important, (u32::MAX, 0, 0), self.rules.len(), at);
                if values.get(&declaration.name).is_none_or(|(previous, _)| rank >= *previous) {
                    values.insert(declaration.name.clone(), (rank, declaration.value.clone()));
                }
            }
        }
        values.into_iter().map(|(name, (_, value))| format!("{name}:{value};")).collect()
    }

    pub(super) fn close(&mut self, tag: &str) {
        if let Some(at) = self.stack.iter().rposition(|index| self.nodes[*index].tag == tag) { self.stack.truncate(at); }
    }
}

pub(super) fn validate_sheet(sheet: &str) -> Result<(), String> { read_rules(sheet).map(|_| ()) }
pub(super) fn validate_style(style: &str) -> Result<(), String> { read_declarations(style).map(|_| ()) }
pub(super) fn matches_element(selector: &str, tag: &str, attributes: &[(String, String)]) -> bool {
    let Ok(selector) = read_selector(selector) else { return false };
    let node = Node { tag: tag.to_string(), attributes: attributes.to_vec(), parent: None, previous: None };
    matches_selector(&selector, &[node], 0)
}

fn read_rules(sheet: &str) -> Result<Vec<Rule>, String> {
    let sheet = strip_comments(sheet)?;
    let mut rest = sheet.trim();
    let mut rules = Vec::new();
    while !rest.is_empty() {
        let open = boundary(rest, '{').ok_or("Missing CSS rule body")?;
        let close = boundary(&rest[open + 1..], '}').map(|at| open + 1 + at).ok_or("Unclosed CSS rule")?;
        for selector in split_outside(&rest[..open], ',')? {
            rules.push(Rule { selector: read_selector(selector.trim())?, declarations: read_declarations(&rest[open + 1..close])? });
        }
        rest = rest[close + 1..].trim();
    }
    Ok(rules)
}

fn read_declarations(style: &str) -> Result<Vec<Declaration>, String> {
    let style = strip_comments(style)?;
    let mut declarations = Vec::new();
    for declaration in split_outside(&style, ';')? {
        if declaration.trim().is_empty() { continue; }
        let colon = boundary(declaration, ':').ok_or("Missing CSS declaration colon")?;
        let name = declaration[..colon].trim().to_ascii_lowercase();
        let mut value = declaration[colon + 1..].trim();
        if name.is_empty() || value.is_empty() { return Err("Empty CSS declaration".into()); }
        let important = value.to_ascii_lowercase().ends_with("!important");
        if important { value = value[..value.len() - "!important".len()].trim_end(); }
        declarations.push(Declaration { name, value: value.to_string(), important });
    }
    Ok(declarations)
}

fn strip_comments(text: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = text;
    let mut quote = None;
    while let Some(c) = rest.chars().next() {
        if quote.is_none() && rest.starts_with("/*") {
            let end = rest.find("*/").ok_or("Unclosed CSS comment")?;
            rest = &rest[end + 2..];
            continue;
        }
        if matches!(c, '\'' | '"') {
            if quote == Some(c) { quote = None; } else if quote.is_none() { quote = Some(c); }
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
        if c == '\\' && quote.is_some()
            && let Some(escaped) = rest.chars().next() {
            out.push(escaped);
            rest = &rest[escaped.len_utf8()..];
        }
    }
    if quote.is_some() { return Err("Unclosed CSS string".into()); }
    Ok(out)
}

fn boundary(text: &str, wanted: char) -> Option<usize> {
    let mut quote = None;
    let mut depth = 0usize;
    let mut escaped = false;
    for (at, c) in text.char_indices() {
        if escaped { escaped = false; continue; }
        if quote.is_some() && c == '\\' { escaped = true; continue; }
        if quote == Some(c) { quote = None; continue; }
        if quote.is_some() { continue; }
        if matches!(c, '\'' | '"') { quote = Some(c); continue; }
        if c == wanted && depth == 0 { return Some(at); }
        match c { '[' | '(' => depth += 1, ']' | ')' => depth = depth.saturating_sub(1), _ => {} }
    }
    None
}

fn split_outside(text: &str, separator: char) -> Result<Vec<&str>, String> {
    let mut rest = text;
    let mut out = Vec::new();
    while let Some(at) = boundary(rest, separator) { out.push(&rest[..at]); rest = &rest[at + separator.len_utf8()..]; }
    out.push(rest);
    let mut quote = None;
    let mut depth = 0isize;
    let mut escaped = false;
    for c in text.chars() {
        if escaped { escaped = false; continue; }
        if quote.is_some() && c == '\\' { escaped = true; continue; }
        if quote == Some(c) { quote = None; continue; }
        if quote.is_some() { continue; }
        if matches!(c, '\'' | '"') { quote = Some(c); continue; }
        match c { '[' | '(' => depth += 1, ']' | ')' => depth -= 1, _ => {} }
        if depth < 0 { return Err("Unbalanced CSS brackets".into()); }
    }
    if quote.is_some() || depth != 0 { return Err("Unbalanced CSS value".into()); }
    Ok(out)
}

fn identifier(text: &str) -> (&str, &str) {
    let end = text.char_indices().find(|(_, c)| !(c.is_alphanumeric() || matches!(c, '_' | '-'))).map_or(text.len(), |(at, _)| at);
    text.split_at(end)
}

fn read_selector(mut text: &str) -> Result<Selector, String> {
    let mut steps = Vec::new();
    let mut relation = Relation::Descendant;
    let mut specificity = (0, 0, 0);
    while !text.trim().is_empty() {
        text = text.trim_start();
        if text.starts_with('>') || text.starts_with('+') {
            if steps.is_empty() { return Err("A CSS selector cannot start with a combinator".into()); }
            relation = if text.starts_with('>') { Relation::Child } else { Relation::Adjacent };
            text = text[1..].trim_start();
            if text.is_empty() { return Err("A CSS selector cannot end with a combinator".into()); }
        }
        if steps.len() >= MAX_SELECTOR_STEPS { return Err("CSS selector is too deep".into()); }
        let mut simple = Simple { tag: None, ids: Vec::new(), classes: Vec::new(), attributes: Vec::new() };
        let universal = text.starts_with('*');
        if let Some(rest) = text.strip_prefix('*') { text = rest; }
        else {
            let (tag, rest) = identifier(text);
            if !tag.is_empty() { simple.tag = Some(tag.to_ascii_lowercase()); specificity.2 += 1; text = rest; }
        }
        let mut saw = universal || simple.tag.is_some();
        while let Some(c) = text.chars().next() {
            match c {
                '#' | '.' => {
                    let (name, rest) = identifier(&text[1..]);
                    if name.is_empty() { return Err("Empty CSS identifier".into()); }
                    if c == '#' { simple.ids.push(name.to_string()); specificity.0 += 1; }
                    else { simple.classes.push(name.to_string()); specificity.1 += 1; }
                    text = rest; saw = true;
                }
                '[' => {
                    let end = boundary(&text[1..], ']').ok_or("Unclosed CSS attribute selector")? + 1;
                    simple.attributes.push(read_attr(&text[1..end])?);
                    specificity.1 += 1;
                    text = &text[end + 1..]; saw = true;
                }
                c if c.is_whitespace() || c == '>' || c == '+' => break,
                _ => return Err("Unsupported CSS selector".into()),
            }
        }
        if !saw { return Err("Empty CSS selector".into()); }
        steps.push(Step { simple, relation });
        relation = Relation::Descendant;
    }
    if steps.is_empty() { return Err("Empty CSS selector".into()); }
    Ok(Selector { steps, specificity })
}

fn read_attr(text: &str) -> Result<Attribute, String> {
    let (name, rest) = identifier(text.trim());
    if name.is_empty() { return Err("Empty CSS attribute".into()); }
    let rest = rest.trim();
    if rest.is_empty() { return Ok(Attribute { name: name.to_ascii_lowercase(), value: String::new(), how: AttrMatch::Present }); }
    let (how, value) = if let Some(value) = rest.strip_prefix("~=") { (AttrMatch::Includes, value) }
        else if let Some(value) = rest.strip_prefix("|=") { (AttrMatch::Dash, value) }
        else if let Some(value) = rest.strip_prefix("^=") { (AttrMatch::Prefix, value) }
        else if let Some(value) = rest.strip_prefix("$=") { (AttrMatch::Suffix, value) }
        else if let Some(value) = rest.strip_prefix("*=") { (AttrMatch::Contains, value) }
        else if let Some(value) = rest.strip_prefix('=') { (AttrMatch::Equal, value) }
        else { return Err("Invalid CSS attribute operator".into()); };
    let value = value.trim();
    let value = if value.starts_with(['\'', '"']) {
        let quote = value.chars().next().unwrap();
        value.strip_prefix(quote).and_then(|value| value.strip_suffix(quote)).ok_or("Unclosed CSS attribute value")?
    } else { value };
    Ok(Attribute { name: name.to_ascii_lowercase(), value: value.to_string(), how })
}

fn matches_selector(selector: &Selector, nodes: &[Node], index: usize) -> bool {
    matches_steps(&selector.steps, nodes, index, selector.steps.len() - 1, &mut BTreeMap::new())
}

fn matches_steps(steps: &[Step], nodes: &[Node], index: usize, step: usize, memo: &mut BTreeMap<(usize, usize), bool>) -> bool {
    if let Some(found) = memo.get(&(index, step)) { return *found; }
    if !matches_simple(&steps[step].simple, &nodes[index]) { return false; }
    if step == 0 { return true; }
    let matched = match steps[step].relation {
        Relation::Child => nodes[index].parent.is_some_and(|parent| matches_steps(steps, nodes, parent, step - 1, memo)),
        Relation::Adjacent => nodes[index].previous.is_some_and(|previous| matches_steps(steps, nodes, previous, step - 1, memo)),
        Relation::Descendant => {
            let mut parent = nodes[index].parent;
            let mut matched = false;
            while let Some(index) = parent {
                if matches_steps(steps, nodes, index, step - 1, memo) { matched = true; break; }
                parent = nodes[index].parent;
            }
            matched
        }
    };
    memo.insert((index, step), matched);
    matched
}

fn matches_simple(simple: &Simple, node: &Node) -> bool {
    let attr = |name: &str| node.attributes.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str());
    if simple.tag.as_ref().is_some_and(|tag| *tag != node.tag) { return false; }
    if simple.ids.iter().any(|id| attr("id") != Some(id.as_str())) { return false; }
    if simple.classes.iter().any(|class| !attr("class").unwrap_or_default().split_whitespace().any(|value| value == class)) { return false; }
    simple.attributes.iter().all(|wanted| attr(&wanted.name).is_some_and(|value| match wanted.how {
        AttrMatch::Present => true,
        AttrMatch::Equal => value == wanted.value,
        AttrMatch::Includes => value.split_whitespace().any(|value| value == wanted.value),
        AttrMatch::Dash => value == wanted.value || value.starts_with(&format!("{}-", wanted.value)),
        AttrMatch::Prefix => !wanted.value.is_empty() && value.starts_with(&wanted.value),
        AttrMatch::Suffix => !wanted.value.is_empty() && value.ends_with(&wanted.value),
        AttrMatch::Contains => !wanted.value.is_empty() && value.contains(&wanted.value),
    }))
}
