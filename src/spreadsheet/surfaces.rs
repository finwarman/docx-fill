//! Header/footer and DrawingML text surfaces in worksheets.
use super::package_util::{self as pkg, Parts};
use super::text::{self, Context, Kind, Tag};
use super::xmlutil::{elements, escaped, local_name};
use crate::render::print_value;
use crate::xml::unescape;
use crate::{Delimiters, FillError, FillErrorId, FillHooks, FillOptions};
use serde_json::Value;

pub(super) fn apply(
    parts: &mut Parts,
    sheet: &str,
    options: &FillOptions,
    data: &Value,
    hooks: &mut dyn FillHooks,
) -> Result<(), FillError> {
    let source = pkg::xml(parts, sheet)?.to_string();
    let mut edits = Vec::new();
    for e in elements(&source)?.into_iter().filter(|e| {
        matches!(
            local_name(&e.name),
            "oddHeader" | "evenHeader" | "firstHeader" | "oddFooter" | "evenFooter" | "firstFooter"
        )
    }) {
        if e.open_end == e.close_start {
            continue;
        }
        let value = unescape(&source[e.open_end..e.close_start]);
        let filled = fill_text(&value, options, data, hooks, sheet, true)?;
        if filled != value {
            if filled.encode_utf16().count() > 255 {
                return Err(
                    fail("Excel headers and footers are limited to 255 UTF-16 code units")
                        .in_part(sheet),
                );
            }
            edits.push((e.open_end, e.close_start, escaped(&filled)));
        }
    }
    if !edits.is_empty() {
        let mut out = source;
        for (start, end, value) in edits.into_iter().rev() {
            out.replace_range(start..end, &value);
        }
        parts.insert(sheet.into(), out.into_bytes());
    }
    let paths = pkg::targets(parts, sheet, &format!("{}/drawing", pkg::OFFICE_NS))?;
    for (_, path) in paths {
        let source = pkg::xml(parts, &path)?.to_string();
        let mut edits = Vec::new();
        let all = elements(&source)?;
        for paragraph in all.iter().filter(|e| local_name(&e.name) == "p") {
            let texts: Vec<_> = all
                .iter()
                .filter(|e| {
                    local_name(&e.name) == "t"
                        && e.start >= paragraph.open_end
                        && e.end <= paragraph.close_start
                })
                .collect();
            if texts.is_empty() {
                continue;
            }
            let values: Vec<_> = texts
                .iter()
                .map(|e| unescape(&source[e.open_end..e.close_start]))
                .collect();
            let joined = values.concat();
            let filled = fill_text(&joined, options, data, hooks, &path, false)?;
            if joined == filled {
                continue;
            }
            let prefix = joined
                .chars()
                .zip(filled.chars())
                .take_while(|(a, b)| a == b)
                .map(|(c, _)| c.len_utf8())
                .sum::<usize>();
            let suffix = joined[prefix..]
                .chars()
                .rev()
                .zip(filled[prefix..].chars().rev())
                .take_while(|(a, b)| a == b)
                .map(|(c, _)| c.len_utf8())
                .sum::<usize>();
            let old_end = joined.len() - suffix;
            let replacement = &filled[prefix..filled.len() - suffix];
            let mut offset = 0;
            let mut inserted = false;
            for (node, value) in texts.iter().zip(values) {
                let end = offset + value.len();
                if end >= prefix && offset <= old_end {
                    let from = prefix.saturating_sub(offset).min(value.len());
                    let to = old_end.saturating_sub(offset).min(value.len());
                    if from <= to {
                        let next = format!(
                            "{}{}{}",
                            &value[..from],
                            if inserted { "" } else { replacement },
                            &value[to..]
                        );
                        inserted = true;
                        edits.push((node.open_end, node.close_start, escaped(&next)));
                    }
                }
                offset = end;
            }
        }
        if !edits.is_empty() {
            let mut out = source;
            edits.sort_by_key(|e| e.0);
            for (start, end, value) in edits.into_iter().rev() {
                out.replace_range(start..end, &value);
            }
            parts.insert(path, out.into_bytes());
        }
    }
    Ok(())
}
fn fill_text(
    source: &str,
    options: &FillOptions,
    data: &Value,
    hooks: &mut dyn FillHooks,
    part: &str,
    header: bool,
) -> Result<String, FillError> {
    let mut delimiters: Delimiters = options.delimiters.clone();
    let tags = text::parse(source, options, &mut delimiters).map_err(|e| e.in_part(part))?;
    if tags.is_empty() {
        return Ok(source.into());
    }
    let mut ctx = Context::new(part, hooks, false);
    render(
        source,
        &tags,
        0,
        tags.len(),
        0,
        source.len(),
        &mut ctx,
        &[data],
        &[],
        header,
        0,
    )
    .map_err(|e| e.in_part(part))
}
#[allow(clippy::too_many_arguments)]
fn render(
    source: &str,
    tags: &[Tag],
    first: usize,
    last: usize,
    start: usize,
    end: usize,
    ctx: &mut Context<'_>,
    scopes: &[&Value],
    indexes: &[usize],
    header: bool,
    depth: usize,
) -> Result<String, FillError> {
    if depth > 64 {
        return Err(fail("Surface loops exceed 64 levels"));
    }
    let mut out = String::new();
    let mut at = start;
    let mut i = first;
    while i < last {
        let tag = &tags[i];
        out.push_str(&source[at..tag.start]);
        match tag.kind {
            Kind::Value => {
                let value = ctx
                    .value(tag, scopes, indexes)
                    .map(|v| print_value(&v))
                    .or_else(|| {
                        ctx.hooks.missing_value(
                            ctx.part,
                            crate::MissingValue {
                                kind: crate::MissingKind::Value,
                                name: &tag.name,
                                written: &tag.written,
                            },
                        )
                    })
                    .unwrap_or_default();
                if header {
                    out.push_str(&value.replace('&', "&&"));
                } else {
                    out.push_str(&value);
                }
                at = tag.end;
                i += 1;
            }
            Kind::Change => {
                at = tag.end;
                i += 1;
            }
            Kind::Open { inverted, .. } => {
                let mut nesting = 1;
                let mut close = i + 1;
                while close < last {
                    match tags[close].kind {
                        Kind::Open { .. } => nesting += 1,
                        Kind::Close { .. } => {
                            nesting -= 1;
                            if nesting == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    close += 1;
                }
                if close == last {
                    return Err(fail("Unclosed loop in spreadsheet text surface"));
                }
                if !tags[close].name.is_empty() && tags[close].name != tag.name {
                    return Err(fail("Mismatched loop in spreadsheet text surface"));
                }
                let value = ctx.value(tag, scopes, indexes);
                if inverted {
                    if !text::shown(value.as_ref()) {
                        out.push_str(&render(
                            source,
                            tags,
                            i + 1,
                            close,
                            tag.end,
                            tags[close].start,
                            ctx,
                            scopes,
                            indexes,
                            header,
                            depth + 1,
                        )?);
                    }
                } else if let Some(Value::Array(items)) = &value {
                    for (index, item) in items.iter().enumerate() {
                        let mut child = scopes.to_vec();
                        child.push(item);
                        let mut indices = indexes.to_vec();
                        indices.push(index);
                        out.push_str(&render(
                            source,
                            tags,
                            i + 1,
                            close,
                            tag.end,
                            tags[close].start,
                            ctx,
                            &child,
                            &indices,
                            header,
                            depth + 1,
                        )?);
                        if out.len() > 1_048_576 {
                            return Err(fail("Spreadsheet surface text exceeds its limit"));
                        }
                    }
                } else if text::shown(value.as_ref()) {
                    let value = value.as_ref().expect("shown value");
                    let mut child = scopes.to_vec();
                    child.push(value);
                    out.push_str(&render(
                        source,
                        tags,
                        i + 1,
                        close,
                        tag.end,
                        tags[close].start,
                        ctx,
                        &child,
                        indexes,
                        header,
                        depth + 1,
                    )?);
                }
                at = tags[close].end;
                i = close + 1;
            }
            Kind::Close { .. } => return Err(fail("Unopened loop in spreadsheet text surface")),
            _ => {
                return Err(fail(
                    "This tag type is not supported in spreadsheet text surfaces",
                ));
            }
        }
        if out.len() > 1_048_576 {
            return Err(fail("Spreadsheet surface text exceeds its limit"));
        }
    }
    out.push_str(&source[at..end]);
    Ok(out)
}
fn fail(message: &str) -> FillError {
    pkg::error(message).with_id(FillErrorId::SpreadsheetCell)
}
