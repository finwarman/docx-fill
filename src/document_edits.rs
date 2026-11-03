//! Whole-document edits: edit protection, page margins, the proofing language and a text
//! watermark. Built from the WordprocessingML and VML specs.

use std::borrow::Cow;

use crate::{CONTENT_TYPES_PART, DOCUMENT_PART, FillError, add_relationship, insert_before_last, is_template_part};
use crate::cell_styling::find_element_start;
use crate::properties::{RUN_ORDER, SETTINGS_FROM_PROTECTION, edit_each, edit_properties, find_starts, set_attribute, set_child};
use crate::protection::Protection;
use crate::xml::escape;

const SETTINGS_PART: &str = "word/settings.xml";
const STYLES_PART: &str = "word/styles.xml";
const TWIPS_PER_INCH: f64 = 1440.0;
const WATERMARK_HEADER: &str = "word/header_docx_fill_watermark.xml";
const WATERMARK_RELATIONSHIP_ID: &str = "rIdDocxFillWatermark";
const HEADER_RELATIONSHIP: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/header";
const HEADER_CONTENT_TYPE: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml";
const MAIN_NAMESPACE: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const RELATIONSHIPS_NAMESPACE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
// The namespaces a VML watermark needs on its header's root.
const WATERMARK_NAMESPACES: [(&str, &str); 3] =
    [("xmlns:v", "urn:schemas-microsoft-com:vml"), ("xmlns:o", "urn:schemas-microsoft-com:office:office"), ("xmlns:w10", "urn:schemas-microsoft-com:office:word")];
// The body width a watermark spans, in points.
const WATERMARK_WIDTH: f64 = 468.0;

/// Edits a package fill makes to the whole document. None by default.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DocumentEdits {
    /// Opens the document read-only in Word, as "Restrict Editing" does, without a password.
    pub read_only: bool,
    /// Restricts editing behind a password; replaces `read_only`.
    pub protection: Option<Protection>,
    /// Text drawn faintly behind every page, in every header.
    pub watermark: Option<Watermark>,
    /// Every section's page margins.
    pub margins: Option<Margins>,
    /// The default proofing language, such as `en-GB`.
    pub language: Option<String>,
}

/// Page margins in inches.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Margins {
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}

/// A text watermark, grey and half transparent, centred on the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watermark {
    pub text: String,
    pub layout: WatermarkLayout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatermarkLayout {
    Diagonal,
    Horizontal,
}

impl DocumentEdits {
    pub(crate) fn check(&self) -> Result<(), FillError> {
        self.protection.as_ref().map_or(Ok(()), Protection::check)
    }

    pub(crate) fn apply(&self, parts: &mut Vec<(String, Cow<[u8]>)>) {
        if let Some(protection) = &self.protection {
            let element = protection.element();
            edit_part(parts, SETTINGS_PART, |xml| protect(xml, &element));
        } else if self.read_only {
            edit_part(parts, SETTINGS_PART, |xml| protect(xml, "<w:documentProtection w:edit=\"readOnly\" w:enforcement=\"1\"/>"));
        }
        if let Some(watermark) = &self.watermark {
            add_watermark(parts, watermark);
        }
        if let Some(margins) = self.margins {
            edit_part(parts, DOCUMENT_PART, |xml| set_margins(xml, margins));
        }
        if let Some(language) = &self.language {
            edit_part(parts, STYLES_PART, |xml| set_language(xml, language));
        }
    }
}

fn edit_part(parts: &mut [(String, Cow<[u8]>)], name: &str, edit: impl FnOnce(&str) -> String) {
    let Some(part) = parts.iter_mut().find(|(each, _)| each == name) else { return };
    let xml = String::from_utf8_lossy(&part.1).into_owned();
    part.1 = Cow::Owned(edit(&xml).into_bytes());
}

/// `xml`'s root element content rewritten by `edit`.
fn edit_root(xml: &str, root: &str, edit: impl FnOnce(&str) -> String) -> String {
    let Some(start) = find_element_start(xml, &format!("<{root}")) else { return xml.to_string() };
    let Some(open_end) = xml[start..].find('>').map(|at| start + at + 1) else { return xml.to_string() };
    let Some(close) = xml.rfind(&format!("</{root}>")) else { return xml.to_string() };
    format!("{}{}{}", &xml[..open_end], edit(&xml[open_end..close]), &xml[close..])
}

fn protect(xml: &str, protection: &str) -> String {
    edit_root(xml, "w:settings", |inner| set_child(inner, "w:documentProtection", protection, SETTINGS_FROM_PROTECTION))
}

/// Draws the watermark in every header, adding a header to the first section when the document
/// has none.
fn add_watermark(parts: &mut Vec<(String, Cow<[u8]>)>, watermark: &Watermark) {
    let mut headers: Vec<String> = parts.iter().map(|(name, _)| name.clone()).filter(|name| is_template_part(name) && name.starts_with("word/header")).collect();
    if headers.is_empty() {
        let has_section = parts.iter().any(|(name, xml)| name == DOCUMENT_PART && String::from_utf8_lossy(xml).contains("<w:sectPr"));
        if !has_section {
            return;
        }
        let header = format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:hdr xmlns:w=\"{MAIN_NAMESPACE}\" xmlns:r=\"{RELATIONSHIPS_NAMESPACE}\"><w:p/></w:hdr>");
        parts.push((WATERMARK_HEADER.to_string(), Cow::Owned(header.into_bytes())));
        let relationship = format!("<Relationship Id=\"{WATERMARK_RELATIONSHIP_ID}\" Type=\"{HEADER_RELATIONSHIP}\" Target=\"header_docx_fill_watermark.xml\"/>");
        add_relationship(parts, DOCUMENT_PART, WATERMARK_RELATIONSHIP_ID, &relationship);
        edit_part(parts, CONTENT_TYPES_PART, |xml| {
            insert_before_last(xml, "</Types>", &format!("<Override PartName=\"/{WATERMARK_HEADER}\" ContentType=\"{HEADER_CONTENT_TYPE}\"/>"))
        });
        // Later sections without headers of their own take the first's.
        edit_part(parts, DOCUMENT_PART, |xml| {
            let Some(start) = find_starts(xml, "w:sectPr").first().copied() else { return xml.to_string() };
            let reference = format!("<w:headerReference w:type=\"default\" r:id=\"{WATERMARK_RELATIONSHIP_ID}\"/>");
            let section = &xml[start..];
            let open_end = start + section.find('>').map_or(0, |at| at + 1);
            let xml = if xml[..open_end].ends_with("/>") {
                format!("{}<w:sectPr{}>{reference}</w:sectPr>{}", &xml[..start], &xml[start + "<w:sectPr".len()..open_end - 2], &xml[open_end..])
            } else {
                format!("{}{reference}{}", &xml[..open_end], &xml[open_end..])
            };
            add_namespace(&xml, "xmlns:r", RELATIONSHIPS_NAMESPACE)
        });
        headers.push(WATERMARK_HEADER.to_string());
    }
    for (index, header) in headers.iter().enumerate() {
        let paragraph = watermark_paragraph(watermark, index + 1);
        edit_part(parts, header, |xml| {
            let mut xml = xml.to_string();
            for (name, value) in WATERMARK_NAMESPACES {
                xml = add_namespace(&xml, name, value);
            }
            edit_root(&xml, "w:hdr", |inner| format!("{paragraph}{inner}"))
        });
    }
}

/// `xml` with namespace `name` declared on its root when missing.
fn add_namespace(xml: &str, name: &str, value: &str) -> String {
    let Some(start) = xml.find("<w:") else { return xml.to_string() };
    let Some(open_end) = xml[start..].find('>').map(|at| start + at + 1) else { return xml.to_string() };
    let root = &xml[start..open_end];
    if root.contains(&format!(" {name}=")) {
        return xml.to_string();
    }
    format!("{}{}{}", &xml[..start], set_attribute(root, name, value), &xml[open_end..])
}

/// A paragraph holding the watermark as a VML text path, as Word draws one. `index` numbers its
/// shape.
fn watermark_paragraph(watermark: &Watermark, index: usize) -> String {
    let mut text = String::new();
    escape(&watermark.text, &mut text);
    let characters = watermark.text.chars().count().max(1) as f64;
    let height = (WATERMARK_WIDTH / characters * 1.6).min(WATERMARK_WIDTH / 2.0);
    let rotation = match watermark.layout {
        WatermarkLayout::Diagonal => "rotation:315;",
        WatermarkLayout::Horizontal => "",
    };
    format!(
        "<w:p><w:pPr><w:pStyle w:val=\"Header\"/></w:pPr><w:r><w:rPr><w:noProof/></w:rPr><w:pict>\
<v:shapetype id=\"_x0000_t136\" coordsize=\"21600,21600\" o:spt=\"136\" adj=\"10800\" path=\"m@7,l@8,m@5,21600l@6,21600e\">\
<v:formulas><v:f eqn=\"sum #0 0 10800\"/><v:f eqn=\"prod #0 2 1\"/><v:f eqn=\"sum 21600 0 @1\"/><v:f eqn=\"sum 0 0 @2\"/><v:f eqn=\"sum 21600 0 @3\"/>\
<v:f eqn=\"if @0 @3 0\"/><v:f eqn=\"if @0 21600 @1\"/><v:f eqn=\"if @0 0 @2\"/><v:f eqn=\"if @0 @4 21600\"/><v:f eqn=\"mid @5 @6\"/><v:f eqn=\"mid @8 @5\"/>\
<v:f eqn=\"mid @7 @8\"/><v:f eqn=\"mid @6 @7\"/><v:f eqn=\"sum @6 0 @5\"/></v:formulas>\
<v:path textpathok=\"t\" o:connecttype=\"custom\" o:connectlocs=\"@9,0;@10,10800;@11,21600;@12,10800\" o:connectangles=\"270,180,90,0\"/>\
<v:textpath on=\"t\" fitshape=\"t\"/><v:handles><v:h position=\"#0,bottomRight\" xrange=\"6629,14971\"/></v:handles><o:lock v:ext=\"edit\" text=\"t\" shapetype=\"t\"/></v:shapetype>\
<v:shape id=\"PowerPlusWaterMarkObject{index}\" o:spid=\"_x0000_s{index}025\" type=\"#_x0000_t136\" \
style=\"position:absolute;margin-left:0;margin-top:0;width:{WATERMARK_WIDTH}pt;height:{height:.1}pt;{rotation}z-index:-251657216;\
mso-position-horizontal:center;mso-position-horizontal-relative:margin;mso-position-vertical:center;mso-position-vertical-relative:margin\" \
o:allowincell=\"f\" fillcolor=\"silver\" stroked=\"f\"><v:fill opacity=\".5\"/><v:textpath style=\"font-family:&quot;Calibri&quot;;font-size:1pt\" string=\"{text}\"/>\
<w10:wrap anchorx=\"margin\" anchory=\"margin\"/></v:shape></w:pict></w:r></w:p>"
    )
}

fn set_margins(xml: &str, margins: Margins) -> String {
    let twips = |inches: f64| ((inches * TWIPS_PER_INCH).round() as i64).to_string();
    edit_each(xml, "w:pgMar", |raw| {
        let raw = set_attribute(raw, "w:top", &twips(margins.top));
        let raw = set_attribute(&raw, "w:right", &twips(margins.right));
        let raw = set_attribute(&raw, "w:bottom", &twips(margins.bottom));
        set_attribute(&raw, "w:left", &twips(margins.left))
    })
}

fn set_language(xml: &str, language: &str) -> String {
    let mut escaped = String::new();
    escape(language, &mut escaped);
    let set = |inner: &str| match find_element_start(inner, "<w:lang") {
        Some(start) => {
            let end = start + inner[start..].find("/>").map_or(0, |at| at + 2);
            format!("{}{}{}", &inner[..start], set_attribute(&inner[start..end], "w:val", &escaped), &inner[end..])
        }
        None => set_child(inner, "w:lang", &format!("<w:lang w:val=\"{escaped}\"/>"), RUN_ORDER),
    };
    if find_element_start(xml, "<w:rPrDefault").is_some() {
        return edit_each(xml, "w:rPrDefault", |defaults| edit_properties(defaults, "w:rPr", set));
    }
    let defaults = format!("<w:rPrDefault><w:rPr><w:lang w:val=\"{escaped}\"/></w:rPr></w:rPrDefault>");
    if find_element_start(xml, "<w:docDefaults").is_some() {
        return edit_each(xml, "w:docDefaults", |element| match element.find('>') {
            Some(at) if !element[..at].ends_with('/') => format!("{}{defaults}{}", &element[..=at], &element[at + 1..]),
            _ => format!("<w:docDefaults>{defaults}</w:docDefaults>"),
        });
    }
    edit_root(xml, "w:styles", |inner| format!("<w:docDefaults>{defaults}</w:docDefaults>{inner}"))
}
