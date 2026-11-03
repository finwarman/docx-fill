use std::collections::BTreeMap;

use serde_json::Value;

use crate::expressions::{Assignments, Evaluation, Expression};
use crate::render::FillHooks;
use crate::{Delimiters, FillError, FillOptions};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Value,
    Open { inverted: bool },
    Close,
    Change,
}

#[derive(Debug)]
pub(crate) struct Tag {
    pub start: usize,
    pub end: usize,
    pub name: String,
    pub written: String,
    pub kind: Kind,
    expression: Option<Expression>,
}

pub(crate) struct Context<'a> {
    pub hooks: &'a mut dyn FillHooks,
    pub assignments: Assignments,
    pub part: &'a str,
}

impl<'a> Context<'a> {
    pub(crate) fn new(part: &'a str, hooks: &'a mut dyn FillHooks) -> Self {
        Self {
            hooks,
            assignments: Assignments::default(),
            part,
        }
    }

    pub(crate) fn value(
        &mut self,
        tag: &Tag,
        scopes: &[&Value],
        indexes: &[usize],
    ) -> Option<Value> {
        if let Some(value) = self
            .hooks
            .tag_value(&tag.name, scopes)
            .filter(|value| !value.is_null())
        {
            return Some(value);
        }
        if let Some(expression) = &tag.expression {
            let mut evaluation = Evaluation {
                scopes,
                loop_indexes: indexes,
                hooks: self.hooks,
                assignments: &mut self.assignments,
            };
            return evaluation
                .evaluate(expression)
                .map(|value| value.into_owned())
                .filter(|value| !value.is_null());
        }
        if tag.name == "." {
            return scopes.last().copied().cloned();
        }
        for scope in scopes.iter().rev() {
            if let Some(value) = scope.get(&tag.name).filter(|value| !value.is_null()) {
                return Some(value.clone());
            }
        }
        None
    }
}

pub(crate) fn shown(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Null) | None => false,
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) => value.as_f64().is_some_and(|number| number != 0.0),
        Some(Value::String(value)) => !value.is_empty(),
        Some(Value::Object(_)) => true,
    }
}

pub(crate) fn parse(
    text: &str,
    options: &FillOptions,
    delimiters: &mut Delimiters,
) -> Result<Vec<Tag>, FillError> {
    let mut errors = Vec::new();
    let found = crate::template::find_tags(text, delimiters, options, 0, &mut errors);
    if !errors.is_empty() {
        return Err(FillError::many(errors));
    }
    let mut cache: BTreeMap<String, Expression> = BTreeMap::new();
    let mut tags = Vec::with_capacity(found.len());
    for found in found {
        let written = crate::xml::unescape(&text[found.name.clone()]);
        let (parsed, name) =
            if options.expressions && written.starts_with('$') && !written.starts_with("${") {
                (crate::template::TagKind::Value, written.clone())
            } else {
                crate::template::read_tag(&written, options)
            };
        let kind = if found.changes_delimiters {
            Kind::Change
        } else {
            match parsed {
                crate::template::TagKind::Value => Kind::Value,
                crate::template::TagKind::Open {
                    is_inverted,
                    element: None,
                } => Kind::Open {
                    inverted: is_inverted,
                },
                crate::template::TagKind::Close => Kind::Close,
                _ => {
                    return Err(FillError::new(format!(
                        "Unsupported tag in spreadsheet cell: {written}"
                    )));
                }
            }
        };
        let expression = if matches!(kind, Kind::Value | Kind::Open { .. }) && options.expressions {
            if let Some(expression) = cache.get(&name) {
                Some(expression.clone())
            } else {
                let expression = Expression::parse(&name, &options.filters).map_err(|reason| {
                    FillError::new(format!(
                        "The tag \"{name}\" is not a valid expression: {reason}"
                    ))
                    .with_id(crate::FillErrorId::ScopeParserCompilation)
                })?;
                if expression.has_assignment() {
                    return Err(FillError::new(
                        "Assignments are not supported in spreadsheet cell tags",
                    )
                    .with_id(crate::FillErrorId::SpreadsheetCell));
                }
                cache.insert(name.clone(), expression.clone());
                Some(expression)
            }
        } else {
            None
        };
        tags.push(Tag {
            start: found.start,
            end: found.end,
            name,
            written,
            kind,
            expression,
        });
    }
    Ok(tags)
}
