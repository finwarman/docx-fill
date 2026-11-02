//! Edits an element's property children (`<w:pPr>`, `<w:rPr>`, `<w:tcPr>`, …) in the order the
//! WordprocessingML schema gives them.

use crate::xml::{ElementKind, Token, tokenize};

/// `<w:pPr>` children in schema order.
pub(crate) const PARAGRAPH_ORDER: &[&str] = &[
    "w:pStyle", "w:keepNext", "w:keepLines", "w:pageBreakBefore", "w:framePr", "w:widowControl", "w:numPr", "w:suppressLineNumbers",
    "w:pBdr", "w:shd", "w:tabs", "w:suppressAutoHyphens", "w:kinsoku", "w:wordWrap", "w:overflowPunct", "w:topLinePunct", "w:autoSpaceDE",
    "w:autoSpaceDN", "w:bidi", "w:adjustRightInd", "w:snapToGrid", "w:spacing", "w:ind", "w:contextualSpacing", "w:mirrorIndents",
    "w:suppressOverlap", "w:jc", "w:textDirection", "w:textAlignment", "w:textboxTightWrap", "w:outlineLvl", "w:divId", "w:cnfStyle",
    "w:rPr", "w:sectPr", "w:pPrChange",
];

/// `<w:rPr>` children in schema order.
pub(crate) const RUN_ORDER: &[&str] = &[
    "w:ins", "w:del", "w:moveFrom", "w:moveTo", "w:rStyle", "w:rFonts", "w:b", "w:bCs", "w:i", "w:iCs", "w:caps", "w:smallCaps", "w:strike",
    "w:dstrike", "w:outline", "w:shadow", "w:emboss", "w:imprint", "w:noProof", "w:snapToGrid", "w:vanish", "w:webHidden", "w:color",
    "w:spacing", "w:w", "w:kern", "w:position", "w:sz", "w:szCs", "w:highlight", "w:u", "w:effect", "w:bdr", "w:shd", "w:fitText",
    "w:vertAlign", "w:rtl", "w:cs", "w:em", "w:lang", "w:eastAsianLayout", "w:specVanish", "w:oMath", "w:rPrChange",
];

/// `<w:tcPr>` children in schema order.
pub(crate) const CELL_ORDER: &[&str] = &[
    "w:cnfStyle", "w:tcW", "w:gridSpan", "w:hMerge", "w:vMerge", "w:tcBorders", "w:shd", "w:noWrap", "w:tcMar", "w:textDirection",
    "w:tcFitText", "w:vAlign", "w:hideMark", "w:headers", "w:cellIns", "w:cellDel", "w:cellMerge", "w:tcPrChange",
];

/// `<w:settings>` children from `<w:documentProtection>` on, in schema order.
pub(crate) const SETTINGS_FROM_PROTECTION: &[&str] = &[
    "w:documentProtection",
    "w:autoFormatOverride", "w:styleLockTheme", "w:styleLockQFSet", "w:defaultTabStop", "w:autoHyphenation", "w:consecutiveHyphenLimit",
    "w:hyphenationZone", "w:doNotHyphenateCaps", "w:showEnvelope", "w:summaryLength", "w:clickAndTypeStyle", "w:defaultTableStyle",
    "w:evenAndOddHeaders", "w:bookFoldRevPrinting", "w:bookFoldPrinting", "w:bookFoldPrintingSheets", "w:drawingGridHorizontalSpacing",
    "w:drawingGridVerticalSpacing", "w:displayHorizontalDrawingGridEvery", "w:displayVerticalDrawingGridEvery",
    "w:doNotUseMarginsForDrawingGridOrigin", "w:drawingGridHorizontalOrigin", "w:drawingGridVerticalOrigin", "w:doNotShadeFormData",
    "w:noPunctuationKerning", "w:characterSpacingControl", "w:printTwoOnOne", "w:strictFirstAndLastChars", "w:noLineBreaksAfter",
    "w:noLineBreaksBefore", "w:savePreviewPicture", "w:doNotValidateAgainstSchema", "w:saveInvalidXml", "w:ignoreMixedContent",
    "w:alwaysShowPlaceholderText", "w:doNotDemarcateInvalidXml", "w:saveXmlDataOnly", "w:useXSLTWhenSaving", "w:saveThroughXslt",
    "w:showXMLTags", "w:alwaysMergeEmptyNamespace", "w:updateFields", "w:hdrShapeDefaults", "w:footnotePr", "w:endnotePr", "w:compat",
    "w:docVars", "w:rsids", "m:mathPr", "w:attachedSchema", "w:themeFontLang", "w:clrSchemeMapping", "w:doNotIncludeSubdocsInStats",
    "w:doNotAutoCompressPictures", "w:forceUpgrade", "w:captions", "w:readModeInkLockDown", "w:smartTagType", "sl:schemaLibrary",
    "w:shapeDefaults", "w:doNotEmbedSmartTags", "w:decimalSymbol", "w:listSeparator",
];

/// A direct child's name and byte range in `inner`, the content of a properties element.
fn read_children(inner: &str) -> Vec<(String, usize, usize)> {
    let Ok(tokens) = tokenize(inner) else { return Vec::new() };
    let mut children = Vec::new();
    let mut depth = 0usize;
    let mut at = 0;
    let mut open: Option<(String, usize)> = None;
    for token in tokens {
        let length = match token {
            Token::Element { raw, .. } => raw.len(),
            Token::Text(text) => text.len(),
        };
        if let Token::Element { kind, name, .. } = token {
            match kind {
                ElementKind::SelfClosing if depth == 0 => children.push((name.to_string(), at, at + length)),
                ElementKind::Open => {
                    if depth == 0 {
                        open = Some((name.to_string(), at));
                    }
                    depth += 1;
                }
                ElementKind::Close => {
                    depth = depth.saturating_sub(1);
                    if depth == 0
                        && let Some((name, start)) = open.take()
                    {
                        children.push((name, start, at + length));
                    }
                }
                _ => {}
            }
        }
        at += length;
    }
    children
}

/// `inner` with its child `name` replaced by `child`, or `child` added where `order` places it.
/// An empty `child` removes the existing one.
pub(crate) fn set_child(inner: &str, name: &str, child: &str, order: &[&str]) -> String {
    let children = read_children(inner);
    if let Some((_, start, end)) = children.iter().find(|(each, ..)| each == name) {
        return format!("{}{child}{}", &inner[..*start], &inner[*end..]);
    }
    let rank = |each: &str| order.iter().position(|ordered| *ordered == each);
    let Some(own) = rank(name) else { return format!("{inner}{child}") };
    match children.iter().find(|(each, ..)| rank(each).is_some_and(|other| other > own)) {
        Some((_, start, _)) => format!("{}{child}{}", &inner[..*start], &inner[*start..]),
        None => format!("{inner}{child}"),
    }
}

/// The content of child `name` in `inner`, when it has one.
pub(crate) fn read_child<'a>(inner: &'a str, name: &str) -> Option<&'a str> {
    let (_, start, end) = read_children(inner).into_iter().find(|(each, ..)| each == name)?;
    let element = &inner[start..end];
    let open_end = element.find('>')? + 1;
    if element[..open_end].ends_with("/>") {
        return Some("");
    }
    Some(&element[open_end..element.len() - name.len() - 3])
}

/// `element` (a whole `<name …>…</name>`) with its first child `properties` rewritten by `edit`.
/// The properties element is added as the first child when missing.
pub(crate) fn edit_properties(element: &str, properties: &str, edit: impl FnOnce(&str) -> String) -> String {
    let Some(open_end) = element.find('>').map(|at| at + 1) else { return element.to_string() };
    if element[..open_end].ends_with("/>") {
        return element.to_string();
    }
    let rest = &element[open_end..];
    let (open, empty, close) = (format!("<{properties}>"), format!("<{properties}/>"), format!("</{properties}>"));
    let (inner, after) = if let Some(after) = rest.strip_prefix(&empty) {
        ("", after)
    } else if let Some(body) = rest.strip_prefix(&open) {
        match body.find(&close) {
            Some(end) => (&body[..end], &body[end + close.len()..]),
            None => return element.to_string(),
        }
    } else {
        ("", rest)
    };
    let edited = edit(inner);
    format!("{}<{properties}>{edited}</{properties}>{after}", &element[..open_end])
}

/// `raw`, an element's start tag, with attribute `name` set to `value`.
pub(crate) fn set_attribute(raw: &str, name: &str, value: &str) -> String {
    if let Some(old) = crate::xml::read_attribute(raw, name) {
        let at = crate::xml::offset_in(raw, old);
        let quote = &raw[at - 1..at];
        return format!("{}{quote}{value}{quote}{}", &raw[..at - 1], &raw[at + old.len() + 1..]);
    }
    let end = if raw.ends_with("/>") { raw.len() - 2 } else { raw.len() - 1 };
    format!("{} {name}=\"{value}\"{}", &raw[..end], &raw[end..])
}

/// Each start of element `name` (`<w:r>` or `<w:r …>`, not `<w:rPr>`) in `xml`.
pub(crate) fn find_starts(xml: &str, name: &str) -> Vec<usize> {
    let prefix = format!("<{name}");
    let mut starts = Vec::new();
    let mut from = 0;
    while let Some(at) = xml[from..].find(&prefix) {
        let at = from + at;
        let next = xml[at + prefix.len()..].chars().next();
        if next.is_some_and(|c| c.is_whitespace() || c == '>' || c == '/') {
            starts.push(at);
        }
        from = at + prefix.len();
    }
    starts
}

/// The end of the element starting at `start`, counting nested elements of the same name.
pub(crate) fn find_element_end(xml: &str, start: usize, name: &str) -> Option<usize> {
    let tokens = tokenize(&xml[start..]).ok()?;
    let mut depth = 0usize;
    let mut at = start;
    for token in tokens {
        let length = match token {
            Token::Element { raw, .. } => raw.len(),
            Token::Text(text) => text.len(),
        };
        if let Token::Element { kind, name: each, .. } = token
            && each == name
        {
            match kind {
                ElementKind::SelfClosing if depth == 0 => return Some(at + length),
                ElementKind::Open => depth += 1,
                ElementKind::Close => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(at + length);
                    }
                }
                _ => {}
            }
        }
        at += length;
    }
    None
}

/// `xml` with every `name` element rewritten by `edit`, outermost first, not nested ones.
pub(crate) fn edit_each(xml: &str, name: &str, mut edit: impl FnMut(&str) -> String) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut at = 0;
    for start in find_starts(xml, name) {
        if start < at {
            continue;
        }
        let Some(end) = find_element_end(xml, start, name) else { continue };
        out.push_str(&xml[at..start]);
        out.push_str(&edit(&xml[start..end]));
        at = end;
    }
    out.push_str(&xml[at..]);
    out
}
