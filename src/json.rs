//! JSON forms of the fill options, subtemplate options and text edits, for callers such as a
//! wasm build that pass settings through as JSON. Keys are camelCase; an unknown key is an error.

use serde_json::{Map, Value, json};

use crate::comments::CommentDefaults;
use crate::document_edits::{DocumentEdits, Margins, Watermark, WatermarkLayout};
use crate::includes::{IncludeMode, IncludeStyles};
use crate::protection::{HashAlgorithm, Protection, ProtectionKind};
use crate::text::{CommentedText, FilledDocument, TextPosition, TextRange};
use crate::{Delimiters, Extensions, FillError, FillOptions};

impl FillOptions {
    /// These options with the settings in `value` applied over them:
    ///
    /// ```json
    /// {
    ///   "paragraphLoop": true, "linebreaks": true, "delimiters": { "start": "{", "end": "}" },
    ///   "changeDelimiterPrefix": "=" | null, "allowUnopenedTag": false, "allowUnclosedTag": false,
    ///   "extensions": "all" | false | { "tableMerge": true, "htmlExtras": false, … },
    ///   "dropEmptyLastPage": true, "modulePrefixes": ["$"], "dropStyleParagraphs": false,
    ///   "comments": { "author": "…", "initials": "…", "date": "2026-11-02T09:00:00Z" },
    ///   "continuedBookmarks": "tag_", "fillProperties": true, "expressions": true, "filters": ["upper"],
    ///   "document": {
    ///     "readOnly": false, "language": "en-GB",
    ///     "protection": { "password": "…", "protect": "readOnly", "algorithm": "SHA-512", "spinCount": 100000, "salt": "…" },
    ///     "watermark": { "text": "DRAFT", "diagonal": true },
    ///     "margins": { "top": 1, "right": 1, "bottom": 1, "left": 1 }
    ///   }
    /// }
    /// ```
    ///
    /// `extensions` as an object turns each named extension on or off and keeps the rest.
    ///
    /// @throws On an unknown key or a value of the wrong type.
    pub fn read_json(mut self, value: &Value) -> Result<FillOptions, FillError> {
        for (key, value) in read_object(value, "options")? {
            match key.as_str() {
                "paragraphLoop" => self.paragraph_loop = read_bool(value, key)?,
                "linebreaks" => self.linebreaks = read_bool(value, key)?,
                "tagPrefixes" => {
                    for (name, value) in read_object(value, key)? {
                        let field = match name.as_str() {
                            "loopStart" => &mut self.tag_prefixes.loop_start,
                            "loopEnd" => &mut self.tag_prefixes.loop_end,
                            "invertedLoop" => &mut self.tag_prefixes.inverted_loop,
                            "raw" => &mut self.tag_prefixes.raw,
                            "image" => &mut self.tag_prefixes.image,
                            "centeredImage" => &mut self.tag_prefixes.centered_image,
                            "html" => &mut self.tag_prefixes.html,
                            "blockHtml" => &mut self.tag_prefixes.block_html,
                            "wordRun" => &mut self.tag_prefixes.word_run,
                            _ => return Err(unknown(name)),
                        };
                        *field = read_string(value, name)?;
                    }
                    self.tag_prefixes.validate()?;
                }
                "delimiters" => {
                    let delimiters = read_object(value, key)?;
                    let read = |name: &str| delimiters.get(name).map(|each| read_string(each, name)).transpose();
                    let default = Delimiters::default();
                    self.delimiters = Delimiters { start: read("start")?.unwrap_or(default.start), end: read("end")?.unwrap_or(default.end) };
                }
                "allowUnopenedTag" => self.allow_unopened_tag = read_bool(value, key)?,
                "allowUnclosedTag" => self.allow_unclosed_tag = read_bool(value, key)?,
                "changeDelimiterPrefix" => self.change_delimiter_prefix = if value.is_null() { None } else { Some(read_string(value, key)?) },
                "extensions" => self.extensions = read_extensions(value, self.extensions)?,
                "dropEmptyLastPage" => self.drop_empty_last_page = read_bool(value, key)?,
                "modulePrefixes" => self.module_prefixes = read_array(value, key)?.iter().map(|each| read_string(each, key)).collect::<Result<_, _>>()?,
                "dropStyleParagraphs" => self.drop_style_paragraphs = read_bool(value, key)?,
                "comments" => self.comments = read_comment_defaults(value)?,
                "document" => self.document = read_document_edits(value)?,
                "fillProperties" => self.fill_properties = read_bool(value, key)?,
                "continuedBookmarks" => self.continued_bookmarks = Some(read_string(value, key)?),
                "expressions" => self.expressions = read_bool(value, key)?,
                "filters" => self.filters = read_array(value, key)?.iter().map(|each| read_string(each, key)).collect::<Result<_, _>>()?,
                _ => return Err(unknown(key)),
            }
        }
        Ok(self)
    }
}

/// A subtemplate's mode and styles from `{ "mode": "asIs" | "templated", "styles": "useMain" |
/// "keepOwn" }`, templated and using the main document's styles by default.
///
/// @throws On an unknown key or value.
pub fn read_include_options(value: &Value) -> Result<(IncludeMode, IncludeStyles), FillError> {
    let (mut mode, mut styles) = (IncludeMode::Templated, IncludeStyles::UseMain);
    for (key, value) in read_object(value, "subtemplate options")? {
        match (key.as_str(), read_string(value, key)?.as_str()) {
            ("mode", "asIs") => mode = IncludeMode::AsIs,
            ("mode", "templated") => mode = IncludeMode::Templated,
            ("styles", "useMain") => styles = IncludeStyles::UseMain,
            ("styles", "keepOwn") => styles = IncludeStyles::KeepOwn,
            ("mode" | "styles", other) => return Err(FillError::new(format!("Unknown {key} \"{other}\""))),
            _ => return Err(unknown(key)),
        }
    }
    Ok((mode, styles))
}

impl FilledDocument {
    /// Runs each edit in `edits`, a JSON array, in turn. The result of each, in an array:
    ///
    /// - `{ "op": "search", "find": "…" }`: the ranges found, each `{ "start": { "paragraph",
    ///   "offset" }, "end": …, "text": "…" }`.
    /// - `{ "op": "replace", "range": … | "find": "…", "index": 0, "text": "…" }`: `null`. Without
    ///   a range, it replaces match `index` (default 0) of `find`.
    /// - `{ "op": "replaceAll", "find": "…", "text": "…" }`: how many it replaced.
    /// - `{ "op": "addComment", "range": … | "find", "index", "comment": "…", "author",
    ///   "initials", "date" }`: the comment's id. The author defaults to `Author`.
    /// - `{ "op": "comments" }`: each comment as `{ "id", "comment", "author", "initials", "date",
    ///   "range", "xml" }`.
    ///
    /// @throws On an unknown op or key, a missing match, or a failed replace or comment; edits
    /// before it stay made.
    pub fn run_json(&mut self, edits: &Value) -> Result<Value, FillError> {
        let mut results = Vec::new();
        for edit in read_array(edits, "edits")? {
            let edit = read_object(edit, "edit")?;
            let op = edit.get("op").map(|op| read_string(op, "op")).transpose()?.unwrap_or_default();
            let text = || edit.get("text").map_or(Ok(String::new()), |text| read_string(text, "text"));
            let result = match op.as_str() {
                "search" => Value::Array(self.search(&read_field(edit, "find")?).iter().map(write_range).collect()),
                "replace" => {
                    self.replace(&self.read_target(edit)?, &text()?)?;
                    Value::Null
                }
                "replaceAll" => json!(self.replace_all(&read_field(edit, "find")?, &text()?)),
                "addComment" => {
                    let by = read_comment_defaults(&Value::Object(edit.iter().filter(|(key, _)| ["author", "initials", "date"].contains(&key.as_str())).map(|(key, value)| (key.clone(), value.clone())).collect()))?;
                    json!(self.add_comment(&self.read_target(edit)?, &read_field(edit, "comment")?, &by)?)
                }
                "comments" => Value::Array(self.comments().iter().map(write_comment).collect()),
                other => return Err(FillError::new(format!("Unknown op \"{other}\""))),
            };
            results.push(result);
        }
        Ok(Value::Array(results))
    }

    /// The edit's `range`, or match `index` of its `find`.
    fn read_target(&self, edit: &Map<String, Value>) -> Result<TextRange, FillError> {
        if let Some(range) = edit.get("range") {
            return read_range(range);
        }
        let find = read_field(edit, "find")?;
        let index = edit.get("index").map_or(Some(0), Value::as_u64).ok_or_else(|| wrong_type("index"))? as usize;
        self.search(&find).into_iter().nth(index).ok_or_else(|| FillError::new(format!("No match {index} of \"{find}\"")))
    }
}

fn read_extensions(value: &Value, mut extensions: Extensions) -> Result<Extensions, FillError> {
    match value {
        Value::Bool(false) => return Ok(Extensions::NONE),
        Value::Bool(true) => return Ok(Extensions::ALL),
        Value::String(all) if all == "all" => return Ok(Extensions::ALL),
        Value::String(none) if none == "none" => return Ok(Extensions::NONE),
        _ => {}
    }
    for (key, value) in read_object(value, "extensions")? {
        let on = read_bool(value, key)?;
        let field = match key.as_str() {
            "verticalTables" => &mut extensions.vertical_tables,
            "cellStyling" => &mut extensions.cell_styling,
            "richText" => &mut extensions.rich_text,
            "images" => &mut extensions.images,
            "qrCodes" => &mut extensions.qr_codes,
            "tableMerge" => &mut extensions.table_merge,
            "styling" => &mut extensions.styling,
            "wordRun" => &mut extensions.word_run,
            "paragraphPlaceholder" => &mut extensions.paragraph_placeholder,
            "footnotes" => &mut extensions.footnotes,
            "fullTables" => &mut extensions.full_tables,
            "htmlExtras" => &mut extensions.html_extras,
            "centeredImages" => &mut extensions.centered_images,
            "subtemplates" => &mut extensions.subtemplates,
            "comments" => &mut extensions.comments,
            "sections" => &mut extensions.sections,
            _ => return Err(unknown(key)),
        };
        *field = on;
    }
    Ok(extensions)
}

fn read_comment_defaults(value: &Value) -> Result<CommentDefaults, FillError> {
    let mut defaults = CommentDefaults::default();
    for (key, value) in read_object(value, "comments")? {
        match key.as_str() {
            "author" => defaults.author = read_string(value, key)?,
            "initials" => defaults.initials = Some(read_string(value, key)?),
            "date" => defaults.date = Some(read_string(value, key)?),
            _ => return Err(unknown(key)),
        }
    }
    Ok(defaults)
}

fn read_document_edits(value: &Value) -> Result<DocumentEdits, FillError> {
    let mut edits = DocumentEdits::default();
    for (key, value) in read_object(value, "document")? {
        match key.as_str() {
            "readOnly" => edits.read_only = read_bool(value, key)?,
            "language" => edits.language = Some(read_string(value, key)?),
            "protection" => edits.protection = Some(read_protection(value)?),
            "watermark" => {
                let watermark = read_object(value, key)?;
                let is_diagonal = watermark.get("diagonal").map_or(Ok(true), |each| read_bool(each, "diagonal"))?;
                if let Some(other) = watermark.keys().find(|name| !["text", "diagonal"].contains(&name.as_str())) {
                    return Err(unknown(other));
                }
                let layout = if is_diagonal { WatermarkLayout::Diagonal } else { WatermarkLayout::Horizontal };
                edits.watermark = Some(Watermark { text: read_field(watermark, "text")?, layout });
            }
            "margins" => {
                let margins = read_object(value, key)?;
                if let Some(other) = margins.keys().find(|name| !["top", "right", "bottom", "left"].contains(&name.as_str())) {
                    return Err(unknown(other));
                }
                let read = |name: &str| margins.get(name).ok_or_else(|| FillError::new(format!("Missing {name}"))).and_then(|each| each.as_f64().ok_or_else(|| wrong_type(name)));
                edits.margins = Some(Margins { top: read("top")?, right: read("right")?, bottom: read("bottom")?, left: read("left")? });
            }
            _ => return Err(unknown(key)),
        }
    }
    Ok(edits)
}

fn read_protection(value: &Value) -> Result<Protection, FillError> {
    let fields = read_object(value, "protection")?;
    let mut protection = Protection::new(read_field(fields, "password")?);
    for (key, value) in fields {
        match key.as_str() {
            "password" => {}
            "protect" => {
                protection.kind = match read_string(value, key)?.as_str() {
                    "readOnly" => ProtectionKind::ReadOnly,
                    "trackedChanges" => ProtectionKind::TrackedChanges,
                    "forms" => ProtectionKind::Forms,
                    "comments" => ProtectionKind::Comments,
                    other => return Err(FillError::new(format!("Unknown protect \"{other}\""))),
                }
            }
            "algorithm" => {
                protection.algorithm = match read_string(value, key)?.to_ascii_uppercase().replace('-', "").as_str() {
                    "SHA1" => HashAlgorithm::Sha1,
                    "SHA256" => HashAlgorithm::Sha256,
                    "SHA384" => HashAlgorithm::Sha384,
                    "SHA512" => HashAlgorithm::Sha512,
                    _ => return Err(FillError::new(format!("Unknown algorithm \"{value}\""))),
                }
            }
            "spinCount" => protection.spin_count = value.as_u64().and_then(|count| u32::try_from(count).ok()).ok_or_else(|| wrong_type(key))?,
            "salt" => protection.salt = Some(read_string(value, key)?),
            _ => return Err(unknown(key)),
        }
    }
    Ok(protection)
}

fn read_range(value: &Value) -> Result<TextRange, FillError> {
    let range = read_object(value, "range")?;
    let read_position = |name: &str| -> Result<TextPosition, FillError> {
        let position = read_object(range.get(name).ok_or_else(|| FillError::new(format!("Missing {name}")))?, name)?;
        let read = |field: &str| position.get(field).and_then(Value::as_u64).map(|number| number as usize).ok_or_else(|| wrong_type(field));
        Ok(TextPosition { paragraph: read("paragraph")?, offset: read("offset")? })
    };
    Ok(TextRange { start: read_position("start")?, end: read_position("end")?, text: read_field(range, "text")? })
}

fn write_range(range: &TextRange) -> Value {
    let position = |at: TextPosition| json!({ "paragraph": at.paragraph, "offset": at.offset });
    json!({ "start": position(range.start), "end": position(range.end), "text": range.text })
}

fn write_comment(comment: &CommentedText) -> Value {
    json!({
        "id": comment.id,
        "comment": comment.comment,
        "author": comment.author,
        "initials": comment.initials,
        "date": comment.date,
        "range": write_range(&comment.range),
        "xml": comment.xml,
    })
}

fn read_object<'a>(value: &'a Value, name: &str) -> Result<&'a Map<String, Value>, FillError> {
    value.as_object().ok_or_else(|| wrong_type(name))
}

fn read_array<'a>(value: &'a Value, name: &str) -> Result<&'a Vec<Value>, FillError> {
    value.as_array().ok_or_else(|| wrong_type(name))
}

fn read_bool(value: &Value, name: &str) -> Result<bool, FillError> {
    value.as_bool().ok_or_else(|| wrong_type(name))
}

fn read_string(value: &Value, name: &str) -> Result<String, FillError> {
    value.as_str().map(str::to_string).ok_or_else(|| wrong_type(name))
}

fn read_field(object: &Map<String, Value>, name: &str) -> Result<String, FillError> {
    read_string(object.get(name).ok_or_else(|| FillError::new(format!("Missing {name}")))?, name)
}

fn unknown(key: &str) -> FillError {
    FillError::new(format!("Unknown option \"{key}\""))
}

fn wrong_type(name: &str) -> FillError {
    FillError::new(format!("Wrong type for {name}"))
}
