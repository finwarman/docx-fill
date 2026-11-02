//! Text search, replacement and comments on a filled package's body, as docxtemplater's meta
//! module documents `search`, `replace`, `replaceAll`, `addComment` and `getComments`. Built from
//! the WordprocessingML spec. A paragraph's text is its runs' `<w:t>` text, with `<w:tab/>` as a
//! tab and `<w:br/>` or `<w:cr/>` as a line break.

use std::borrow::Cow;
use std::collections::BTreeSet;

use crate::comments::{COMMENTS_PART, Comment, CommentDefaults, add_comments};
use crate::properties::set_attribute;
use crate::xml::{ElementKind, Token, escape, read_attribute, tokenize, unescape};
use crate::{DOCUMENT_PART, FillError};

mod bookmarks;
pub use bookmarks::Bookmark;

/// A place in the body's text: a paragraph, counted from 0 in document order with paragraphs in
/// tables and text boxes included, and a character offset in its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextPosition {
    pub paragraph: usize,
    pub offset: usize,
}

/// Text between two positions, such as a search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextRange {
    pub start: TextPosition,
    pub end: TextPosition,
    /// The text the range held when found, with paragraphs joined by `\n`.
    pub text: String,
}

/// A comment in the body and the text it marks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentedText {
    pub id: u32,
    /// The comment's own text, its paragraphs joined by `\n`.
    pub comment: String,
    pub author: Option<String>,
    pub initials: Option<String>,
    pub date: Option<String>,
    pub range: TextRange,
    /// The body XML between the comment's range marks.
    pub xml: String,
}

/// A filled package whose body text can be searched, replaced and commented.
pub struct FilledDocument {
    parts: Vec<(String, Cow<'static, [u8]>)>,
}

/// One stretch of a paragraph's text: a `<w:t>`'s text, or a tab or break element.
struct Piece {
    /// Its first character in the paragraph's text.
    offset: usize,
    /// Its raw XML: the text between `<w:t>` and `</w:t>`, or the whole element.
    raw: (usize, usize),
    /// The `<w:t …>` start tag around text, `None` for an element.
    text_open: Option<(usize, usize)>,
    /// The run's `<w:rPr>…</w:rPr>`, when it has one.
    run_props: Option<(usize, usize)>,
    text: String,
}

#[derive(Default)]
struct Paragraph {
    text: String,
    pieces: Vec<Piece>,
}

/// A comment range mark's place, in the body XML and its text.
struct Mark {
    id: u32,
    is_start: bool,
    xml: (usize, usize),
    position: TextPosition,
}

/// The body's paragraphs and comment marks.
struct BodyText {
    paragraphs: Vec<Paragraph>,
    marks: Vec<Mark>,
}

/// A change to the body XML: replace bytes `start..end` with `text`.
type XmlEdit = (usize, usize, String);

impl FilledDocument {
    pub fn new(parts: impl IntoIterator<Item = (String, Vec<u8>)>) -> FilledDocument {
        FilledDocument { parts: parts.into_iter().map(|(name, bytes)| (name, Cow::Owned(bytes))).collect() }
    }

    pub fn into_parts(self) -> Vec<(String, Vec<u8>)> {
        self.parts.into_iter().map(|(name, bytes)| (name, bytes.into_owned())).collect()
    }

    /// Each place `find` occurs within one paragraph, in document order, not overlapping. `\n`
    /// matches a line break and `\t` a tab. An empty `find` finds nothing.
    pub fn search(&self, find: &str) -> Vec<TextRange> {
        let body = read_body_text(&self.read_document());
        find_all(&body, find)
    }

    /// Replaces a range's text with `text`, in the run of the range's first character.
    ///
    /// @throws When the range spans paragraphs, or the body no longer holds the range's text there.
    pub fn replace(&mut self, range: &TextRange, text: &str) -> Result<(), FillError> {
        let xml = self.read_document();
        let body = read_body_text(&xml);
        check_range(&body, range)?;
        let mut opens = BTreeSet::new();
        let edits = replacement_edits(&xml, &body.paragraphs[range.start.paragraph], range, text, &mut opens);
        self.write_document(apply(&xml, edits));
        Ok(())
    }

    /// Replaces every place `find` occurs, as `search` finds them, with `text`. How many it replaced.
    pub fn replace_all(&mut self, find: &str, text: &str) -> usize {
        let xml = self.read_document();
        let body = read_body_text(&xml);
        let ranges = find_all(&body, find);
        let mut opens = BTreeSet::new();
        let edits = ranges.iter().flat_map(|range| replacement_edits(&xml, &body.paragraphs[range.start.paragraph], range, text, &mut opens)).collect();
        self.write_document(apply(&xml, edits));
        ranges.len()
    }

    /// Adds a comment around a range, written by `by`'s author, initials and date. The comment's id.
    ///
    /// @throws When the range is empty, spans paragraphs, or no longer holds its text.
    pub fn add_comment(&mut self, range: &TextRange, comment: &str, by: &CommentDefaults) -> Result<u32, FillError> {
        self.add_part_comment(DOCUMENT_PART, range, comment, by)
    }

    /// Comments on located diagnostics in Word paragraphs. Returns the number annotated.
    /// Diagnostics without a usable paragraph/offset remain available on `error`.
    pub fn annotate_errors(&mut self, error: &FillError, by: &CommentDefaults) -> Result<usize, FillError> {
        let mut count = 0;
        for error in error.diagnostics() {
            let part = error.part.as_deref().unwrap_or(DOCUMENT_PART);
            let Some(xml) = self.read_part(part) else { continue };
            let body = read_body_text(&xml);
            let Some(range) = error_range(&xml, &body, error) else { continue };
            let message = format!("{}: {}", error.id.as_str(), error.message);
            self.add_part_comment(part, &range, &message, by)?;
            count += 1;
        }
        Ok(count)
    }

    fn add_part_comment(&mut self, part: &str, range: &TextRange, comment: &str, by: &CommentDefaults) -> Result<u32, FillError> {
        let xml = self.read_part(part).ok_or_else(|| FillError::new("The comment part is missing"))?;
        let body = read_body_text(&xml);
        check_range(&body, range)?;
        if range.start.offset == range.end.offset {
            return Err(FillError::new("The range to comment is empty"));
        }
        let written = read_comments(&self.read_part(COMMENTS_PART).unwrap_or_default());
        let id = written.iter().map(|comment| comment.0).chain(body.marks.iter().map(|mark| mark.id)).max().map_or(0, |max| max + 1);

        let paragraph = &body.paragraphs[range.start.paragraph];
        let start_mark = format!("<w:commentRangeStart w:id=\"{id}\"/>");
        let end_mark = format!("<w:commentRangeEnd w:id=\"{id}\"/><w:r><w:rPr><w:rStyle w:val=\"CommentReference\"/></w:rPr><w:commentReference w:id=\"{id}\"/></w:r>");
        let mut opens = BTreeSet::new();
        let mut edits = split_at(&xml, paragraph, range.start.offset, &start_mark, true, &mut opens);
        edits.extend(split_at(&xml, paragraph, range.end.offset, &end_mark, false, &mut opens));
        if let Some((_, bytes)) = self.parts.iter_mut().find(|(name, _)| name == part) {
            *bytes = Cow::Owned(apply(&xml, edits).into_bytes());
        }

        let written = Comment { id, text: comment.to_string(), author: by.author.clone(), initials: by.initials.clone(), date: by.date.clone() };
        add_comments(&mut self.parts, &[written]);
        Ok(id)
    }

    /// The body's comments with the text they mark, in the order their ranges start.
    pub fn comments(&self) -> Vec<CommentedText> {
        let xml = self.read_document();
        let body = read_body_text(&xml);
        let written = read_comments(&self.read_part(COMMENTS_PART).unwrap_or_default());
        let mut found = Vec::new();
        for start in body.marks.iter().filter(|mark| mark.is_start) {
            let Some(end) = body.marks.iter().find(|mark| !mark.is_start && mark.id == start.id) else { continue };
            let Some((_, comment, author, initials, date)) = written.iter().find(|each| each.0 == start.id) else { continue };
            let text = read_between(&body, start.position, end.position);
            found.push(CommentedText {
                id: start.id,
                comment: comment.clone(),
                author: author.clone(),
                initials: initials.clone(),
                date: date.clone(),
                range: TextRange { start: start.position, end: end.position, text },
                xml: xml.get(start.xml.1..end.xml.0).unwrap_or_default().to_string(),
            });
        }
        found
    }

    fn read_part(&self, name: &str) -> Option<String> {
        self.parts.iter().find(|(each, _)| each == name).map(|(_, bytes)| String::from_utf8_lossy(bytes).into_owned())
    }

    fn read_document(&self) -> String {
        self.read_part(DOCUMENT_PART).unwrap_or_default()
    }

    fn write_document(&mut self, xml: String) {
        if let Some(part) = self.parts.iter_mut().find(|(name, _)| name == DOCUMENT_PART) {
            part.1 = Cow::Owned(xml.into_bytes());
        }
    }
}

fn error_range(xml: &str, body: &BodyText, error: &FillError) -> Option<TextRange> {
    let index = error.paragraph?;
    let paragraph = body.paragraphs.get(index)?;
    let mut remaining = error.offset?;
    let mut start = None;
    for piece in &paragraph.pieces {
        if piece.text_open.is_none() { continue; }
        let raw = &xml[piece.raw.0..piece.raw.1];
        if remaining < raw.len() {
            let prefix = raw.get(..remaining)?;
            start = Some(piece.offset + unescape(prefix).chars().count());
            break;
        }
        remaining -= raw.len();
    }
    let start = start?;
    let chars: Vec<_> = paragraph.text.chars().collect();
    if start >= chars.len() { return None; }
    let tail: String = chars[start..].iter().collect();
    let length = match error.tag.as_deref().filter(|tag| !tag.is_empty()) {
        Some(tag) => match tail.find(tag) {
            Some(at) => tail[..at + tag.len()].chars().count(),
            None if matches!(error.id, crate::FillErrorId::UnopenedTag | crate::FillErrorId::DuplicateCloseTag) => 1,
            None => return None,
        },
        None => 1,
    };
    let end = (start + length).min(chars.len());
    Some(TextRange { start: TextPosition { paragraph: index, offset: start }, end: TextPosition { paragraph: index, offset: end }, text: chars[start..end].iter().collect() })
}

fn read_body_text(xml: &str) -> BodyText {
    let mut body = BodyText { paragraphs: Vec::new(), marks: Vec::new() };
    let Ok(tokens) = tokenize(xml) else { return body };
    // Each open paragraph, with how many runs were open when it opened.
    let mut open: Vec<(usize, usize)> = Vec::new();
    // Each open run's properties.
    let mut runs: Vec<Option<(usize, usize)>> = Vec::new();
    let mut run_props_start = None;
    let mut text_open = None;
    let mut at = 0;
    for token in tokens {
        let length = match token {
            Token::Element { raw, .. } => raw.len(),
            Token::Text(text) => text.len(),
        };
        let in_run = open.last().is_some_and(|&(_, base)| runs.len() > base);
        match token {
            Token::Element { kind: ElementKind::Open, name: "w:p", .. } => {
                open.push((body.paragraphs.len(), runs.len()));
                body.paragraphs.push(Paragraph::default());
            }
            Token::Element { kind: ElementKind::SelfClosing, name: "w:p", .. } => body.paragraphs.push(Paragraph::default()),
            Token::Element { kind: ElementKind::Close, name: "w:p", .. } => _ = open.pop(),
            Token::Element { kind: ElementKind::Open, name: "w:r", .. } => runs.push(None),
            Token::Element { kind: ElementKind::Close, name: "w:r", .. } => _ = runs.pop(),
            Token::Element { kind: ElementKind::Open, name: "w:rPr", .. } if in_run && runs.last() == Some(&None) => run_props_start = Some(at),
            Token::Element { kind: ElementKind::Close, name: "w:rPr", .. } => {
                if let (Some(start), Some(run)) = (run_props_start.take(), runs.last_mut()) {
                    *run = Some((start, at + length));
                }
            }
            Token::Element { kind: ElementKind::Open, name: "w:t", .. } if in_run => text_open = Some((at, at + length)),
            Token::Element { kind: ElementKind::Close, name: "w:t", .. } => text_open = None,
            Token::Text(text) if in_run && text_open.is_some() => push_piece(&mut body, &open, &runs, (at, at + length), text_open, unescape(text)),
            Token::Element { kind: ElementKind::SelfClosing, name: name @ ("w:tab" | "w:br" | "w:cr"), .. } if in_run => {
                let text = if name == "w:tab" { "\t" } else { "\n" };
                push_piece(&mut body, &open, &runs, (at, at + length), None, text.to_string());
            }
            Token::Element { kind: ElementKind::SelfClosing, name: name @ ("w:commentRangeStart" | "w:commentRangeEnd"), raw } => {
                let Some(id) = read_attribute(raw, "w:id").and_then(|id| id.parse().ok()) else {
                    at += length;
                    continue;
                };
                let is_start = name == "w:commentRangeStart";
                let position = match open.last() {
                    Some(&(index, _)) => TextPosition { paragraph: index, offset: body.paragraphs[index].text.chars().count() },
                    // Between paragraphs, a start marks the next one and an end the last.
                    None if is_start => TextPosition { paragraph: body.paragraphs.len(), offset: 0 },
                    None => {
                        let paragraph = body.paragraphs.len().saturating_sub(1);
                        TextPosition { paragraph, offset: body.paragraphs.get(paragraph).map_or(0, |each| each.text.chars().count()) }
                    }
                };
                body.marks.push(Mark { id, is_start, xml: (at, at + length), position });
            }
            _ => {}
        }
        at += length;
    }
    body
}

fn push_piece(body: &mut BodyText, open: &[(usize, usize)], runs: &[Option<(usize, usize)>], raw: (usize, usize), text_open: Option<(usize, usize)>, text: String) {
    let Some(&(index, _)) = open.last() else { return };
    let paragraph = &mut body.paragraphs[index];
    let offset = paragraph.text.chars().count();
    paragraph.text.push_str(&text);
    paragraph.pieces.push(Piece { offset, raw, text_open, run_props: runs.last().copied().flatten(), text });
}

fn find_all(body: &BodyText, find: &str) -> Vec<TextRange> {
    if find.is_empty() {
        return Vec::new();
    }
    let mut found = Vec::new();
    for (index, paragraph) in body.paragraphs.iter().enumerate() {
        let mut from = 0;
        while let Some(at) = paragraph.text[from..].find(find).map(|at| from + at) {
            let start = paragraph.text[..at].chars().count();
            let end = start + find.chars().count();
            found.push(TextRange { start: TextPosition { paragraph: index, offset: start }, end: TextPosition { paragraph: index, offset: end }, text: find.to_string() });
            from = at + find.len();
        }
    }
    found
}

fn check_range(body: &BodyText, range: &TextRange) -> Result<(), FillError> {
    if range.start.paragraph != range.end.paragraph {
        return Err(FillError::new("The range spans paragraphs"));
    }
    let held = body.paragraphs.get(range.start.paragraph).and_then(|paragraph| {
        let chars: Vec<char> = paragraph.text.chars().collect();
        chars.get(range.start.offset..range.end.offset).map(|slice| slice.iter().collect::<String>())
    });
    if held.as_deref() != Some(range.text.as_str()) {
        return Err(FillError::new("The document no longer holds the range's text"));
    }
    Ok(())
}

/// The edits that replace a checked one-paragraph range with `text`. `opens` holds the `<w:t>`
/// tags already given `xml:space="preserve"`.
fn replacement_edits(xml: &str, paragraph: &Paragraph, range: &TextRange, text: &str, opens: &mut BTreeSet<usize>) -> Vec<XmlEdit> {
    let (start, end) = (range.start.offset, range.end.offset);
    let mut edits = Vec::new();
    let mut is_written = false;
    for piece in &paragraph.pieces {
        let length = piece.text.chars().count();
        let (first, last) = (start.max(piece.offset), end.min(piece.offset + length));
        let is_holder = !is_written && (first < last || (start == end && start >= piece.offset && start <= piece.offset + length));
        if first >= last && !is_holder {
            continue;
        }
        let mut written = String::new();
        if is_holder {
            escape(text, &mut written);
            is_written = true;
        }
        let Some(open) = piece.text_open else {
            // A tab or break: removed, or swapped for a text element holding the replacement.
            let replacement = if written.is_empty() { String::new() } else { format!("<w:t xml:space=\"preserve\">{written}</w:t>") };
            edits.push((piece.raw.0, piece.raw.1, replacement));
            continue;
        };
        let raw = &xml[piece.raw.0..piece.raw.1];
        let (from, to) = (byte_at_char(raw, first - piece.offset), byte_at_char(raw, last.max(first) - piece.offset));
        edits.push((piece.raw.0 + from, piece.raw.0 + to, written));
        edits.extend(preserve_spaces(xml, open, opens));
    }
    edits
}

/// An edit giving a `<w:t>` tag `xml:space="preserve"`, unless `opens` shows it has one already.
fn preserve_spaces(xml: &str, open: (usize, usize), opens: &mut BTreeSet<usize>) -> Option<XmlEdit> {
    let raw = &xml[open.0..open.1];
    if !opens.insert(open.0) || read_attribute(raw, "xml:space") == Some("preserve") {
        return None;
    }
    Some((open.0, open.1, set_attribute(raw, "xml:space", "preserve")))
}

/// The byte offset in escaped XML text `raw` of its `chars`-th character.
fn byte_at_char(raw: &str, chars: usize) -> usize {
    let mut at = 0;
    for _ in 0..chars {
        let Some(c) = raw[at..].chars().next() else { return raw.len() };
        at += match raw[at..].find(';').filter(|_| c == '&') {
            Some(semi) => semi + 1,
            None => c.len_utf8(),
        };
    }
    at
}

/// An edit that splits the paragraph's text at `offset` and puts `mark` between the halves.
/// `is_start` places it before the character at `offset`, otherwise after the one before it.
fn split_at(xml: &str, paragraph: &Paragraph, offset: usize, mark: &str, is_start: bool, opens: &mut BTreeSet<usize>) -> Vec<XmlEdit> {
    let piece = if is_start {
        paragraph.pieces.iter().find(|piece| piece.offset <= offset && offset < piece.offset + piece.text.chars().count())
    } else {
        paragraph.pieces.iter().find(|piece| piece.offset < offset && offset <= piece.offset + piece.text.chars().count())
    }
    .expect("checked range has a piece at each end");
    let props = piece.run_props.map_or("", |(start, end)| &xml[start..end]);
    let Some(open) = piece.text_open else {
        let at = if is_start { piece.raw.0 } else { piece.raw.1 };
        return vec![(at, at, format!("</w:r>{mark}<w:r>{props}"))];
    };
    let at = piece.raw.0 + byte_at_char(&xml[piece.raw.0..piece.raw.1], offset - piece.offset);
    let mut edits = vec![(at, at, format!("</w:t></w:r>{mark}<w:r>{props}<w:t xml:space=\"preserve\">"))];
    edits.extend(preserve_spaces(xml, open, opens));
    edits
}

/// The text between two positions, paragraphs joined by `\n`.
fn read_between(body: &BodyText, start: TextPosition, end: TextPosition) -> String {
    let mut text = String::new();
    for index in start.paragraph..=end.paragraph.min(body.paragraphs.len().saturating_sub(1)) {
        let chars = body.paragraphs[index].text.chars();
        let from = if index == start.paragraph { start.offset } else { 0 };
        let taken: String = if index == end.paragraph { chars.take(end.offset).skip(from).collect() } else { chars.skip(from).collect() };
        if index > start.paragraph {
            text.push('\n');
        }
        text.push_str(&taken);
    }
    text
}

/// Each comment in a comments part: id, text, author, initials and date.
#[allow(clippy::type_complexity)]
fn read_comments(xml: &str) -> Vec<(u32, String, Option<String>, Option<String>, Option<String>)> {
    let mut comments = Vec::new();
    let Ok(tokens) = tokenize(xml) else { return comments };
    let mut is_in_text = false;
    let mut paragraphs = 0;
    for token in tokens {
        match token {
            Token::Element { kind: ElementKind::Open, name: "w:comment", raw } => {
                let Some(id) = read_attribute(raw, "w:id").and_then(|id| id.parse().ok()) else { continue };
                let field = |name: &str| read_attribute(raw, name).map(unescape);
                comments.push((id, String::new(), field("w:author"), field("w:initials"), field("w:date")));
                paragraphs = 0;
            }
            Token::Element { kind: ElementKind::Open | ElementKind::SelfClosing, name: "w:p", .. } => {
                if let Some(comment) = comments.last_mut().filter(|_| paragraphs > 0) {
                    comment.1.push('\n');
                }
                paragraphs += 1;
            }
            Token::Element { kind, name: "w:t", .. } => is_in_text = kind == ElementKind::Open,
            Token::Text(text) if is_in_text => {
                if let Some(comment) = comments.last_mut() {
                    comment.1.push_str(&unescape(text));
                }
            }
            _ => {}
        }
    }
    comments
}

fn apply(xml: &str, mut edits: Vec<XmlEdit>) -> String {
    edits.sort_by_key(|edit| (edit.0, edit.1));
    let mut out = String::with_capacity(xml.len());
    let mut at = 0;
    for (start, end, text) in edits {
        if start < at {
            continue;
        }
        out.push_str(&xml[at..start]);
        out.push_str(&text);
        at = end;
    }
    out.push_str(&xml[at..]);
    out
}
