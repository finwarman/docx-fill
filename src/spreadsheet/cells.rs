use serde_json::Value;

use crate::render::print_value;
use crate::{FillError, FillOptions};

use super::text::{self, Context, Kind, Tag};
use super::xmlutil::{elements, escaped, local_name, prefix};

#[derive(Debug)]
pub(crate) struct Cell {
    original: String,
    pub text: Option<String>,
    pub tags: Vec<Tag>,
    text_elements: Vec<(usize, usize, usize)>,
    phonetic: Vec<(usize, usize)>,
    original_type: Option<String>,
}

impl Cell {
    pub(crate) fn parse(
        xml: &str,
        shared: &[String],
        options: &FillOptions,
    ) -> Result<Self, FillError> {
        let all = elements(xml)?;
        let cell = all
            .iter()
            .find(|element| local_name(&element.name) == "c" && element.start == 0)
            .ok_or_else(|| {
                FillError::new("Spreadsheet cell XML has no <c> root")
                    .with_id(crate::FillErrorId::SpreadsheetCell)
            })?;
        let cell_type = attribute(&xml[cell.start..cell.open_end], "t");
        let formula = all.iter().any(|element| local_name(&element.name) == "f");
        let value_element = all
            .iter()
            .find(|element| local_name(&element.name) == "v")
            .map(|element| (element.start, element.open_end, element.close_start));
        if cell_type.as_deref() == Some("s") && value_element.is_none() {
            return Err(FillError::new("Shared-string cell has no value element")
                .with_id(crate::FillErrorId::SpreadsheetCell));
        }

        let mut text_elements: Vec<(usize, usize, usize)> = Vec::new();
        // Phonetic runs annotate the cell text; they are not part of it.
        let phonetic: Vec<_> = all
            .iter()
            .filter(|element| local_name(&element.name) == "rPh")
            .map(|element| (element.start, element.end))
            .collect();
        let mut value_text = String::new();
        if cell_type.as_deref() == Some("s") {
            if let Some((_, content_start, value_end)) = value_element {
                if let Ok(index) = xml[content_start..value_end].trim().parse::<usize>() {
                    if let Some(si) = shared.get(index) {
                        let shared_elements = elements(si)?;
                        let phonetic: Vec<_> = shared_elements
                            .iter()
                            .filter(|element| local_name(&element.name) == "rPh")
                            .collect();
                        for item in shared_elements.iter().filter(|element| {
                            local_name(&element.name) == "t"
                                && !phonetic.iter().any(|range| {
                                    item_is_inside(element.start, range.start, range.end)
                                })
                        }) {
                            value_text.push_str(&crate::xml::unescape(
                                &si[item.open_end..item.close_start],
                            ));
                        }
                    } else {
                        return Err(FillError::new("Cell references a missing shared string")
                            .with_id(crate::FillErrorId::SpreadsheetCell));
                    }
                } else {
                    return Err(FillError::new("Cell has an invalid shared-string index")
                        .with_id(crate::FillErrorId::SpreadsheetCell));
                }
            }
        } else if !formula {
            let target = if cell_type.as_deref() == Some("str") && !formula {
                "v"
            } else {
                "t"
            };
            for element in all.iter().filter(|element| {
                local_name(&element.name) == target
                    && element.start >= cell.open_end
                    && element.end <= cell.end
                    && !phonetic.iter().any(|(start, end)| item_is_inside(element.start, *start, *end))
            }) {
                text_elements.push((element.start, element.open_end, element.close_start));
                value_text.push_str(&crate::xml::unescape(
                    &xml[element.open_end..element.close_start],
                ));
            }
        }
        let text = if formula {
            None
        } else if cell_type.as_deref() == Some("s") || !text_elements.is_empty()
            || cell_type.as_deref() == Some("inlineStr")
            || cell_type.as_deref() == Some("str")
        {
            Some(value_text)
        } else {
            None
        };
        let mut delimiters = options.delimiters.clone();
        let tags = if let Some(text) = &text {
            text::parse(text, options, &mut delimiters)?
        } else {
            Vec::new()
        };
        Ok(Self {
            original: xml.to_string(),
            text,
            tags,
            text_elements,
            phonetic,
            original_type: cell_type,
        })
    }

    pub(crate) fn render(
        &self,
        ctx: &mut Context<'_>,
        scopes: &[&Value],
        indexes: &[usize],
        removed: &[usize],
    ) -> Result<String, FillError> {
        let Some(text) = &self.text else {
            return Ok(self.original.clone());
        };
        if self.tags.is_empty() {
            return Ok(self.original.clone());
        }
        let removed_set: std::collections::BTreeSet<usize> = removed.iter().copied().collect();
        let visible: Vec<_> = self
            .tags
            .iter()
            .enumerate()
            .filter(|(index, _)| !removed_set.contains(index))
            .collect();
        if visible.len() == 1 {
            let (visible_index, tag) = visible[0];
            let mut surrounding = text.clone();
            for (index, item) in self.tags.iter().enumerate().rev() {
                if index == visible_index || removed_set.contains(&index) {
                    surrounding.replace_range(item.start..item.end, "");
                }
            }
            if tag.kind == Kind::Value && surrounding.is_empty() {
                return match ctx.value(tag, scopes, indexes) {
                    Some(value) => {
                        if value.as_object().is_some_and(|object| {
                            object.get("type").and_then(Value::as_str) == Some("formula")
                        }) {
                            let (formula, result) = formula_value(&value).ok_or_else(|| {
                                FillError::new("Formula values require a valid string value field")
                                    .with_id(crate::FillErrorId::SpreadsheetCell)
                            })?;
                            self.render_formula(&formula, result.as_ref())
                        } else if value.is_number() || value.is_boolean() || value.is_null() {
                            self.render_typed(&value)
                        } else if value.is_object() {
                            Err(FillError::new(
                                "Spreadsheet cells accept objects only for formula values",
                            )
                            .with_id(crate::FillErrorId::SpreadsheetCell))
                        } else {
                            self.render_text(&print_value(&value))
                        }
                    }
                    None => {
                        let missing = ctx.hooks.missing_value(
                            ctx.part,
                            crate::render::MissingValue {
                                kind: crate::render::MissingKind::Value,
                                name: &tag.name,
                                written: &tag.written,
                            },
                        );
                        self.render_text(missing.as_deref().unwrap_or_default())
                    }
                };
            }
        }
        let tag_indexes = (0..self.tags.len()).collect::<Vec<_>>();
        let out = render_segment(
            text,
            &self.tags,
            &tag_indexes,
            &removed_set,
            ctx,
            scopes,
            indexes,
            0,
            0,
            text.len(),
        )?;
        self.render_text(&out)
    }

    fn render_text(&self, value: &str) -> Result<String, FillError> {
        if value.encode_utf16().count() > 32_767 {
            return Err(FillError::new(
                "A spreadsheet cell exceeds Excel's 32,767 character limit",
            )
            .with_id(crate::FillErrorId::SpreadsheetCell));
        }
        if self.text_elements.is_empty() {
            if self.original_type.as_deref() == Some("s") {
                return Ok(set_inline_string(&self.original, value));
            }
            return Ok(self.original.clone());
        }
        let mut edits: Vec<_> = self.phonetic.iter().map(|(start, end)| (*start, *end, String::new())).collect();
        for (index, (start, open_end, close_start)) in self.text_elements.iter().enumerate() {
            edits.push((
                *open_end,
                *close_start,
                if index == 0 {
                    escaped(&excel_text(value))
                } else {
                    String::new()
                },
            ));
            if index == 0
                && (value.chars().next().is_some_and(char::is_whitespace)
                    || value.chars().next_back().is_some_and(char::is_whitespace))
            {
                let opening = &self.original[*start..*open_end];
                if !opening.contains("xml:space=") {
                    let at = opening.rfind('>').unwrap_or(opening.len());
                    edits.push((
                        *start,
                        *open_end,
                        format!(
                            "{} xml:space=\"preserve\"{}",
                            &opening[..at],
                            &opening[at..]
                        ),
                    ));
                }
            }
        }
        Ok(apply_edits(&self.original, edits))
    }

    fn render_typed(&self, value: &Value) -> Result<String, FillError> {
        let (kind, text) = match value {
            Value::Number(number) => (None, number.to_string()),
            Value::Bool(boolean) => (Some("b"), if *boolean { "1".into() } else { "0".into() }),
            Value::Null => (Some("inlineStr"), String::new()),
            _ => return Ok(self.original.clone()),
        };
        if kind == Some("inlineStr") {
            return Ok(set_inline_string(&self.original, ""));
        }
        let mut result = set_attribute(&self.original, "t", kind);
        let all = elements(&result)?;
        if let Some(element) = all.iter().find(|element| local_name(&element.name) == "v") {
            result.replace_range(element.open_end..element.close_start, &escaped(&text));
        } else {
            let root = all
                .iter()
                .find(|element| element.start == 0 && local_name(&element.name) == "c")
                .ok_or_else(|| FillError::new("Spreadsheet cell XML has no <c> root"))?;
            let p = prefix(&root.name);
            let replacement = format!("<{p}v>{}</{p}v>", escaped(&text));
            if let Some(inline) = all.iter().find(|element| local_name(&element.name) == "is") {
                result.replace_range(inline.start..inline.end, &replacement);
            } else {
                result = insert_cell_content(&result, root, &replacement);
            }
        }
        Ok(result)
    }

    fn render_formula(&self, formula: &str, result: Option<&Value>) -> Result<String, FillError> {
        let all = elements(&self.original)?;
        let mut xml = self.original.clone();
        let p = all
            .iter()
            .find(|element| element.start == 0 && local_name(&element.name) == "c")
            .map_or("", |element| prefix(&element.name));
        let formula = format!("<{p}f>{}</{p}f>", escaped(formula.trim_start_matches('=')));
        let (kind, value) = match result {
            Some(Value::Number(number)) => (None, Some(format!("<{p}v>{}</{p}v>", number))),
            Some(Value::Bool(boolean)) => (
                Some("b"),
                Some(format!("<{p}v>{}</{p}v>", if *boolean { "1" } else { "0" })),
            ),
            Some(Value::String(value)) => (
                Some("str"),
                Some(format!("<{p}v>{}</{p}v>", escaped(&excel_text(value)))),
            ),
            Some(Value::Null) | None => (None, None),
            _ => {
                return Err(FillError::new(
                    "Formula result must be a number, boolean, string, or null",
                )
                .with_id(crate::FillErrorId::SpreadsheetCell));
            }
        };
        let value = value.unwrap_or_default();
        let current = elements(&xml)?;
        let mut old_elements = current
            .iter()
            .filter(|element| matches!(local_name(&element.name), "f" | "v" | "is"))
            .collect::<Vec<_>>();
        old_elements.sort_by_key(|element| std::cmp::Reverse(element.start));
        for element in old_elements {
            xml.replace_range(element.start..element.end, "");
        }
        let current = elements(&xml)?;
        let root = current
            .iter()
            .find(|element| element.start == 0 && local_name(&element.name) == "c")
            .cloned()
            .ok_or_else(|| FillError::new("Spreadsheet cell XML has no <c> root"))?;
        let payload = format!("{formula}{value}");
        xml = insert_cell_content(&xml, &root, &payload);
        xml = set_attribute(&xml, "t", kind);
        Ok(xml)
    }
}

#[allow(clippy::too_many_arguments)]
fn render_segment(
    text: &str,
    tags: &[Tag],
    tag_indexes: &[usize],
    removed: &std::collections::BTreeSet<usize>,
    ctx: &mut Context<'_>,
    scopes: &[&Value],
    indexes: &[usize],
    depth: usize,
    range_start: usize,
    range_end: usize,
) -> Result<String, FillError> {
    if depth > 64 {
        return Err(FillError::new("Cell loop nesting exceeds 64")
            .with_id(crate::FillErrorId::SpreadsheetCell));
    }
    let mut out = String::new();
    let mut cursor = range_start;
    let mut position = 0;
    while position < tag_indexes.len() {
        let index = tag_indexes[position];
        let tag = &tags[index];
        if tag.start < cursor || tag.end > range_end {
            position += 1;
            continue;
        }
        if removed.contains(&index) {
            cursor = tag.end;
            position += 1;
            continue;
        }
        out.push_str(&text[cursor..tag.start]);
        match tag.kind {
            Kind::Value => {
                if let Some(value) = ctx.value(tag, scopes, indexes) {
                    out.push_str(&print_value(&value));
                } else if let Some(missing) = ctx.hooks.missing_value(
                    ctx.part,
                    crate::render::MissingValue {
                        kind: crate::render::MissingKind::Value,
                        name: &tag.name,
                        written: &tag.written,
                    },
                ) {
                    out.push_str(&missing);
                }
                cursor = tag.end;
                position += 1;
            }
            Kind::Open { inverted } => {
                let mut nesting = 1usize;
                let mut close_position = position + 1;
                while close_position < tag_indexes.len() {
                    match tags[tag_indexes[close_position]].kind {
                        Kind::Open { .. } => nesting += 1,
                        Kind::Close => {
                            nesting -= 1;
                            if nesting == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    close_position += 1;
                }
                if close_position == tag_indexes.len() {
                    out.push_str(&text[tag.start..tag.end]);
                    cursor = tag.end;
                    position += 1;
                    continue;
                }
                let close_index = tag_indexes[close_position];
                let close = &tags[close_index];
                if !tags[close_index].name.is_empty() && tags[close_index].name != tag.name {
                    return Err(FillError::new(format!(
                        "Cell loop {:?} is closed by {:?}",
                        tag.name, close.name
                    ))
                    .with_id(crate::FillErrorId::SpreadsheetCell));
                }
                let value = ctx.value(tag, scopes, indexes);
                match value.as_ref() {
                    Some(Value::Array(items)) if !inverted => {
                        for (item_index, item) in items.iter().enumerate() {
                            let mut child_scopes = scopes.to_vec();
                            child_scopes.push(item);
                            let mut child_indexes = indexes.to_vec();
                            child_indexes.push(item_index);
                            let children = &tag_indexes[position + 1..close_position];
                            out.push_str(&render_segment(
                                text,
                                tags,
                                children,
                                removed,
                                ctx,
                                &child_scopes,
                                &child_indexes,
                                depth + 1,
                                tag.end,
                                close.start,
                            )?);
                        }
                    }
                    Some(Value::Array(items)) if inverted && items.is_empty() => {
                        out.push_str(&render_segment(
                            text,
                            tags,
                            &tag_indexes[position + 1..close_position],
                            removed,
                            ctx,
                            scopes,
                            indexes,
                            depth + 1,
                            tag.end,
                            close.start,
                        )?);
                    }
                    Some(value) if !inverted && text::shown(Some(value)) => {
                        let item = value.clone();
                        let mut child_scopes = scopes.to_vec();
                        child_scopes.push(&item);
                        out.push_str(&render_segment(
                            text,
                            tags,
                            &tag_indexes[position + 1..close_position],
                            removed,
                            ctx,
                            &child_scopes,
                            indexes,
                            depth + 1,
                            tag.end,
                            close.start,
                        )?);
                    }
                    Some(value) if inverted && !text::shown(Some(value)) => {
                        out.push_str(&render_segment(
                            text,
                            tags,
                            &tag_indexes[position + 1..close_position],
                            removed,
                            ctx,
                            scopes,
                            indexes,
                            depth + 1,
                            tag.end,
                            close.start,
                        )?);
                    }
                    None if inverted => {
                        out.push_str(&render_segment(
                            text,
                            tags,
                            &tag_indexes[position + 1..close_position],
                            removed,
                            ctx,
                            scopes,
                            indexes,
                            depth + 1,
                            tag.end,
                            close.start,
                        )?);
                    }
                    _ => {}
                }
                cursor = close.end;
                position = close_position + 1;
            }
            Kind::Close => {
                out.push_str(&text[tag.start..tag.end]);
                cursor = tag.end;
                position += 1;
            }
            Kind::Change => {
                cursor = tag.end;
                position += 1;
            }
        }
    }
    out.push_str(&text[cursor..range_end]);
    Ok(out)
}

fn formula_value(value: &Value) -> Option<(String, Option<Value>)> {
    let object = value.as_object()?;
    if object.get("type")?.as_str()? != "formula" {
        return None;
    }
    let formula = object.get("value")?.as_str()?;
    if formula.trim().is_empty() || formula.chars().count() > 8_192 {
        return None;
    }
    Some((formula.to_string(), object.get("result").cloned()))
}

fn item_is_inside(item_start: usize, parent_start: usize, parent_end: usize) -> bool {
    item_start > parent_start && item_start < parent_end
}

fn attribute(open: &str, name: &str) -> Option<String> {
    let marker = format!("{name}=");
    let at = open.find(&marker)? + marker.len();
    let quote = open[at..].chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let end = open[at + 1..].find(quote)? + at + 1;
    Some(crate::xml::unescape(&open[at + 1..end]))
}

fn set_attribute(xml: &str, name: &str, value: Option<&str>) -> String {
    let root = match elements(xml) {
        Ok(elements) => elements
            .into_iter()
            .find(|element| element.start == 0 && local_name(&element.name) == "c"),
        Err(_) => None,
    };
    let Some(root) = root else { return xml.into() };
    let open = &xml[root.start..root.open_end];
    let existing = attribute(open, name);
    let updated = if existing.is_some() {
        let marker = format!("{name}=");
        let at = open.find(&marker).unwrap_or(open.len()) + marker.len();
        let quote = open[at..].chars().next().unwrap_or('"');
        let end = open[at + 1..]
            .find(quote)
            .map_or(at + 1, |end| at + 1 + end);
        if let Some(value) = value {
            format!(
                "{}{name}={quote}{}{quote}{}",
                &open[..at - marker.len()],
                escaped(value),
                &open[end + 1..]
            )
        } else {
            format!("{}{}", &open[..at - marker.len() - 1], &open[end + 1..])
        }
    } else if let Some(value) = value {
        format!(
            "{} {name}=\"{}\"{}",
            &open[..open.len() - 1],
            escaped(value),
            &open[open.len() - 1..]
        )
    } else {
        let _ = existing;
        open.to_string()
    };
    format!("{}{}{}", &xml[..root.start], updated, &xml[root.open_end..])
}

fn set_inline_string(xml: &str, value: &str) -> String {
    let root = match elements(xml) {
        Ok(elements) => elements
            .into_iter()
            .find(|element| element.start == 0 && local_name(&element.name) == "c"),
        Err(_) => None,
    };
    let Some(root) = root else { return xml.into() };
    let p = prefix(&root.name);
    let is_tag = format!("{p}is");
    let inline = format!(
        "<{is_tag}><{p}t{}>{}</{p}t></{is_tag}>",
        space_preserve(value),
        escaped(&excel_text(value))
    );
    let elements = match elements(xml) {
        Ok(elements) => elements,
        Err(_) => return xml.into(),
    };
    if let Some(is) = elements
        .iter()
        .find(|element| local_name(&element.name) == "is")
    {
        let mut updated = xml.to_string();
        updated.replace_range(is.start..is.end, &inline);
        set_attribute(&updated, "t", Some("inlineStr"))
    } else if let Some(v) = elements
        .iter()
        .find(|element| local_name(&element.name) == "v")
    {
        let mut updated = xml.to_string();
        updated.replace_range(v.start..v.end, &inline);
        set_attribute(&updated, "t", Some("inlineStr"))
    } else {
        let updated = insert_cell_content(xml, &root, &inline);
        set_attribute(&updated, "t", Some("inlineStr"))
    }
}

fn insert_cell_content(xml: &str, root: &super::xmlutil::Element, content: &str) -> String {
    if root.close_start == root.open_end
        && xml[root.start..root.open_end].trim_end().ends_with("/>")
    {
        let open = &xml[root.start..root.open_end];
        let open = open.trim_end().strip_suffix("/>").unwrap_or(open);
        format!(
            "{}>{}{content}</{}>{}",
            &xml[..root.start],
            open,
            root.name,
            &xml[root.end..]
        )
    } else {
        format!(
            "{}{}{content}{}",
            &xml[..root.close_start],
            "",
            &xml[root.close_start..]
        )
    }
}

fn space_preserve(value: &str) -> &'static str {
    if value.chars().next().is_some_and(char::is_whitespace)
        || value.chars().next_back().is_some_and(char::is_whitespace)
    {
        " xml:space=\"preserve\""
    } else {
        ""
    }
}

fn excel_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.char_indices().peekable();
    while let Some((_, ch)) = chars.next() {
        if ch == '_' && chars.peek().is_some_and(|(_, next)| *next == 'x') {
            let at = chars.peek().map_or(value.len(), |(at, _)| *at);
            let tail = &value[at..];
            if tail.len() >= 6
                && tail.as_bytes().get(5) == Some(&b'_')
                && tail[1..5].bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                out.push_str("_x005F_");
                continue;
            }
        }
        if !crate::xml::is_xml_char(ch) {
            out.push_str(&format!("_x{:04X}_", u32::from(ch)));
        } else {
            out.push(ch);
        }
    }
    out
}

fn apply_edits(xml: &str, mut edits: Vec<(usize, usize, String)>) -> String {
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.0));
    let mut xml = xml.to_string();
    for (start, end, replacement) in edits {
        xml.replace_range(start..end, &replacement);
    }
    xml
}
