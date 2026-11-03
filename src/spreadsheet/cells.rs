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
    run_texts: Vec<String>,
    phonetic: Vec<(usize, usize)>,
    original_type: Option<String>,
    shared_xml: Option<String>,
    html_ignore_unknown: bool,
    html_ignore_css: bool,
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
        let mut run_texts: Vec<String> = Vec::new();
        // Phonetic runs annotate the cell text; they are not part of it.
        let phonetic: Vec<_> = all
            .iter()
            .filter(|element| local_name(&element.name) == "rPh")
            .map(|element| (element.start, element.end))
            .collect();
        let mut value_text = String::new();
        let mut shared_xml = None;
        if cell_type.as_deref() == Some("s") {
            if let Some((_, content_start, value_end)) = value_element {
                if let Ok(index) = xml[content_start..value_end].trim().parse::<usize>() {
                    if let Some(si) = shared.get(index) {
                        shared_xml = Some(si.clone());
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
                    && !phonetic
                        .iter()
                        .any(|(start, end)| item_is_inside(element.start, *start, *end))
            }) {
                text_elements.push((element.start, element.open_end, element.close_start));
                let run = crate::xml::unescape(&xml[element.open_end..element.close_start]);
                run_texts.push(run.clone());
                value_text.push_str(&run);
            }
        }
        let text = if formula {
            None
        } else if cell_type.as_deref() == Some("s")
            || !text_elements.is_empty()
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
            run_texts,
            phonetic,
            original_type: cell_type,
            shared_xml,
            html_ignore_unknown: options.html.ignore_unknown_tags,
            html_ignore_css: options.html.ignore_css_errors,
        })
    }

    #[allow(clippy::collapsible_if)]
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
            let replace_start = if removed_set.is_empty() { tag.start } else { 0 };
            let replace_end = if removed_set.is_empty() { tag.end } else { text.len() };
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
                            self.apply_descriptor_style(&value, ctx, "")?;
                            self.render_formula(&formula, result.as_ref())
                        } else if let Some(kind) = value.get("type").and_then(Value::as_str) {
                            match kind {
                                "date" | "datetime" | "time" => {
                                    self.render_temporal(&value, kind, ctx)
                                }
                                "richText" => self.render_rich_text(&value, ctx),
                                "string" => {
                                    self.apply_descriptor_style(&value, ctx, "")?;
                                    let text = value
                                        .get("value")
                                        .and_then(Value::as_str)
                                        .ok_or_else(|| {
                                            cell_error("String descriptors require a string value")
                                        })?;
                                    self.render_text_at(text, replace_start, replace_end)
                                }
                                "boolean" => {
                                    let flag = value
                                        .get("value")
                                        .and_then(Value::as_bool)
                                        .ok_or_else(|| {
                                            cell_error(
                                                "Boolean descriptors require a boolean value",
                                            )
                                        })?;
                                    self.apply_descriptor_style(&value, ctx, "")?;
                                    self.render_typed(&Value::Bool(flag))
                                }
                                "number" | "cell" | "float" | "int" | "currency" | "percent" => {
                                    let mut descriptor = value.clone();
                                    if matches!(kind, "currency" | "percent") {
                                        let object =
                                            descriptor.as_object_mut().expect("object descriptor");
                                        let has_format = object
                                            .get("style")
                                            .and_then(Value::as_object)
                                            .is_some_and(|s| {
                                                s.contains_key("format")
                                                    || s.contains_key("numberFormat")
                                            })
                                            || object.contains_key("format")
                                            || object.contains_key("fmt");
                                        if !has_format {
                                            object.insert(
                                                "format".into(),
                                                Value::String(
                                                    if kind == "currency" {
                                                        "$#,##0.00"
                                                    } else {
                                                        "0.00%"
                                                    }
                                                    .into(),
                                                ),
                                            );
                                        }
                                    }
                                    self.apply_descriptor_style(&descriptor, ctx, "")?;
                                    let mut cell_value =
                                        value.get("value").cloned().unwrap_or(Value::Null);
                                    if kind == "cell" && cell_value.is_null() {
                                        cell_value = value
                                            .get("hyperlink")
                                            .and_then(|h| {
                                                h.get("display").or_else(|| h.get("target"))
                                            })
                                            .cloned()
                                            .or_else(|| {
                                                value
                                                    .get("hyperlink")
                                                    .and_then(Value::as_str)
                                                    .map(|s| Value::String(s.to_string()))
                                            })
                                            .unwrap_or(Value::Null);
                                    }
                                    if kind == "int"
                                        && !cell_value.as_i64().is_some()
                                        && !cell_value.as_u64().is_some()
                                    {
                                        return Err(cell_error(
                                            "Integer descriptors require an integer value",
                                        ));
                                    }
                                    if kind == "float" && !cell_value.is_number() {
                                        return Err(cell_error(
                                            "Float descriptors require a numeric value",
                                        ));
                                    }
                                    if matches!(kind, "number" | "currency" | "percent")
                                        && !cell_value.is_number()
                                    {
                                        return Err(cell_error(format!(
                                            "{} descriptors require a numeric value",
                                            match kind {
                                                "number" => "Number",
                                                "currency" => "Currency",
                                                _ => "Percent",
                                            }
                                        )));
                                    }
                                    if cell_value.is_number()
                                        || cell_value.is_boolean()
                                        || cell_value.is_null()
                                    {
                                        self.render_typed(&cell_value)
                                    } else if cell_value.is_string() {
                                        self.render_text_at(
                                            &print_value(&cell_value),
                                            replace_start,
                                            replace_end,
                                        )
                                    } else {
                                        Err(cell_error("Cell descriptor value must be a scalar"))
                                    }
                                }
                                "image" | "chart" => {
                                    ctx.effects.push(super::effects::CellEffect {
                                        kind: if kind == "image" {
                                            super::effects::EffectKind::Image
                                        } else {
                                            super::effects::EffectKind::Chart
                                        },
                                        cell: ctx.cell.clone(),
                                        value: value.get("value").cloned().unwrap_or(Value::Null),
                                        part: ctx.part.to_string(),
                                    });
                                    self.render_typed(&Value::Null)
                                }
                                "formula" => unreachable!(),
                                _ => Err(FillError::new(format!(
                                    "Unsupported spreadsheet cell type {kind:?}"
                                ))
                                .with_id(crate::FillErrorId::SpreadsheetCell)),
                            }
                        } else if value.is_number() || value.is_boolean() || value.is_null() {
                            self.render_typed(&value)
                        } else if value.is_object() {
                            Err(FillError::new(
                                "Spreadsheet cells accept objects only for formula values",
                            )
                            .with_id(crate::FillErrorId::SpreadsheetCell))
                        } else {
                            self.render_text_at(&print_value(&value), replace_start, replace_end)
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
                        self.render_text_at(
                            missing.as_deref().unwrap_or_default(),
                            replace_start,
                            replace_end,
                        )
                    }
                };
            }
            if tag.kind == Kind::Value && removed_set.is_empty() {
                let replacement = match ctx.value(tag, scopes, indexes) {
                    Some(Value::String(value)) => value,
                    Some(value) if value.is_number() || value.is_boolean() => print_value(&value),
                    Some(Value::Null) => String::new(),
                    Some(_value) => {
                        return Err(cell_error(
                            "Typed cell descriptors must occupy the whole cell",
                        ));
                    }
                    None => ctx
                        .hooks
                        .missing_value(
                            ctx.part,
                            crate::render::MissingValue {
                                kind: crate::render::MissingKind::Value,
                                name: &tag.name,
                                written: &tag.written,
                            },
                        )
                        .unwrap_or_default(),
                };
                return self.render_text_at(&replacement, tag.start, tag.end);
            }
            if matches!(tag.kind, Kind::RichText { .. }) && surrounding.is_empty() {
                if let Some(value) = ctx.value(tag, scopes, indexes) {
                    let html = print_value(&value);
                    let descriptor =
                        html_descriptor(&html, self.html_ignore_unknown, self.html_ignore_css)?;
                    return self.render_rich_text(&descriptor, ctx);
                }
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
        let mut edits: Vec<_> = self
            .phonetic
            .iter()
            .map(|(start, end)| (*start, *end, String::new()))
            .collect();
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

    fn render_text_at(&self, value: &str, start: usize, end: usize) -> Result<String, FillError> {
        if self.run_texts.is_empty() {
            if let Some(si) = &self.shared_xml {
                return self.render_shared_text_at(si, value, start, end);
            }
            return self.render_text(value);
        }
        if value.encode_utf16().count() > 32_767 {
            return Err(cell_error(
                "A spreadsheet cell exceeds Excel's 32,767 character limit",
            ));
        }
        let mut before = vec![String::new(); self.run_texts.len()];
        let mut after = vec![String::new(); self.run_texts.len()];
        let mut offset = 0usize;
        let mut insertion = None;
        for (index, run) in self.run_texts.iter().enumerate() {
            let next = offset + run.len();
            let before_end = start.clamp(offset, next);
            if before_end > offset {
                before[index].push_str(&run[..before_end - offset]);
            }
            if insertion.is_none() && start >= offset && start <= next {
                insertion = Some(index);
            }
            let after_start = end.clamp(offset, next);
            if after_start < next {
                after[index].push_str(&run[after_start - offset..]);
            }
            offset = next;
        }
        let index = insertion.unwrap_or(before.len() - 1);
        let mut values = before;
        values[index].push_str(value);
        for (current, suffix) in values.iter_mut().zip(after) {
            current.push_str(&suffix);
        }
        let mut edits: Vec<_> = self
            .phonetic
            .iter()
            .map(|(a, b)| (*a, *b, String::new()))
            .collect();
        for ((start, open, close), value) in self.text_elements.iter().zip(values) {
            edits.push((*open, *close, escaped(&excel_text(&value))));
            if !value.is_empty()
                && (value.chars().next().is_some_and(char::is_whitespace)
                    || value.chars().next_back().is_some_and(char::is_whitespace))
            {
                let raw = &self.original[*start..*open];
                if !raw.contains("xml:space=") {
                    let at = raw.rfind('>').unwrap_or(raw.len());
                    edits.push((
                        *start,
                        *open,
                        format!("{} xml:space=\"preserve\"{}", &raw[..at], &raw[at..]),
                    ));
                }
            }
        }
        Ok(apply_edits(&self.original, edits))
    }

    fn render_shared_text_at(
        &self,
        shared: &str,
        value: &str,
        start: usize,
        end: usize,
    ) -> Result<String, FillError> {
        if value.encode_utf16().count() > 32_767 {
            return Err(cell_error(
                "A spreadsheet cell exceeds Excel's 32,767 character limit",
            ));
        }
        let all = elements(shared)?;
        let texts: Vec<_> = all
            .iter()
            .filter(|e| {
                local_name(&e.name) == "t"
                    && !all.iter().any(|r| {
                        local_name(&r.name) == "rPh" && item_is_inside(e.start, r.start, r.end)
                    })
            })
            .collect();
        if texts.is_empty() {
            return Ok(set_inline_string(&self.original, value));
        }
        let runs: Vec<String> = texts
            .iter()
            .map(|e| crate::xml::unescape(&shared[e.open_end..e.close_start]))
            .collect();
        let mut before = vec![String::new(); runs.len()];
        let mut after = vec![String::new(); runs.len()];
        let mut offset = 0usize;
        let mut insertion = None;
        for (index, run) in runs.iter().enumerate() {
            let next = offset + run.len();
            let before_end = start.clamp(offset, next);
            if before_end > offset {
                before[index].push_str(&run[..before_end - offset]);
            }
            if insertion.is_none() && start >= offset && start <= next {
                insertion = Some(index);
            }
            let after_start = end.clamp(offset, next);
            if after_start < next {
                after[index].push_str(&run[after_start - offset..]);
            }
            offset = next;
        }
        let index = insertion.unwrap_or(before.len() - 1);
        let mut output = before;
        output[index].push_str(value);
        for (run, suffix) in output.iter_mut().zip(after) {
            run.push_str(&suffix);
        }
        let mut edits = Vec::new();
        for (node, run) in texts.iter().zip(output) {
            edits.push((node.open_end, node.close_start, escaped(&excel_text(&run))));
            if !run.is_empty()
                && (run.chars().next().is_some_and(char::is_whitespace)
                    || run.chars().next_back().is_some_and(char::is_whitespace))
            {
                let opening = &shared[node.start..node.open_end];
                if !opening.contains("xml:space=") {
                    let at = opening.rfind('>').unwrap_or(opening.len());
                    edits.push((
                        node.start,
                        node.open_end,
                        format!(
                            "{} xml:space=\"preserve\"{}",
                            &opening[..at],
                            &opening[at..]
                        ),
                    ));
                }
            }
        }
        for e in all.iter().filter(|e| local_name(&e.name) == "rPh") {
            edits.push((e.start, e.end, String::new()));
        }
        let si = apply_edits(shared, edits);
        let si_elements = elements(&si)?;
        let root = si_elements
            .first()
            .ok_or_else(|| cell_error("Shared string is empty"))?;
        let mut inline = si[root.start..root.end].to_string();
        inline = inline
            .replacen(":si", ":is", 1)
            .replacen("<si", "<is", 1)
            .replacen("</si>", "</is>", 1);
        let mut xml = set_attribute(&self.original, "t", Some("inlineStr"));
        let all = elements(&xml)?;
        let payload = inline;
        let mut old = all
            .iter()
            .filter(|e| local_name(&e.name) == "v")
            .map(|e| (e.start, e.end))
            .collect::<Vec<_>>();
        old.sort_by_key(|(s, _)| std::cmp::Reverse(*s));
        for (s, e) in old {
            xml.replace_range(s..e, "");
        }
        let c = elements(&xml)?
            .first()
            .cloned()
            .ok_or_else(|| cell_error("Spreadsheet cell is empty"))?;
        Ok(insert_cell_content(&xml, &c, &payload))
    }

    fn render_typed(&self, value: &Value) -> Result<String, FillError> {
        let (kind, text) = match value {
            Value::Number(number) => {
                validate_numeric_precision(&number.to_string())?;
                (None, number.to_string())
            }
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

    fn render_temporal(
        &self,
        descriptor: &Value,
        kind: &str,
        ctx: &mut Context<'_>,
    ) -> Result<String, FillError> {
        let raw = descriptor
            .get("value")
            .and_then(Value::as_str)
            .ok_or_else(|| cell_error("Date and time descriptors require a string value"))?;
        let serial = match kind {
            "time" => parse_time(raw)?,
            "date" => {
                let (y, m, d) = parse_date(raw)?;
                date_serial(y, m, d, ctx.date_1904)? as f64
            }
            _ => {
                let (date, time) = raw
                    .split_once('T')
                    .or_else(|| raw.split_once(' '))
                    .ok_or_else(|| {
                        cell_error("Datetime must contain a date and time separated by T")
                    })?;
                let (y, m, d) = parse_date(date)?;
                let t = parse_time(time.trim_end_matches('Z'))?;
                date_serial(y, m, d, ctx.date_1904)? as f64 + t
            }
        };
        let number = serde_json::Number::from_f64(serial)
            .ok_or_else(|| cell_error("Date or time value is outside the supported range"))?;
        let value = Value::Number(number);
        self.apply_descriptor_style(descriptor, ctx, kind)?;
        self.render_typed(&value)
    }

    #[allow(clippy::collapsible_if)]
    fn apply_descriptor_style(
        &self,
        descriptor: &Value,
        ctx: &mut Context<'_>,
        temporal: &str,
    ) -> Result<(), FillError> {
        let mut style = descriptor
            .get("style")
            .cloned()
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        if let Some(style_map) = style.as_object_mut() {
            for key in [
                "numberFormat",
                "format",
                "fmt",
                "font",
                "fill",
                "border",
                "alignment",
            ] {
                if let Some(value) = descriptor.get(key) {
                    style_map.insert(
                        if key == "fmt" {
                            "format".into()
                        } else {
                            key.to_string()
                        },
                        value.clone(),
                    );
                }
            }
        }
        if style.as_object().is_some_and(|map| !map.is_empty()) || !temporal.is_empty() {
            if !style.is_object() {
                return Err(cell_error("Style must be an object"));
            }
            let mut value = style;
            if let Some(map) = value.as_object_mut() {
                if !map.contains_key("numberFormat")
                    && !map.contains_key("format")
                    && !temporal.is_empty()
                {
                    map.insert(
                        "numberFormat".into(),
                        Value::String(
                            match temporal {
                                "date" => "yyyy-mm-dd",
                                "time" => "hh:mm:ss",
                                _ => "yyyy-mm-dd hh:mm:ss",
                            }
                            .into(),
                        ),
                    );
                }
            }
            ctx.effects.push(super::effects::CellEffect {
                kind: super::effects::EffectKind::Style,
                cell: ctx.cell.clone(),
                value,
                part: ctx.part.to_string(),
            });
        }
        for (key, kind) in [
            ("hyperlink", super::effects::EffectKind::Hyperlink),
            ("comment", super::effects::EffectKind::Comment),
        ] {
            if let Some(value) = descriptor.get(key) {
                ctx.effects.push(super::effects::CellEffect {
                    kind,
                    cell: ctx.cell.clone(),
                    value: value.clone(),
                    part: ctx.part.to_string(),
                });
            }
        }
        Ok(())
    }

    fn render_rich_text(
        &self,
        descriptor: &Value,
        ctx: &mut Context<'_>,
    ) -> Result<String, FillError> {
        if let Some(key) = descriptor.as_object().and_then(|o| {
            o.keys().find(|k| {
                !matches!(
                    k.as_str(),
                    "type"
                        | "runs"
                        | "raw"
                        | "style"
                        | "font"
                        | "fill"
                        | "border"
                        | "alignment"
                        | "format"
                        | "fmt"
                        | "numberFormat"
                        | "hyperlink"
                        | "comment"
                )
            })
        }) {
            return Err(cell_error(format!(
                "Unsupported rich text descriptor property {key:?}"
            )));
        }
        self.apply_descriptor_style(descriptor, ctx, "")?;
        let root = elements(&self.original)?
            .into_iter()
            .find(|e| e.start == 0 && local_name(&e.name) == "c")
            .ok_or_else(|| cell_error("Spreadsheet cell has no root"))?;
        let p = prefix(&root.name);
        if let Some(raw) = descriptor.get("raw") {
            if descriptor.get("runs").is_some() {
                return Err(cell_error("richText accepts either runs or raw markup"));
            }
            let raw = raw
                .as_str()
                .ok_or_else(|| cell_error("richText raw markup must be a string"))?;
            let body = validate_raw_runs(raw, p)?;
            return self.insert_inline_rich(p, &body);
        }
        let runs = descriptor
            .get("runs")
            .and_then(Value::as_array)
            .ok_or_else(|| cell_error("richText requires runs or raw markup"))?;
        let mut body = String::new();
        for run in runs {
            let obj = run
                .as_object()
                .ok_or_else(|| cell_error("Each rich text run must be an object"))?;
            let text = obj
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| cell_error("Rich text runs require a string text field"))?;
            let mut props = String::new();
            for (key, tag) in [("bold", "b"), ("italic", "i"), ("strike", "strike")] {
                if let Some(value) = obj.get(key).and_then(Value::as_bool) {
                    if value {
                        props.push_str(&format!("<{p}{tag}/>"));
                    } else {
                        props.push_str(&format!("<{p}{tag} val=\"0\"/>"));
                    }
                }
            }
            if let Some(color) = obj.get("color") {
                let color = color
                    .as_str()
                    .filter(|s| {
                        let s = s.trim_start_matches('#');
                        (s.len() == 6 || s.len() == 8) && s.bytes().all(|b| b.is_ascii_hexdigit())
                    })
                    .ok_or_else(|| cell_error("Rich text color must be six or eight hex digits"))?;
                let color = color.trim_start_matches('#');
                let rgb = if color.len() == 6 {
                    format!("FF{}", color.to_ascii_uppercase())
                } else {
                    color.to_ascii_uppercase()
                };
                props.push_str(&format!("<{p}color rgb=\"{rgb}\"/>"));
            }
            if let Some(size) = obj.get("size") {
                let n = size
                    .as_f64()
                    .filter(|n| n.is_finite() && *n >= 1.0 && *n <= 409.0)
                    .ok_or_else(|| cell_error("Rich text size must be from 1 to 409 points"))?;
                props.push_str(&format!("<{p}sz val=\"{n}\"/>"));
            }
            if let Some(value) = obj.get("underline").and_then(Value::as_bool) {
                if value {
                    props.push_str(&format!("<{p}u/>"));
                } else {
                    props.push_str(&format!("<{p}u val=\"0\"/>"));
                }
            }
            if let Some(font) = obj.get("font") {
                let font = font
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 31)
                    .ok_or_else(|| cell_error("Rich text font must contain 1 to 31 characters"))?;
                props.push_str(&format!("<{p}rFont val=\"{}\"/>", escaped(font)));
            }
            if let Some(key) = obj.keys().find(|k| {
                !matches!(
                    k.as_str(),
                    "text" | "bold" | "italic" | "underline" | "strike" | "size" | "color" | "font"
                )
            }) {
                return Err(cell_error(format!(
                    "Unsupported rich text property {key:?}"
                )));
            }
            body.push_str(&format!(
                "<{p}r>{}<{p}t{}>{}</{p}t></{p}r>",
                if props.is_empty() {
                    String::new()
                } else {
                    format!("<{p}rPr>{props}</{p}rPr>")
                },
                space_preserve(text),
                escaped(&excel_text(text))
            ));
        }
        self.insert_inline_rich(p, &body)
    }

    fn insert_inline_rich(&self, p: &str, body: &str) -> Result<String, FillError> {
        let inline = format!("<{p}is>{body}</{p}is>");
        let mut xml = self.original.clone();
        let all = elements(&xml)?;
        if let Some(is) = all.iter().find(|e| local_name(&e.name) == "is") {
            xml.replace_range(is.start..is.end, &inline);
        } else if let Some(v) = all.iter().find(|e| local_name(&e.name) == "v") {
            xml.replace_range(v.start..v.end, &inline);
        } else {
            let root = elements(&xml)?.remove(0);
            xml = insert_cell_content(&xml, &root, &inline);
        }
        Ok(set_attribute(&xml, "t", Some("inlineStr")))
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
            Some(Value::Number(number)) => {
                validate_numeric_precision(&number.to_string())?;
                (None, Some(format!("<{p}v>{}</{p}v>", number)))
            }
            Some(Value::Bool(boolean)) => (
                Some("b"),
                Some(format!("<{p}v>{}</{p}v>", if *boolean { "1" } else { "0" })),
            ),
            Some(Value::String(value)) => {
                let value = excel_text(value);
                validate_excel_text_len(&value)?;
                (
                    Some("str"),
                    Some(format!("<{p}v>{}</{p}v>", escaped(&value))),
                )
            }
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
    if depth > ctx.max_loop_depth {
        return Err(
            FillError::new(format!("Cell loop nesting exceeds {}", ctx.max_loop_depth))
                .with_id(crate::FillErrorId::SpreadsheetLimit),
        );
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
            Kind::Open { inverted, .. } => {
                let mut nesting = 1usize;
                let mut close_position = position + 1;
                while close_position < tag_indexes.len() {
                    match tags[tag_indexes[close_position]].kind {
                        Kind::Open { .. } => nesting += 1,
                        Kind::Close { .. } => {
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
            Kind::Close { .. } => {
                out.push_str(&text[tag.start..tag.end]);
                cursor = tag.end;
                position += 1;
            }
            Kind::Change => {
                cursor = tag.end;
                position += 1;
            }
            Kind::Image | Kind::Chart => {
                if let Some(value) = ctx.value(tag, scopes, indexes) {
                    ctx.effects.push(super::effects::CellEffect {
                        kind: if tag.kind == Kind::Image {
                            super::effects::EffectKind::Image
                        } else {
                            super::effects::EffectKind::Chart
                        },
                        cell: ctx.cell.clone(),
                        value,
                        part: ctx.part.to_string(),
                    });
                }
                cursor = tag.end;
                position += 1;
            }
            Kind::RichText { .. } => {
                if let Some(value) = ctx.value(tag, scopes, indexes) {
                    out.push_str(&print_value(&value));
                }
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

fn cell_error(message: impl Into<String>) -> FillError {
    FillError::new(message).with_id(crate::FillErrorId::SpreadsheetCell)
}

fn validate_numeric_precision(raw: &str) -> Result<(), FillError> {
    let mantissa = raw.split(['e', 'E']).next().unwrap_or(raw);
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let significant = digits.trim_start_matches('0').trim_end_matches('0').len();
    if significant > 15 {
        return Err(cell_error(
            "Numeric cell values are limited to 15 significant decimal digits",
        ));
    }
    Ok(())
}

fn validate_excel_text_len(value: &str) -> Result<(), FillError> {
    if value.encode_utf16().count() > 32_767 {
        return Err(cell_error(
            "Formula result exceeds Excel's 32,767 character cell limit",
        ));
    }
    Ok(())
}
fn html_descriptor(
    html: &str,
    ignore_unknown: bool,
    ignore_css_errors: bool,
) -> Result<Value, FillError> {
    let token = regex::Regex::new(r"(?is)<(/?)([a-z][a-z0-9]*)([^>]*)>|([^<]+)|(<)")
        .expect("constant regex");
    let mut stack: Vec<(String, serde_json::Map<String, Value>)> = Vec::new();
    let mut runs = Vec::new();
    let mut hyperlink: Option<String> = None;
    let mut cursor = 0;
    for cap in token.captures_iter(html) {
        let m = cap.get(0).expect("matched");
        if m.start() != cursor {
            return Err(cell_error(
                "Unsupported or malformed HTML in spreadsheet rich text",
            ));
        }
        cursor = m.end();
        if let Some(text) = cap.get(4) {
            let text = decode_html_entities(text.as_str());
            if text.is_empty() {
                continue;
            }
            let mut run = serde_json::Map::new();
            run.insert("text".into(), Value::String(text));
            for (tag, styles) in &stack {
                let key = match tag.as_str() {
                    "b" | "strong" => "bold",
                    "i" | "em" => "italic",
                    "u" => "underline",
                    "s" | "strike" => "strike",
                    _ => continue,
                };
                run.insert(key.into(), Value::Bool(true));
                for (key, value) in styles {
                    run.insert(key.clone(), value.clone());
                }
            }
            for (_, styles) in &stack {
                for (key, value) in styles {
                    run.insert(key.clone(), value.clone());
                }
            }
            runs.push(Value::Object(run));
            continue;
        }
        if cap.get(5).is_some() {
            return Err(cell_error(
                "Literal angle brackets must be escaped in spreadsheet HTML",
            ));
        }
        let name = cap
            .get(2)
            .map(|x| x.as_str().to_ascii_lowercase())
            .ok_or_else(|| cell_error("Malformed spreadsheet HTML"))?;
        if !matches!(
            name.as_str(),
            "b" | "strong" | "i" | "em" | "u" | "s" | "strike" | "span" | "a" | "p" | "div" | "br"
        ) {
            if ignore_unknown {
                continue;
            }
            return Err(cell_error(format!("Unsupported HTML tag {name:?}")));
        }
        let attrs = cap
            .get(3)
            .map_or("", |m| m.as_str())
            .trim()
            .trim_end_matches('/')
            .trim();
        if name == "br" {
            if cap.get(1).is_some_and(|x| x.as_str() == "/") {
                return Err(cell_error("br cannot have a closing tag"));
            }
            if !attrs.is_empty() {
                return Err(cell_error("br does not accept attributes"));
            }
            runs.push(serde_json::json!({"text":"\n"}));
            continue;
        }
        let closing = cap.get(1).is_some_and(|x| x.as_str() == "/");
        if closing {
            if !attrs.is_empty()
                || stack.pop().map(|(tag, _)| tag).as_deref() != Some(name.as_str())
            {
                return Err(cell_error("Spreadsheet HTML tags must be properly nested"));
            }
            if matches!(name.as_str(), "p" | "div") {
                runs.push(serde_json::json!({"text":"\n"}));
            }
        } else {
            let mut style = serde_json::Map::new();
            match name.as_str() {
                "span" => {
                    let css = parse_span_css(attrs)?;
                    style = match parse_inline_css(&css) {
                        Ok(style) => style,
                        Err(_) if ignore_css_errors => serde_json::Map::new(),
                        Err(error) => return Err(error),
                    }
                }
                "a" => {
                    let href = parse_href(attrs)?;
                    if hyperlink.as_deref().is_some_and(|old| old != href) {
                        return Err(cell_error("A cell can contain only one hyperlink target"));
                    }
                    hyperlink = Some(href);
                }
                "p" | "div" => {
                    if !attrs.is_empty() {
                        return Err(cell_error("p and div do not accept attributes"));
                    }
                    runs.push(serde_json::json!({"text":"\n"}));
                }
                _ => {
                    if !attrs.is_empty() {
                        return Err(cell_error("Formatting tags do not accept attributes"));
                    }
                }
            }
            if m.as_str().ends_with("/>") {
                continue;
            }
            stack.push((name, style));
        }
    }
    if cursor != html.len() || !stack.is_empty() {
        return Err(cell_error(
            "Unsupported or unclosed HTML in spreadsheet rich text",
        ));
    }
    if let Some(target) = hyperlink {
        Ok(
            serde_json::json!({"type":"richText","runs":runs,"hyperlink":{"target":target,"display":runs.iter().filter_map(|r|r.get("text").and_then(Value::as_str)).collect::<String>()}}),
        )
    } else {
        Ok(serde_json::json!({"type":"richText","runs":runs}))
    }
}
fn parse_href(attrs: &str) -> Result<String, FillError> {
    let re = regex::Regex::new(r#"(?i)^href\s*=\s*(?:"([^"]*)"|'([^']*)')$"#).unwrap();
    let target = re
        .captures(attrs.trim())
        .and_then(|c| c.get(1).or_else(|| c.get(2)))
        .map(|m| m.as_str())
        .ok_or_else(|| cell_error("a requires one quoted href attribute"))?;
    if target.is_empty() || target.chars().any(char::is_control) {
        return Err(cell_error("HTML hyperlink target is invalid"));
    }
    Ok(decode_html_entities(target))
}
fn parse_span_css(attrs: &str) -> Result<String, FillError> {
    if attrs.is_empty() {
        return Ok(String::new());
    }
    let re = regex::Regex::new(r#"(?is)^style\s*=\s*(?:"([^"]*)"|'([^']*)')$"#).unwrap();
    let css = re
        .captures(attrs.trim())
        .and_then(|c| c.get(1).or_else(|| c.get(2)))
        .map(|m| m.as_str())
        .ok_or_else(|| cell_error("span supports only a quoted style attribute"))?;
    Ok(css.to_string())
}
fn parse_inline_css(style: &str) -> Result<serde_json::Map<String, Value>, FillError> {
    let mut out = serde_json::Map::new();
    if style.is_empty() {
        return Ok(out);
    }
    for item in style.split(';').filter(|part| !part.trim().is_empty()) {
        let (name, value) = item
            .split_once(':')
            .ok_or_else(|| cell_error("Invalid inline HTML CSS declaration"))?;
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        match name.as_str() {
            "font-weight" => match value.to_ascii_lowercase().as_str() {
                "bold" | "600" | "700" | "800" | "900" => {
                    out.insert("bold".into(), Value::Bool(true));
                }
                "normal" | "100" | "200" | "300" | "400" | "500" => {
                    out.insert("bold".into(), Value::Bool(false));
                }
                _ => return Err(cell_error("Unsupported font-weight value")),
            },
            "font-style" => match value.to_ascii_lowercase().as_str() {
                "italic" | "oblique" => {
                    out.insert("italic".into(), Value::Bool(true));
                }
                "normal" => {
                    out.insert("italic".into(), Value::Bool(false));
                }
                _ => return Err(cell_error("Unsupported font-style value")),
            },
            "text-decoration" => {
                for decoration in value.to_ascii_lowercase().split_whitespace() {
                    match decoration {
                        "underline" => {
                            out.insert("underline".into(), Value::Bool(true));
                        }
                        "line-through" => {
                            out.insert("strike".into(), Value::Bool(true));
                        }
                        "none" => {
                            out.insert("underline".into(), Value::Bool(false));
                            out.insert("strike".into(), Value::Bool(false));
                        }
                        _ => return Err(cell_error("Unsupported text-decoration value")),
                    }
                }
            }
            "color" => {
                let color = css_color(value)?;
                out.insert("color".into(), Value::String(color));
            }
            "font-size" => {
                let number = value
                    .strip_suffix("pt")
                    .and_then(|v| v.parse::<f64>().ok())
                    .or_else(|| {
                        value
                            .strip_suffix("px")
                            .and_then(|v| v.parse::<f64>().ok())
                            .map(|v| v * 0.75)
                    })
                    .ok_or_else(|| cell_error("HTML font-size must use pt or px"))?;
                if !(1.0..=409.0).contains(&number) {
                    return Err(cell_error("HTML font-size is outside 1 to 409 points"));
                }
                out.insert("size".into(), serde_json::json!(number));
            }
            "font-family" => {
                let font = value.trim_matches(|c| c == '\'' || c == '\"').trim();
                if font.is_empty() || font.len() > 31 || font.contains(',') {
                    return Err(cell_error(
                        "HTML font-family must name one font of at most 31 characters",
                    ));
                }
                out.insert("font".into(), Value::String(font.into()));
            }
            _ => {
                return Err(cell_error(format!(
                    "Unsupported inline HTML CSS property {name:?}"
                )));
            }
        }
    }
    Ok(out)
}
fn css_color(value: &str) -> Result<String, FillError> {
    let value = value.trim().to_ascii_lowercase();
    let named = match value.as_str() {
        "black" => Some("000000"),
        "white" => Some("FFFFFF"),
        "red" => Some("FF0000"),
        "green" => Some("008000"),
        "blue" => Some("0000FF"),
        "yellow" => Some("FFFF00"),
        "gray" | "grey" => Some("808080"),
        "navy" => Some("000080"),
        "purple" => Some("800080"),
        "orange" => Some("FFA500"),
        _ => None,
    };
    if let Some(color) = named {
        return Ok(color.into());
    }
    let raw = value.trim_start_matches('#');
    if raw.len() == 3 && raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        let mut expanded = String::with_capacity(6);
        for ch in raw.chars() {
            expanded.push(ch);
            expanded.push(ch);
        }
        return Ok(expanded.to_ascii_uppercase());
    }
    if (raw.len() == 6 || raw.len() == 8) && raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(raw.to_ascii_uppercase());
    }
    let rgb =
        regex::Regex::new(r"^rgb\(\s*(\d{1,3})\s*,\s*(\d{1,3})\s*,\s*(\d{1,3})\s*\)$").unwrap();
    if let Some(c) = rgb.captures(&value) {
        let vals: Option<Vec<u8>> = (1..=3)
            .map(|i| c.get(i)?.as_str().parse::<u8>().ok())
            .collect();
        if let Some(vals) = vals {
            return Ok(format!("{:02X}{:02X}{:02X}", vals[0], vals[1], vals[2]));
        }
    }
    Err(cell_error("Unsupported CSS color value"))
}
fn decode_html_entities(text: &str) -> String {
    let decoded = crate::xml::unescape(text);
    decoded
        .replace("&nbsp;", "\u{00a0}")
        .replace("&ndash;", "\u{2013}")
        .replace("&mdash;", "\u{2014}")
        .replace("&apos;", "'")
}
#[allow(clippy::collapsible_if)]
fn validate_raw_runs(raw: &str, prefix: &str) -> Result<String, FillError> {
    let entities = regex::Regex::new(r"&(amp|lt|gt|quot|apos|#[0-9]+|#x[0-9A-Fa-f]+);").unwrap();
    if raw.trim().is_empty()
        || raw.contains("<?")
        || raw.contains("<!")
        || entities.find_iter(raw).count() != raw.matches('&').count()
    {
        return Err(cell_error(
            "Raw rich text contains invalid XML markup or entities",
        ));
    }
    let wrapped = format!("<wrapper>{raw}</wrapper>");
    let all = elements(&wrapped)?;
    let root = all
        .first()
        .ok_or_else(|| cell_error("Raw rich text is empty"))?;
    let direct = direct_xml_children(&all, root);
    if direct.is_empty() || direct.iter().any(|e| local_name(&e.name) != "r") {
        return Err(cell_error(
            "Raw rich text must contain SpreadsheetML r runs",
        ));
    }
    let allowed = [
        "r",
        "rPr",
        "t",
        "b",
        "i",
        "u",
        "strike",
        "color",
        "sz",
        "rFont",
        "vertAlign",
        "charset",
        "family",
        "outline",
        "shadow",
        "condense",
        "extend",
    ];
    let attr_re =
        regex::Regex::new(r#"([A-Za-z_:][A-Za-z0-9_.:-]*)\s*=\s*(?:"([^"]*)"|'([^']*)')"#).unwrap();
    for node in all.iter().skip(1) {
        let local = local_name(&node.name);
        if !allowed.contains(&local) {
            return Err(cell_error("Raw rich text contains an unsupported element"));
        }
        let opening = &wrapped[node.start..node.open_end];
        let tag_name_end = opening
            .char_indices()
            .skip(1)
            .find(|(_, ch)| ch.is_whitespace() || matches!(ch, '/' | '>'))
            .map_or(opening.len(), |(at, _)| at);
        let mut parsed_end = tag_name_end;
        let mut seen_attributes = std::collections::BTreeSet::new();
        for captures in attr_re.captures_iter(opening) {
            let full = captures.get(0).expect("attribute match");
            if !opening[parsed_end..full.start()].trim().is_empty() {
                return Err(cell_error("Raw rich text has malformed attributes"));
            }
            parsed_end = full.end();
            let attr = captures.get(1).expect("attribute name").as_str();
            if !seen_attributes.insert(attr) {
                return Err(cell_error("Raw rich text has a duplicate attribute"));
            }
            let value = captures
                .get(2)
                .or_else(|| captures.get(3))
                .expect("attribute value")
                .as_str();
            let valid = match (local, attr) {
                ("t", "xml:space") => value == "preserve",
                ("color", "rgb") => {
                    (value.len() == 6 || value.len() == 8)
                        && value.bytes().all(|b| b.is_ascii_hexdigit())
                }
                ("color", "indexed") => value.parse::<u8>().is_ok(),
                ("color", "theme") => value.parse::<u8>().is_ok_and(|n| n <= 11),
                ("color", "tint") => value
                    .parse::<f64>()
                    .is_ok_and(|n| n.is_finite() && (-1.0..=1.0).contains(&n)),
                ("color", "auto") => matches!(value, "0" | "1" | "true" | "false"),
                ("sz", "val") => value
                    .parse::<f64>()
                    .is_ok_and(|n| n.is_finite() && (1.0..=409.0).contains(&n)),
                ("rFont", "val") => !value.is_empty() && value.len() <= 31,
                ("vertAlign", "val") => matches!(value, "baseline" | "subscript" | "superscript"),
                ("charset" | "family", "val") => value.parse::<u8>().is_ok(),
                (
                    "b" | "i" | "u" | "strike" | "outline" | "shadow" | "condense" | "extend",
                    "val",
                ) => {
                    matches!(value, "0" | "1" | "true" | "false")
                }
                _ => false,
            };
            if !valid {
                return Err(cell_error("Raw rich text has an unsupported attribute"));
            }
        }
        if !opening[parsed_end..]
            .trim_matches(['>', '/', ' '])
            .is_empty()
        {
            return Err(cell_error("Raw rich text has malformed attributes"));
        }
    }
    for run in &direct {
        let children = direct_xml_children(&all, run);
        if !children.iter().any(|e| local_name(&e.name) == "t")
            || children
                .iter()
                .any(|e| !matches!(local_name(&e.name), "rPr" | "t"))
        {
            return Err(cell_error(
                "Each raw rich text run requires text and optional run properties",
            ));
        }
        let props_position = children.iter().position(|e| local_name(&e.name) == "rPr");
        let text_position = children
            .iter()
            .position(|e| local_name(&e.name) == "t")
            .expect("text run was validated");
        if props_position.is_some_and(|props| props > text_position)
            || children
                .iter()
                .filter(|e| local_name(&e.name) == "rPr")
                .count()
                > 1
            || children
                .iter()
                .filter(|e| local_name(&e.name) == "t")
                .count()
                > 1
        {
            return Err(cell_error(
                "Raw rich text run children are not in SpreadsheetML order",
            ));
        }
        if let Some(props) = children.iter().find(|e| local_name(&e.name) == "rPr") {
            let property_nodes = direct_xml_children(&all, props);
            if property_nodes.iter().any(|e| {
                !matches!(
                    local_name(&e.name),
                    "b" | "i"
                        | "u"
                        | "strike"
                        | "color"
                        | "sz"
                        | "rFont"
                        | "vertAlign"
                        | "charset"
                        | "family"
                        | "outline"
                        | "shadow"
                        | "condense"
                        | "extend"
                )
            }) {
                return Err(cell_error("Raw rich text run properties are invalid"));
            }
            let order = [
                "b",
                "i",
                "strike",
                "outline",
                "shadow",
                "condense",
                "extend",
                "color",
                "sz",
                "u",
                "vertAlign",
                "charset",
                "rFont",
                "family",
            ];
            let mut last = 0usize;
            for (index, node) in property_nodes.iter().enumerate() {
                let Some(position) = order
                    .iter()
                    .position(|name| *name == local_name(&node.name))
                else {
                    continue;
                };
                if index > 0 && position < last {
                    return Err(cell_error(
                        "Raw rich text run properties are not in SpreadsheetML order",
                    ));
                }
                last = position;
            }
        }
    }
    let tag_re = regex::Regex::new(
        r#"(<\s*/?\s*)(?:[A-Za-z_][A-Za-z0-9_.-]*:)?(rPr|r|t|b|i|u|strike|color|sz|rFont|vertAlign|charset|family|outline|shadow|condense|extend)([\s/>])"#,
    )
    .unwrap();
    Ok(tag_re
        .replace_all(raw, |caps: &regex::Captures<'_>| {
            format!("{}{prefix}{}{}", &caps[1], &caps[2], &caps[3])
        })
        .into_owned())
}
fn direct_xml_children<'a>(
    all: &'a [super::xmlutil::Element],
    parent: &super::xmlutil::Element,
) -> Vec<&'a super::xmlutil::Element> {
    let mut out = Vec::new();
    let mut skip = parent.open_end;
    for node in all
        .iter()
        .filter(|e| e.start >= parent.open_end && e.end <= parent.close_start)
    {
        if node.start < skip {
            continue;
        }
        out.push(node);
        skip = node.end;
    }
    out
}
fn parse_date(raw: &str) -> Result<(i32, u32, u32), FillError> {
    let mut p = raw.split('-');
    let y = p.next().and_then(|x| x.parse::<i32>().ok());
    let m = p.next().and_then(|x| x.parse::<u32>().ok());
    let d = p.next().and_then(|x| x.parse::<u32>().ok());
    let (Some(y), Some(m), Some(d)) = (y, m, d) else {
        return Err(cell_error("Date must use YYYY-MM-DD format"));
    };
    if p.next().is_some() || y < 1 || !(1..=12).contains(&m) || d == 0 || d > days_in_month(y, m) {
        return Err(cell_error("Date is not a valid Gregorian calendar date"));
    }
    Ok((y, m, d))
}
fn parse_time(raw: &str) -> Result<f64, FillError> {
    let mut p = raw.split(':');
    let h = p.next().and_then(|x| x.parse::<u32>().ok());
    let m = p.next().and_then(|x| x.parse::<u32>().ok());
    let s = p.next().and_then(|x| x.parse::<f64>().ok());
    let (Some(h), Some(m), Some(s)) = (h, m, s) else {
        return Err(cell_error("Time must use HH:MM:SS format"));
    };
    if p.next().is_some() || h > 23 || m > 59 || !s.is_finite() || !(0.0..60.0).contains(&s) {
        return Err(cell_error("Time is outside the valid 24-hour range"));
    }
    Ok((f64::from(h * 3600 + m * 60) + s) / 86400.0)
}
fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = i64::from(y) - i64::from(m <= 2);
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let m = i64::from(m);
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe
}
fn date_serial(y: i32, m: u32, d: u32, date_1904: bool) -> Result<i64, FillError> {
    let day = days_from_civil(y, m, d);
    if date_1904 {
        let base = days_from_civil(1904, 1, 1);
        if day < base {
            return Err(cell_error(
                "Dates before 1904-01-01 are not supported in the 1904 date system",
            ));
        }
        Ok(day - base)
    } else {
        let base = days_from_civil(1899, 12, 31);
        let mut serial = day - base;
        if (y, m, d) >= (1900, 3, 1) {
            serial += 1;
        }
        if serial < 0 {
            return Err(cell_error("Dates before 1900-01-01 are not supported"));
        }
        Ok(serial)
    }
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
