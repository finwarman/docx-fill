//! Renumbers ids that repeated content copied, which Word expects unique within a part.

use std::collections::{BTreeMap, BTreeSet};

use crate::properties::set_attribute;
use crate::xml::{ElementKind, Token, read_attribute, tokenize};

const DRAWING: &str = "wp:docPr";
const BOOKMARK_START: &str = "w:bookmarkStart";
const BOOKMARK_END: &str = "w:bookmarkEnd";
const PARAGRAPH_ID: &str = "w14:paraId";
const REVISIONS: &[&str] = &[
    "w:ins", "w:del", "w:moveFrom", "w:moveTo", "w:rPrChange", "w:pPrChange", "w:sectPrChange",
    "w:tblPrChange", "w:tblPrExChange", "w:tblGridChange", "w:trPrChange", "w:tcPrChange",
    "w:numberingChange", "w:cellIns", "w:cellDel", "w:cellMerge",
];
/// `w14:paraId` values must stay below this.
const PARAGRAPH_ID_LIMIT: u32 = 0x8000_0000;

/// `xml` with each repeated drawing, bookmark, paragraph and revision id given a new one.
/// A renamed bookmark's end follows its start, and bookmarks named with `kept_bookmarks` keep
/// their names. `None` when nothing repeats or the XML does not tokenize.
pub(crate) fn make_ids_unique(xml: &str, kept_bookmarks: Option<&str>) -> Option<String> {
    let tokens = tokenize(xml).ok()?;
    let mut ids = Ids::read(&tokens);
    ids.kept_bookmarks = kept_bookmarks.map(str::to_string);
    let mut out = String::with_capacity(xml.len());
    let mut is_changed = false;
    for token in &tokens {
        let (Token::Element { raw, .. } | Token::Text(raw)) = token;
        let edited = match token {
            Token::Element { kind: ElementKind::Open | ElementKind::SelfClosing, name, .. } => ids.renumber(name, raw),
            _ => None,
        };
        is_changed |= edited.is_some();
        out.push_str(edited.as_deref().unwrap_or(raw));
    }
    is_changed.then_some(out)
}

#[derive(Default)]
struct Ids {
    drawings: BTreeSet<u64>,
    next_drawing: u64,
    bookmarks: BTreeSet<u64>,
    next_bookmark: u64,
    /// Bookmarks open under a new id: the id as written, and the new one.
    renamed_bookmarks: BTreeMap<u64, u64>,
    names: BTreeSet<String>,
    kept_bookmarks: Option<String>,
    all_names: BTreeSet<String>,
    paragraphs: BTreeSet<u32>,
    next_paragraph: u32,
    revisions: BTreeSet<i64>,
    all_revisions: BTreeSet<i64>,
    next_revision: i64,
}

impl Ids {
    /// The largest of each id and every bookmark name, so new ones never collide.
    fn read(tokens: &[Token]) -> Ids {
        let mut ids = Ids::default();
        for token in tokens {
            let Token::Element { kind: ElementKind::Open | ElementKind::SelfClosing, name, raw } = token else { continue };
            match *name {
                DRAWING => ids.next_drawing = ids.next_drawing.max(read_number(raw, "id") + 1),
                BOOKMARK_START => {
                    ids.next_bookmark = ids.next_bookmark.max(read_number(raw, "w:id") + 1);
                    ids.all_names.extend(read_attribute(raw, "w:name").map(str::to_string));
                }
                _ => {}
            }
            if let Some(id) = read_paragraph_id(raw) {
                ids.next_paragraph = ids.next_paragraph.max(id + 1);
            }
            if REVISIONS.contains(name) && let Some(id) = read_revision_id(raw) {
                ids.all_revisions.insert(id);
            }
        }
        ids
    }

    /// The start tag rewritten with new ids, or `None` when its ids are the first of their kind.
    fn renumber(&mut self, name: &str, raw: &str) -> Option<String> {
        let mut edited = None::<String>;
        match name {
            DRAWING if !self.drawings.insert(read_number(raw, "id")) => {
                edited = Some(set_attribute(raw, "id", &self.next_drawing.to_string()));
                self.next_drawing += 1;
            }
            BOOKMARK_START => {
                let id = read_number(raw, "w:id");
                if !self.bookmarks.insert(id) {
                    self.renamed_bookmarks.insert(id, self.next_bookmark);
                    edited = Some(set_attribute(raw, "w:id", &self.next_bookmark.to_string()));
                    self.next_bookmark += 1;
                } else {
                    self.renamed_bookmarks.remove(&id);
                }
                let is_kept = |written: &&str| self.kept_bookmarks.as_deref().is_some_and(|prefix| written.starts_with(prefix));
                if let Some(written) = read_attribute(raw, "w:name").filter(|written| !is_kept(written) && !self.names.insert(written.to_string())) {
                    let name = self.new_name(written);
                    edited = Some(set_attribute(edited.as_deref().unwrap_or(raw), "w:name", &name));
                }
            }
            BOOKMARK_END => {
                let new = self.renamed_bookmarks.remove(&read_number(raw, "w:id"))?;
                edited = Some(set_attribute(raw, "w:id", &new.to_string()));
            }
            name if REVISIONS.contains(&name) => {
                if let Some(id) = read_revision_id(raw) && !self.revisions.insert(id) {
                    let new = self.new_revision_id()?;
                    edited = Some(set_attribute(raw, "w:id", &new.to_string()));
                }
            }
            _ => {}
        }
        if read_paragraph_id(raw).is_some_and(|id| !self.paragraphs.insert(id)) {
            let new = format!("{:08X}", self.next_paragraph % PARAGRAPH_ID_LIMIT);
            self.next_paragraph += 1;
            edited = Some(set_attribute(edited.as_deref().unwrap_or(raw), PARAGRAPH_ID, &new));
        }
        edited
    }

    fn new_revision_id(&mut self) -> Option<i64> {
        while !self.all_revisions.insert(self.next_revision) {
            self.next_revision = self.next_revision.checked_add(1)?;
        }
        Some(self.next_revision)
    }

    /// `name_2`, `name_3` and so on: the first no bookmark in the part has.
    fn new_name(&mut self, name: &str) -> String {
        let new = (2..).map(|count| format!("{name}_{count}")).find(|each| !self.all_names.contains(each)).expect("unbounded");
        self.all_names.insert(new.clone());
        self.names.insert(new.clone());
        new
    }
}

fn read_number(raw: &str, attribute: &str) -> u64 {
    read_attribute(raw, attribute).and_then(|value| value.parse().ok()).unwrap_or(0)
}

fn read_paragraph_id(raw: &str) -> Option<u32> {
    read_attribute(raw, PARAGRAPH_ID).and_then(|value| u32::from_str_radix(value, 16).ok())
}

fn read_revision_id(raw: &str) -> Option<i64> {
    read_attribute(raw, "w:id").and_then(|value| value.parse().ok())
}
