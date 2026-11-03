//! `{:include name}`: subtemplate bodies printed at their marks, with the styles, numbering,
//! relationships, media and footnotes they use. Built from the WordprocessingML and OPC specs.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::properties::{edit_each, find_element_end, find_starts};
use crate::render::FillHooks;
use crate::xml::{ElementKind, Token, read_attribute, tokenize};
use crate::{CONTENT_TYPES_PART, DOCUMENT_PART, FillError, FillOptions, PackageTemplate, STYLES_PART, add_relationship, footnotes, image_content_type, insert_before_last};

/// Marks an include in a filled part: the start, its index, then the end.
pub(crate) const INCLUDE_START: char = '\u{E005}';
pub(crate) const INCLUDE_END: char = '\u{E006}';

const NUMBERING_PART: &str = "word/numbering.xml";
const NUMBERING_RELATIONSHIP: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering";
const NUMBERING_CONTENT_TYPE: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml";
const MAIN_NAMESPACE: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
// Body elements pointing into parts an include does not bring.
const DROPPED: [&str; 5] = ["w:sectPr", "w:commentRangeStart", "w:commentRangeEnd", "w:commentReference", "w:endnoteReference"];
const STYLE_REFERENCES: [&str; 3] = ["w:pStyle", "w:rStyle", "w:tblStyle"];
const STYLE_LINKS: [&str; 3] = ["w:basedOn", "w:link", "w:next"];

/// Whether a subtemplate is filled before it is printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncludeMode {
    /// Printed as given.
    AsIs,
    /// Filled with the scope at its include tag: the tag's loop items over the root data.
    Templated,
}

/// Which definition a subtemplate's style takes when the main document has one with its id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncludeStyles {
    /// The main document's.
    UseMain,
    /// The subtemplate's own, renamed when it differs.
    KeepOwn,
}

/// A DOCX package `{:include}` tags print. Its body comes without section properties, so the
/// main document's headers, footers and page setup stay; comments and endnotes are dropped.
#[derive(Debug)]
pub struct Subtemplate {
    parts: Vec<(String, Vec<u8>)>,
    template: Option<PackageTemplate>,
    styles: IncludeStyles,
}

impl Subtemplate {
    /// Reads a subtemplate from its parts, named without a leading `/`.
    ///
    /// @returns An error when there is no `word/document.xml`, or, templated, when it does not
    ///   parse.
    pub fn parse(parts: impl IntoIterator<Item = (String, Vec<u8>)>, options: &FillOptions, mode: IncludeMode, styles: IncludeStyles) -> Result<Subtemplate, FillError> {
        let parts: Vec<(String, Vec<u8>)> = parts.into_iter().collect();
        if !parts.iter().any(|(name, _)| name == DOCUMENT_PART) {
            return Err(FillError::new("The subtemplate is not valid"));
        }
        let template = match mode {
            IncludeMode::Templated => Some(PackageTemplate::parse(parts.clone(), options)?),
            IncludeMode::AsIs => None,
        };
        Ok(Subtemplate { parts, template, styles })
    }

    fn render(&self, scope: &Value, hooks: &mut dyn FillHooks) -> (crate::PackageParts<'_>, Vec<FillError>) {
        match &self.template {
            Some(template) => template.render_result(scope, hooks),
            None => (self.parts.iter().map(|(name, bytes)| (name.clone(), Cow::Borrowed(bytes.as_slice()))).collect(), Vec::new()),
        }
    }
}

/// Prints each part's subtemplates at its include marks. A name with no subtemplate prints
/// nothing.
///
/// @returns How many includes printed, to number later imports after them.
pub(crate) fn print_includes(parts: &mut Vec<(String, Cow<[u8]>)>, uses: &[(String, Vec<(String, Value)>)], includes: &BTreeMap<String, Subtemplate>, hooks: &mut dyn FillHooks, errors: &mut Vec<FillError>) -> usize {
    let mut count = 0;
    for (part, used) in uses {
        let Some(index) = parts.iter().position(|(name, _)| name == part) else { continue };
        let mut next_drawing_id = crate::images::read_max_drawing_id(&String::from_utf8_lossy(&parts[index].1)).wrapping_add(1);
        let mut bodies = Vec::with_capacity(used.len());
        for (name, scope) in used {
            let Some(include) = includes.get(name) else {
                bodies.push(String::new());
                continue;
            };
            count += 1;
            let (rendered, found) = include.render(scope, hooks);
            errors.extend(found);
            let import = Import { parts: &mut *parts, part, from: &rendered, count, name, styles: include.styles, copied: BTreeMap::new() };
            bodies.push(import.run(&mut next_drawing_id));
        }
        let Some(index) = parts.iter().position(|(name, _)| name == part) else { continue };
        let xml = String::from_utf8_lossy(&parts[index].1).into_owned();
        parts[index].1 = Cow::Owned(replace_marks(&xml, &bodies).into_bytes());
    }
    count
}

/// The header and footer references of a subtemplate's last section, with the parts they name
/// copied into the package under the body's new relationships, and whether the section has a
/// first-page header. The styles and lists they use are imported as for a body include.
pub(crate) fn import_section_headers(parts: &mut Vec<(String, Cow<[u8]>)>, include: &Subtemplate, name: &str, scope: &Value, hooks: &mut dyn FillHooks, count: usize, errors: &mut Vec<FillError>) -> (String, bool) {
    let (rendered, found) = include.render(scope, hooks);
    errors.extend(found);
    let mut import = Import { parts, part: DOCUMENT_PART, from: &rendered, count, name, styles: include.styles, copied: BTreeMap::new() };
    let document = import.read_from(DOCUMENT_PART);
    let Some(section) = find_starts(&document, "w:sectPr").last().and_then(|&start| find_element_end(&document, start, "w:sectPr").map(|end| &document[start..end])) else {
        return (String::new(), false);
    };
    let is_title_page = find_starts(section, "w:titlePg").first().is_some_and(|&at| !section[at..].starts_with("<w:titlePg w:val=\"0\"") && !section[at..].starts_with("<w:titlePg w:val=\"false\""));

    let mut references = String::new();
    for element in ["w:headerReference", "w:footerReference"] {
        for start in find_starts(section, element) {
            let Some(end) = find_element_end(section, start, element) else { continue };
            references.push_str(&section[start..end]);
        }
    }
    let relationships = import.import_relationships(&references, DOCUMENT_PART);
    let references = edit_attributes(&references, |_, attribute, value| match relationships.get(value) {
        Some(renamed) if attribute == "r:id" => Edit::Set(renamed.clone()),
        _ => Edit::Keep,
    });

    let copied: Vec<String> = import.copied.values().filter(|name| name.ends_with(".xml")).cloned().collect();
    let headers: String = copied.iter().map(|name| import.read_main(name).unwrap_or_default()).collect();
    let definitions = read_styles(&import.read_from(STYLES_PART));
    let used = read_style_closure(&headers, &definitions);
    let numbered: String = used.iter().filter_map(|id| definitions.get(id).map(String::as_str)).collect();
    let numbers = import.import_numbering(&format!("{headers}{numbered}"));
    let styles = import.import_styles(&used, &definitions, &numbers);
    for part in copied {
        let Some(xml) = import.read_main(&part) else { continue };
        let renamed = edit_attributes(&xml, |element, attribute, value| rename_reference(element, attribute, value, &BTreeMap::new(), &styles, &numbers));
        import.write_main(&part, renamed);
    }
    (references, is_title_page)
}

fn replace_marks(xml: &str, bodies: &[String]) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut chars = xml.chars();
    while let Some(c) = chars.next() {
        if c != INCLUDE_START {
            out.push(c);
            continue;
        }
        let index: String = chars.by_ref().take_while(|c| *c != INCLUDE_END).collect();
        if let Some(body) = index.parse::<usize>().ok().and_then(|index| bodies.get(index)) {
            out.push_str(body);
        }
    }
    out
}

/// One include of a subtemplate into a part of the main package.
struct Import<'a, 'b> {
    parts: &'a mut Vec<(String, Cow<'b, [u8]>)>,
    part: &'a str,
    from: &'a [(String, Cow<'a, [u8]>)],
    /// Numbers this include, so its parts and relationships get names of their own.
    count: usize,
    name: &'a str,
    styles: IncludeStyles,
    /// The new name of each part copied so far.
    copied: BTreeMap<String, String>,
}

/// What an attribute edit does.
enum Edit {
    Keep,
    Set(String),
    Remove,
}

impl Import<'_, '_> {
    /// The subtemplate's body, its references renamed to what it brought into the package.
    fn run(mut self, next_drawing_id: &mut u32) -> String {
        let document = self.read_from(DOCUMENT_PART);
        let Some(body) = read_body(&document) else { return String::new() };
        let mut body = body.to_string();
        for name in DROPPED {
            body = edit_each(&body, name, |_| String::new());
        }
        self.merge_namespaces(self.part, &document);

        let relationships = self.import_relationships(&body, DOCUMENT_PART);
        let (notes, note_ids) = self.read_notes(&body);
        let definitions = read_styles(&self.read_from(STYLES_PART));
        let used_styles = read_style_closure(&format!("{body}{notes}"), &definitions);
        let numbered: String = used_styles.iter().filter_map(|id| definitions.get(id).map(String::as_str)).collect();
        let numbers = self.import_numbering(&format!("{body}{notes}{numbered}"));
        let styles = self.import_styles(&used_styles, &definitions, &numbers);

        let notes = edit_attributes(&notes, |element, attribute, value| rename_reference(element, attribute, value, &BTreeMap::new(), &styles, &numbers));
        self.add_notes(&notes);
        edit_attributes(&body, |element, attribute, value| {
            if element == "wp:docPr" && attribute == "id" {
                let id = *next_drawing_id;
                *next_drawing_id = id.wrapping_add(1);
                return Edit::Set(id.to_string());
            }
            if element == "w:footnoteReference" && attribute == "w:id" {
                return note_ids.get(value).map_or(Edit::Keep, |id| Edit::Set(id.clone()));
            }
            rename_reference(element, attribute, value, &relationships, &styles, &numbers)
        })
    }

    fn read_from(&self, name: &str) -> String {
        self.from.iter().find(|(each, _)| each == name).map(|(_, bytes)| String::from_utf8_lossy(bytes).into_owned()).unwrap_or_default()
    }

    fn read_main(&self, name: &str) -> Option<String> {
        self.parts.iter().find(|(each, _)| each == name).map(|(_, bytes)| String::from_utf8_lossy(bytes).into_owned())
    }

    fn write_main(&mut self, name: &str, xml: String) {
        match self.parts.iter_mut().find(|(each, _)| each == name) {
            Some(part) => part.1 = Cow::Owned(xml.into_bytes()),
            None => self.parts.push((name.to_string(), Cow::Owned(xml.into_bytes()))),
        }
    }

    /// Declares on the main part's root the namespaces `from`'s root declares and it lacks, and
    /// adds `from`'s ignorable prefixes.
    fn merge_namespaces(&mut self, part: &str, from: &str) {
        let (Some(main), Some(from_root)) = (self.read_main(part), read_root(from)) else { return };
        let Some(root) = read_root(&main) else { return };
        let mut edited = root.to_string();
        let mut ignorable: Vec<String> = read_attribute(root, "mc:Ignorable").unwrap_or_default().split_whitespace().map(str::to_string).collect();
        for (name, value) in read_attributes(from_root) {
            if name.starts_with("xmlns:") && read_attribute(root, &name).is_none() {
                edited = crate::properties::set_attribute(&edited, &name, &value);
            }
            if name == "mc:Ignorable" {
                ignorable.extend(value.split_whitespace().filter(|prefix| !ignorable.iter().any(|each| each == prefix)).map(str::to_string).collect::<Vec<_>>());
            }
        }
        if !ignorable.is_empty() && edited.contains("xmlns:mc=") {
            edited = crate::properties::set_attribute(&edited, "mc:Ignorable", &ignorable.join(" "));
        }
        if edited != root {
            let at = main.find(root).expect("read from main");
            self.write_main(part, format!("{}{edited}{}", &main[..at], &main[at + root.len()..]));
        }
    }

    /// Copies the relationships of `from_part` that `xml` uses, and their parts, giving each a
    /// new id on the including part.
    fn import_relationships(&mut self, xml: &str, from_part: &str) -> BTreeMap<String, String> {
        let used = read_values(xml, |_, attribute| attribute.starts_with("r:"));
        let relationships = self.read_from(&relationships_of(from_part));
        let mut renamed = BTreeMap::new();
        let Ok(tokens) = tokenize(&relationships) else { return renamed };
        for token in tokens {
            let Token::Element { kind: ElementKind::Open | ElementKind::SelfClosing, name: "Relationship", raw } = token else { continue };
            let (Some(id), Some(kind), Some(target)) = (read_attribute(raw, "Id"), read_attribute(raw, "Type"), read_attribute(raw, "Target")) else { continue };
            if !used.contains(id) {
                continue;
            }
            let safe: String = id.chars().filter(char::is_ascii_alphanumeric).collect();
            let new_id = format!("rIdDocxFillInclude{}_{safe}", self.count);
            let is_external = read_attribute(raw, "TargetMode") == Some("External");
            let new_target = if is_external { target.to_string() } else {
                let Some(copied) = self.copy_part(&resolve(from_part, target)) else { continue };
                relative_to(self.part, &copied)
            };
            let mode = if is_external { " TargetMode=\"External\"" } else { "" };
            let relationship = format!("<Relationship Id=\"{new_id}\" Type=\"{kind}\" Target=\"{new_target}\"{mode}/>");
            add_relationship(self.parts, self.part, &new_id, &relationship);
            renamed.insert(id.to_string(), new_id);
        }
        renamed
    }

    /// Copies part `source` under a new name, with its content type and the parts its own
    /// relationships target. The new name.
    fn copy_part(&mut self, source: &str) -> Option<String> {
        if let Some(copied) = self.copied.get(source) {
            return Some(copied.clone());
        }
        let bytes = self.from.iter().find(|(name, _)| *name == source)?.1.to_vec();
        let (folder, file) = source.rsplit_once('/').unwrap_or(("", source));
        let copied = format!("{folder}/docx_fill_include{}_{file}", self.count);
        self.copied.insert(source.to_string(), copied.clone());

        let types = self.read_from(CONTENT_TYPES_PART);
        let override_type = read_content_type(&types, "Override", "PartName", &format!("/{source}"));
        if let Some(mut main_types) = self.read_main(CONTENT_TYPES_PART) {
            let extension = file.rsplit_once('.').map_or("", |(_, extension)| extension).to_ascii_lowercase();
            let added = match override_type {
                Some(content_type) => Some(format!("<Override PartName=\"/{copied}\" ContentType=\"{content_type}\"/>")),
                None if !main_types.to_ascii_lowercase().contains(&format!("extension=\"{extension}\"")) => {
                    let content_type = read_content_type(&types, "Default", "Extension", &extension).unwrap_or_else(|| image_content_type(&extension));
                    Some(format!("<Default Extension=\"{extension}\" ContentType=\"{content_type}\"/>"))
                }
                None => None,
            };
            if let Some(added) = added {
                main_types = insert_before_last(&main_types, "</Types>", &added);
                self.write_main(CONTENT_TYPES_PART, main_types);
            }
        }
        self.parts.push((copied.clone(), Cow::Owned(bytes)));
        self.copy_relationships(source, &copied);
        Some(copied)
    }

    /// Writes the relationships of part `source` for its copy, copying the parts they target.
    fn copy_relationships(&mut self, source: &str, copied: &str) {
        let relationships = self.read_from(&relationships_of(source));
        let Ok(tokens) = tokenize(&relationships) else { return };
        let tokens: Vec<(bool, String)> = tokens
            .iter()
            .map(|token| match *token {
                Token::Element { kind: ElementKind::Open | ElementKind::SelfClosing, name: "Relationship", raw } => (true, raw.to_string()),
                Token::Element { raw, .. } => (false, raw.to_string()),
                Token::Text(text) => (false, text.to_string()),
            })
            .collect();
        if !tokens.iter().any(|(is_relationship, _)| *is_relationship) {
            return;
        }
        let mut written = String::new();
        for (is_relationship, raw) in tokens {
            let target = read_attribute(&raw, "Target").filter(|_| is_relationship && read_attribute(&raw, "TargetMode") != Some("External"));
            let Some(target) = target else {
                written.push_str(&raw);
                continue;
            };
            let resolved = resolve(source, target);
            match self.copy_part(&resolved) {
                Some(new_part) => written.push_str(&crate::properties::set_attribute(&raw, "Target", &relative_to(copied, &new_part))),
                None => written.push_str(&raw),
            }
        }
        self.parts.push((relationships_of(copied), Cow::Owned(written.into_bytes())));
    }

    /// The footnotes `body` references, with new ids, and the new id for each old one.
    fn read_notes(&self, body: &str) -> (String, BTreeMap<String, String>) {
        let used = read_values(body, |element, attribute| element == "w:footnoteReference" && attribute == "w:id");
        let mut ids = BTreeMap::new();
        if used.is_empty() {
            return (String::new(), ids);
        }
        let from = self.read_from(footnotes::FOOTNOTES_PART);
        let mut next = self.read_main(footnotes::FOOTNOTES_PART).map_or(0, |xml| footnotes::read_max_id(&xml)) + 1;
        let mut notes = String::new();
        for start in find_starts(&from, "w:footnote") {
            let Some(end) = find_element_end(&from, start, "w:footnote") else { continue };
            let note = &from[start..end];
            let Some(id) = read_attribute(&note[..note.find('>').unwrap_or(note.len())], "w:id") else { continue };
            if !used.contains(id) {
                continue;
            }
            ids.insert(id.to_string(), next.to_string());
            notes.push_str(&edit_attributes(note, |element, attribute, _| match (element, attribute) {
                ("w:footnote", "w:id") => Edit::Set(next.to_string()),
                _ => Edit::Keep,
            }));
            next += 1;
        }
        (notes, ids)
    }

    fn add_notes(&mut self, notes: &str) {
        if notes.is_empty() {
            return;
        }
        let from = self.read_from(footnotes::FOOTNOTES_PART);
        footnotes::append_notes(self.parts, notes);
        self.merge_namespaces(footnotes::FOOTNOTES_PART, &from);
    }

    /// Copies the `used` styles the main document lacks, or with `KeepOwn` has otherwise. The new
    /// id of each, which differs only for a renamed style.
    fn import_styles(&mut self, used: &BTreeSet<String>, definitions: &BTreeMap<String, String>, numbers: &BTreeMap<String, String>) -> BTreeMap<String, String> {
        let mut renamed = BTreeMap::new();
        let Some(main) = self.read_main(STYLES_PART) else { return renamed };
        let existing = read_styles(&main);

        let suffix: String = self.name.chars().filter(char::is_ascii_alphanumeric).collect();
        let mut copied = Vec::new();
        for id in used.iter().filter(|id| definitions.contains_key(*id)) {
            let new_id = match existing.get(id) {
                None => id.clone(),
                Some(_) if self.styles == IncludeStyles::UseMain => continue,
                Some(main) if *main == definitions[id] => continue,
                Some(_) => format!("{id}{suffix}"),
            };
            renamed.insert(id.clone(), new_id.clone());
            copied.push(id);
        }
        let mut added = String::new();
        for id in copied {
            let new_id = &renamed[id];
            if existing.contains_key(new_id) {
                continue;
            }
            let is_renamed = new_id != id;
            let definition = edit_attributes(&definitions[id], |element, attribute, value| match (element, attribute) {
                ("w:style", "w:styleId") => Edit::Set(new_id.clone()),
                ("w:style", "w:default") => Edit::Remove,
                ("w:name", "w:val") if is_renamed => Edit::Set(format!("{value} ({})", self.name)),
                _ => rename_reference(element, attribute, value, &BTreeMap::new(), &renamed, numbers),
            });
            added.push_str(&definition);
        }
        if !added.is_empty() {
            self.write_main(STYLES_PART, insert_before_last(&main, "</w:styles>", &added));
            let from = self.read_from(STYLES_PART);
            self.merge_namespaces(STYLES_PART, &from);
        }
        renamed
    }

    /// Copies the lists `xml` numbers by, as new lists. The new id of each.
    fn import_numbering(&mut self, xml: &str) -> BTreeMap<String, String> {
        let mut renamed = BTreeMap::new();
        let mut used = read_values(xml, |element, attribute| element == "w:numId" && attribute == "w:val");
        used.remove("0");
        if used.is_empty() {
            return renamed;
        }

        let from = self.read_from(NUMBERING_PART);
        let main = self.read_main(NUMBERING_PART);
        let main_xml = main.as_deref().unwrap_or("");
        let max = |element: &str, attribute: &str| read_values(main_xml, |each, name| each == element && name == attribute).iter().filter_map(|id| id.parse::<u32>().ok()).max().unwrap_or(0);
        let (mut next_number, mut next_abstract) = (max("w:num", "w:numId") + 1, max("w:abstractNum", "w:abstractNumId") + 1);

        let abstracts = read_keyed(&from, "w:abstractNum", "w:abstractNumId");
        let main_abstracts = read_keyed(main_xml, "w:abstractNum", "w:abstractNumId");
        let main_numbers = read_keyed(main_xml, "w:num", "w:numId");
        let mut abstract_ids = BTreeMap::new();
        let (mut added_abstracts, mut added_numbers) = (String::new(), String::new());
        for (id, number) in read_keyed(&from, "w:num", "w:numId") {
            if !used.contains(&id) {
                continue;
            }
            let Some(abstract_id) = read_values(&number, |element, attribute| element == "w:abstractNumId" && attribute == "w:val").into_iter().next() else { continue };
            let Some(definition) = abstracts.get(&abstract_id) else { continue };
            // A list the main document has, level by level, continues there.
            if let Some(reused) = find_same_list(definition, &number, &main_abstracts, &main_numbers) {
                renamed.insert(id, reused);
                continue;
            }
            let new_abstract = abstract_ids.entry(abstract_id).or_insert_with(|| {
                let id = next_abstract.to_string();
                next_abstract += 1;
                let definition = edit_each(definition, "w:nsid", |_| String::new());
                added_abstracts.push_str(&edit_attributes(&definition, |element, attribute, _| match (element, attribute) {
                    ("w:abstractNum", "w:abstractNumId") => Edit::Set(id.clone()),
                    _ => Edit::Keep,
                }));
                id
            });
            let new_number = next_number.to_string();
            next_number += 1;
            added_numbers.push_str(&edit_attributes(&number, |element, attribute, _| match (element, attribute) {
                ("w:num", "w:numId") => Edit::Set(new_number.clone()),
                ("w:abstractNumId", "w:val") => Edit::Set(new_abstract.clone()),
                _ => Edit::Keep,
            }));
            renamed.insert(id, new_number);
        }
        if added_numbers.is_empty() {
            return renamed;
        }

        let Some(main) = main else {
            let xml = format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:numbering xmlns:w=\"{MAIN_NAMESPACE}\">{added_abstracts}{added_numbers}</w:numbering>");
            self.write_main(NUMBERING_PART, xml);
            self.merge_namespaces(NUMBERING_PART, &from);
            let relationship = format!("<Relationship Id=\"rIdDocxFillNumbering\" Type=\"{NUMBERING_RELATIONSHIP}\" Target=\"numbering.xml\"/>");
            add_relationship(self.parts, DOCUMENT_PART, "rIdDocxFillNumbering", &relationship);
            if let Some(types) = self.read_main(CONTENT_TYPES_PART).filter(|types| !types.contains("PartName=\"/word/numbering.xml\"")) {
                let added = format!("<Override PartName=\"/word/numbering.xml\" ContentType=\"{NUMBERING_CONTENT_TYPE}\"/>");
                self.write_main(CONTENT_TYPES_PART, insert_before_last(&types, "</Types>", &added));
            }
            return renamed;
        };
        // Lists come after every abstract list, and before the cleanup list ids.
        let abstracts_at = find_starts(&main, "w:num").first().copied().unwrap_or_else(|| main.rfind("</w:numbering>").unwrap_or(main.len()));
        let mut xml = format!("{}{added_abstracts}{}", &main[..abstracts_at], &main[abstracts_at..]);
        let numbers_at = find_starts(&xml, "w:numIdMacAtCleanup").first().copied().unwrap_or_else(|| xml.rfind("</w:numbering>").unwrap_or(xml.len()));
        xml = format!("{}{added_numbers}{}", &xml[..numbers_at], &xml[numbers_at..]);
        self.write_main(NUMBERING_PART, xml);
        self.merge_namespaces(NUMBERING_PART, &from);
        renamed
    }
}

/// The main document's list with the same number format, text and tab at every level as
/// `definition`, when `number` overrides none of them.
fn find_same_list(definition: &str, number: &str, main_abstracts: &BTreeMap<String, String>, main_numbers: &BTreeMap<String, String>) -> Option<String> {
    let levels = read_levels(definition);
    if levels.is_empty() || number.contains("<w:lvlOverride") {
        return None;
    }
    let (abstract_id, _) = main_abstracts.iter().find(|(_, main)| read_levels(main) == levels)?;
    main_numbers
        .iter()
        .find(|(_, main)| !main.contains("<w:lvlOverride") && read_values(main, |element, attribute| element == "w:abstractNumId" && attribute == "w:val").contains(abstract_id))
        .map(|(id, _)| id.clone())
}

/// Each level's number format, text and tab position.
fn read_levels(definition: &str) -> Vec<[Option<String>; 3]> {
    let read = |level: &str, element: &str, attribute: &str| read_values(level, |each, name| each == element && name == attribute).into_iter().next();
    find_starts(definition, "w:lvl")
        .into_iter()
        .filter_map(|start| find_element_end(definition, start, "w:lvl").map(|end| &definition[start..end]))
        .map(|level| [read(level, "w:numFmt", "w:val"), read(level, "w:lvlText", "w:val"), read(level, "w:tab", "w:pos")])
        .collect()
}

/// The styles `xml` uses, and those they are based on, linked to or followed by.
fn read_style_closure(xml: &str, definitions: &BTreeMap<String, String>) -> BTreeSet<String> {
    let mut wanted: Vec<String> = read_values(xml, |element, attribute| STYLE_REFERENCES.contains(&element) && attribute == "w:val").into_iter().collect();
    let mut seen: BTreeSet<String> = wanted.iter().cloned().collect();
    while let Some(id) = wanted.pop() {
        let Some(definition) = definitions.get(&id) else { continue };
        for linked in read_values(definition, |element, attribute| STYLE_LINKS.contains(&element) && attribute == "w:val") {
            if seen.insert(linked.clone()) {
                wanted.push(linked);
            }
        }
    }
    seen
}

/// The new value of a relationship, style or list reference, if it was renamed.
fn rename_reference(element: &str, attribute: &str, value: &str, relationships: &BTreeMap<String, String>, styles: &BTreeMap<String, String>, numbers: &BTreeMap<String, String>) -> Edit {
    let renamed = if attribute.starts_with("r:") {
        relationships.get(value)
    } else if attribute != "w:val" {
        None
    } else if STYLE_REFERENCES.contains(&element) || STYLE_LINKS.contains(&element) {
        styles.get(value)
    } else if element == "w:numId" {
        numbers.get(value)
    } else {
        None
    };
    renamed.map_or(Edit::Keep, |renamed| Edit::Set(renamed.clone()))
}

/// The content of `<w:body>`.
fn read_body(document: &str) -> Option<&str> {
    let start = find_starts(document, "w:body").first().copied()?;
    let open_end = start + document[start..].find('>')? + 1;
    let close = document.rfind("</w:body>")?;
    (close >= open_end).then(|| &document[open_end..close])
}

/// The start tag of the document element.
fn read_root(xml: &str) -> Option<&str> {
    tokenize(xml).ok()?.into_iter().find_map(|token| match token {
        Token::Element { kind: ElementKind::Open | ElementKind::SelfClosing, raw, .. } => Some(raw),
        _ => None,
    })
}

/// The relationships part of `part`.
pub(crate) fn relationships_of(part: &str) -> String {
    let (folder, file) = part.rsplit_once('/').map_or(("", part), |(folder, file)| (folder, file));
    if folder.is_empty() { format!("_rels/{file}.rels") } else { format!("{folder}/_rels/{file}.rels") }
}

/// `path` as a relationship target from `part`: relative within its folder, else absolute.
pub(crate) fn relative_to(part: &str, path: &str) -> String {
    let folder = part.rfind('/').map_or("", |at| &part[..at + 1]);
    match path.strip_prefix(folder) {
        Some(relative) if !folder.is_empty() => relative.to_string(),
        _ => format!("/{path}"),
    }
}

/// The part a relationship of `part` targets.
pub(crate) fn resolve(part: &str, target: &str) -> String {
    if let Some(absolute) = target.strip_prefix('/') {
        return absolute.to_string();
    }
    let mut path: Vec<&str> = part.split('/').collect();
    path.pop();
    for segment in target.split('/') {
        match segment {
            ".." => {
                path.pop();
            }
            "." | "" => {}
            other => path.push(other),
        }
    }
    path.join("/")
}

fn read_content_type(types: &str, element: &str, key: &str, value: &str) -> Option<String> {
    tokenize(types).ok()?.into_iter().find_map(|token| match token {
        Token::Element { name, raw, .. } if name == element && read_attribute(raw, key).is_some_and(|each| each.eq_ignore_ascii_case(value)) => {
            read_attribute(raw, "ContentType").map(str::to_string)
        }
        _ => None,
    })
}

/// Each `<w:style>` definition by its id.
fn read_styles(xml: &str) -> BTreeMap<String, String> {
    read_keyed(xml, "w:style", "w:styleId")
}

/// Each `name` element by its `key` attribute.
fn read_keyed(xml: &str, name: &str, key: &str) -> BTreeMap<String, String> {
    let mut keyed = BTreeMap::new();
    for start in find_starts(xml, name) {
        let Some(end) = find_element_end(xml, start, name) else { continue };
        let element = &xml[start..end];
        if let Some(id) = read_attribute(&element[..element.find('>').unwrap_or(element.len())], key) {
            keyed.entry(id.to_string()).or_insert_with(|| element.to_string());
        }
    }
    keyed
}

/// The values of the attributes `is_wanted` picks by element and attribute name.
fn read_values(xml: &str, is_wanted: impl Fn(&str, &str) -> bool) -> BTreeSet<String> {
    let mut values = BTreeSet::new();
    edit_attributes(xml, |element, attribute, value| {
        if is_wanted(element, attribute) {
            values.insert(value.to_string());
        }
        Edit::Keep
    });
    values
}

/// `xml` with each start tag's attributes edited by `edit`, given the element name, attribute
/// name and raw value.
fn edit_attributes(xml: &str, mut edit: impl FnMut(&str, &str, &str) -> Edit) -> String {
    let Ok(tokens) = tokenize(xml) else { return xml.to_string() };
    let mut out = String::with_capacity(xml.len());
    for token in tokens {
        match token {
            Token::Element { kind: ElementKind::Open | ElementKind::SelfClosing, name, raw } => {
                let mut written = format!("<{name}");
                let mut is_edited = false;
                for (attribute, value) in read_attributes(raw) {
                    match edit(name, &attribute, &value) {
                        Edit::Keep => written.push_str(&format!(" {attribute}=\"{value}\"")),
                        Edit::Set(value) => {
                            is_edited = true;
                            written.push_str(&format!(" {attribute}=\"{value}\""));
                        }
                        Edit::Remove => is_edited = true,
                    }
                }
                written.push_str(if raw.ends_with("/>") { "/>" } else { ">" });
                out.push_str(if is_edited { &written } else { raw });
            }
            Token::Element { raw, .. } => out.push_str(raw),
            Token::Text(text) => out.push_str(text),
        }
    }
    out
}

/// A start tag's attributes, with values as written (escaped). Single-quoted values come back
/// as written too, to be written double-quoted, so they must not hold `"`.
fn read_attributes(raw: &str) -> Vec<(String, String)> {
    let mut attributes = Vec::new();
    let inner = raw.trim_start_matches('<').trim_end_matches('>').trim_end_matches('/');
    let Some(mut rest) = inner.find(char::is_whitespace).map(|at| &inner[at..]) else { return attributes };
    loop {
        rest = rest.trim_start();
        let Some(equals) = rest.find('=') else { break };
        let name = rest[..equals].trim().to_string();
        let after = rest[equals + 1..].trim_start();
        let Some(quote) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else { break };
        let Some(length) = after[1..].find(quote) else { break };
        let value = after[1..1 + length].to_string();
        attributes.push((name, if quote == '\'' { value.replace('"', "&quot;") } else { value }));
        rest = &after[length + 2..];
    }
    attributes
}
