//! `{:stylepar}`, `{:stylerow}`, `{:stylerun}`, `{:stylebullets}`, `{:styleshape}`,
//! `{:stylestart}` … `{:/style}`, `{::stylepar}` and `{::stylerun}`, and the properties
//! `{:stylecell}` sets beyond its fill. Built from the WordprocessingML and DrawingML specs and the
//! styling tags' documented values.

use serde_json::Value;

use crate::cell_styling::{read_fill, shade_cell};
use crate::properties::{CELL_ORDER, PARAGRAPH_ORDER, RUN_ORDER, edit_each, edit_properties, read_child, set_attribute, set_child};
use crate::xml::escape;

// Mark a `{:stylestart}` range's start (with its index) and end in the filled XML.
pub(crate) const RANGE_START: char = '\u{E002}';
pub(crate) const RANGE_START_END: char = '\u{E003}';
pub(crate) const RANGE_END: char = '\u{E004}';

/// `<wps:spPr>` children in schema order.
const SHAPE_ORDER: &[&str] = &[
    "a:xfrm", "a:custGeom", "a:prstGeom", "a:noFill", "a:solidFill", "a:gradFill", "a:blipFill", "a:pattFill", "a:grpFill", "a:ln", "a:effectLst",
    "a:effectDag", "a:scene3d", "a:sp3d", "a:extLst",
];
/// `<a:ln>` children in schema order.
const LINE_ORDER: &[&str] = &["a:noFill", "a:solidFill", "a:gradFill", "a:pattFill", "a:prstDash", "a:custDash", "a:round", "a:bevel", "a:miter", "a:headEnd", "a:tailEnd", "a:extLst"];
const FILLS: &[&str] = &["a:noFill", "a:solidFill", "a:gradFill", "a:blipFill", "a:pattFill", "a:grpFill"];
/// `<w:style>` children in schema order.
const STYLE_ORDER: &[&str] = &[
    "w:name", "w:aliases", "w:basedOn", "w:next", "w:link", "w:autoRedefine", "w:hidden", "w:uiPriority", "w:semiHidden", "w:unhideWhenUsed",
    "w:qFormat", "w:locked", "w:personal", "w:personalCompose", "w:personalReply", "w:rsid", "w:pPr", "w:rPr", "w:tblPr", "w:trPr", "w:tcPr",
    "w:tblStylePr",
];

// Border sides each container takes, in schema order.
const PARAGRAPH_SIDES: &[&str] = &["top", "left", "bottom", "right", "between", "bar"];
const CELL_SIDES: &[&str] = &["top", "start", "left", "bottom", "end", "right", "insideH", "insideV", "tl2br", "tr2bl"];

/// What a styling tag's value asks for.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Style {
    /// Six-digit hex.
    pub fill: Option<String>,
    pub text_color: Option<String>,
    /// A `w:jc` value.
    pub align: Option<&'static str>,
    pub paragraph_style: Option<String>,
    /// Half-points, as `w:sz`.
    pub font_size: Option<u64>,
    pub font_family: Option<String>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    /// Side and its `w:val`, `w:sz`, `w:space` and `w:color` attributes.
    pub borders: Vec<(String, String)>,
    /// A list paragraph's bullet or number, which takes the paragraph mark's colour.
    pub bullet_color: Option<String>,
    pub shape_fill: Option<String>,
    pub shape_stroke: Option<String>,
}

impl Style {
    /// This style with `over`'s settings on top.
    pub(crate) fn and(self, over: Style) -> Style {
        Style {
            fill: over.fill.or(self.fill),
            text_color: over.text_color.or(self.text_color),
            align: over.align.or(self.align),
            paragraph_style: over.paragraph_style.or(self.paragraph_style),
            font_size: over.font_size.or(self.font_size),
            font_family: over.font_family.or(self.font_family),
            bold: over.bold.or(self.bold),
            italic: over.italic.or(self.italic),
            underline: over.underline.or(self.underline),
            borders: if over.borders.is_empty() { self.borders } else { over.borders },
            bullet_color: over.bullet_color.or(self.bullet_color),
            shape_fill: over.shape_fill.or(self.shape_fill),
            shape_stroke: over.shape_stroke.or(self.shape_stroke),
        }
    }

    fn has_run_properties(&self) -> bool {
        self.text_color.is_some() || self.font_size.is_some() || self.font_family.is_some() || self.bold.is_some() || self.italic.is_some() || self.underline.is_some()
    }

    fn has_paragraph_properties(&self) -> bool {
        self.align.is_some() || self.paragraph_style.is_some() || self.bullet_color.is_some() || self.has_run_properties()
    }
}

/// The style an object value asks for; `None` for any other value.
///
/// Keys: `cellBackground`, `textColor` (hex), `textAlign` (`left`, `center`, `right`, `justify`),
/// `pStyle`, `fontSize` (half-points), `fontFamily`, `bold`, `italic`, `underline` and
/// `borders` (`{ "top": { "val", "sz", "space", "color" } }`), and for bullets `bulletColor`, for
/// shapes `fillColor` and `strokeColor`.
pub(crate) fn read_style(value: Option<&Value>) -> Option<Style> {
    let value = value.filter(|value| value.is_object())?;
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty());
    let flag = |key: &str| value.get(key).and_then(Value::as_bool);
    let borders = value.get("borders").and_then(Value::as_object).map_or_else(Vec::new, |sides| {
        sides.iter().filter_map(|(side, border)| Some((side.clone(), read_border(border)?))).collect()
    });
    Some(Style {
        fill: read_fill(Some(value)),
        text_color: text("textColor").and_then(read_hex),
        align: text("textAlign").and_then(read_align),
        paragraph_style: text("pStyle").map(str::to_string),
        font_size: value.get("fontSize").and_then(Value::as_f64).filter(|size| *size > 0.0).map(|size| size.round() as u64),
        font_family: text("fontFamily").map(str::to_string),
        bold: flag("bold"),
        italic: flag("italic"),
        underline: flag("underline"),
        borders,
        bullet_color: text("bulletColor").and_then(read_hex),
        shape_fill: text("fillColor").and_then(read_hex),
        shape_stroke: text("strokeColor").and_then(read_hex),
    })
}

fn read_hex(color: &str) -> Option<String> {
    let hex = color.strip_prefix('#').unwrap_or(color);
    if !(hex.len() == 6 || hex.len() == 3) || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let full: String = if hex.len() == 3 { hex.chars().flat_map(|digit| [digit, digit]).collect() } else { hex.to_string() };
    Some(full.to_ascii_uppercase())
}

fn read_align(align: &str) -> Option<&'static str> {
    match align {
        "left" | "start" => Some("left"),
        "center" => Some("center"),
        "right" | "end" => Some("right"),
        "justify" | "both" => Some("both"),
        _ => None,
    }
}

/// A border's attributes, as written inside its element.
fn read_border(border: &Value) -> Option<String> {
    let mut attributes = String::new();
    for key in ["val", "sz", "space", "color"] {
        let Some(value) = border.get(key) else { continue };
        let value = match value {
            Value::String(text) if key == "color" => read_hex(text).unwrap_or_else(|| text.clone()),
            Value::String(text) => text.clone(),
            Value::Number(number) => number.to_string(),
            _ => continue,
        };
        attributes.push_str(&format!(" w:{key}=\"{}\"", escape_attribute(&value)));
    }
    Some(attributes)
}

fn escape_attribute(text: &str) -> String {
    let mut out = String::new();
    escape(text, &mut out);
    out
}

fn write_borders(name: &str, sides: &[&str], borders: &[(String, String)]) -> String {
    let mut out = format!("<{name}>");
    for side in sides {
        if let Some((_, attributes)) = borders.iter().find(|(each, _)| each == side) {
            out.push_str(&format!("<w:{side}{attributes}/>"));
        }
    }
    out.push_str(&format!("</{name}>"));
    out
}

/// `<w:rPr>` content with the style's run properties set.
fn style_run_properties(inner: &str, style: &Style) -> String {
    let mut inner = inner.to_string();
    let toggle = |is_on: bool, name: &str| if is_on { format!("<{name}/>") } else { format!("<{name} w:val=\"0\"/>") };
    if let Some(family) = &style.font_family {
        let family = escape_attribute(family);
        inner = set_child(&inner, "w:rFonts", &format!("<w:rFonts w:ascii=\"{family}\" w:hAnsi=\"{family}\" w:cs=\"{family}\" w:eastAsia=\"{family}\"/>"), RUN_ORDER);
    }
    if let Some(bold) = style.bold {
        inner = set_child(&inner, "w:b", &toggle(bold, "w:b"), RUN_ORDER);
        inner = set_child(&inner, "w:bCs", &toggle(bold, "w:bCs"), RUN_ORDER);
    }
    if let Some(italic) = style.italic {
        inner = set_child(&inner, "w:i", &toggle(italic, "w:i"), RUN_ORDER);
        inner = set_child(&inner, "w:iCs", &toggle(italic, "w:iCs"), RUN_ORDER);
    }
    if let Some(color) = &style.text_color {
        inner = set_child(&inner, "w:color", &format!("<w:color w:val=\"{color}\"/>"), RUN_ORDER);
    }
    if let Some(size) = style.font_size {
        inner = set_child(&inner, "w:sz", &format!("<w:sz w:val=\"{size}\"/>"), RUN_ORDER);
        inner = set_child(&inner, "w:szCs", &format!("<w:szCs w:val=\"{size}\"/>"), RUN_ORDER);
    }
    if let Some(underline) = style.underline {
        let value = if underline { "single" } else { "none" };
        inner = set_child(&inner, "w:u", &format!("<w:u w:val=\"{value}\"/>"), RUN_ORDER);
    }
    inner
}

/// `run`, a whole `<w:r>`, with the style's run properties.
pub(crate) fn style_run(run: &str, style: &Style) -> String {
    if !style.has_run_properties() {
        return run.to_string();
    }
    edit_properties(run, "w:rPr", |inner| style_run_properties(inner, style))
}

/// `paragraph`, a whole `<w:p>`, with the style's paragraph properties, and its run properties on
/// every run and the paragraph mark. `is_own` adds the fill and borders, which a cell's style
/// keeps for the cell.
pub(crate) fn style_paragraph(paragraph: &str, style: &Style, is_own: bool) -> String {
    let has_own = is_own && (style.fill.is_some() || !style.borders.is_empty());
    if !style.has_paragraph_properties() && !has_own {
        return paragraph.to_string();
    }
    let styled = edit_properties(paragraph, "w:pPr", |inner| {
        let mut inner = inner.to_string();
        if let Some(name) = &style.paragraph_style {
            inner = set_child(&inner, "w:pStyle", &format!("<w:pStyle w:val=\"{}\"/>", escape_attribute(name)), PARAGRAPH_ORDER);
        }
        if is_own && !style.borders.is_empty() {
            inner = set_child(&inner, "w:pBdr", &write_borders("w:pBdr", PARAGRAPH_SIDES, &style.borders), PARAGRAPH_ORDER);
        }
        if let (true, Some(fill)) = (is_own, &style.fill) {
            inner = set_child(&inner, "w:shd", &format!("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{fill}\"/>"), PARAGRAPH_ORDER);
        }
        if let Some(align) = style.align {
            inner = set_child(&inner, "w:jc", &format!("<w:jc w:val=\"{align}\"/>"), PARAGRAPH_ORDER);
        }
        if style.has_run_properties() || style.bullet_color.is_some() {
            let mark = read_child(&inner, "w:rPr").unwrap_or_default();
            let mut mark = style_run_properties(mark, style);
            if let Some(color) = &style.bullet_color {
                mark = set_child(&mark, "w:color", &format!("<w:color w:val=\"{color}\"/>"), RUN_ORDER);
            }
            inner = set_child(&inner, "w:rPr", &format!("<w:rPr>{mark}</w:rPr>"), PARAGRAPH_ORDER);
        }
        inner
    });
    edit_each(&styled, "w:r", |run| style_run(run, style))
}

/// `cell`, a whole `<w:tc>`, with the style's fill and borders, and its paragraph and run
/// properties on every paragraph in it.
pub(crate) fn style_cell(cell: &str, style: &Style) -> String {
    let mut cell = match &style.fill {
        Some(fill) => shade_cell(cell, fill),
        None => cell.to_string(),
    };
    if !style.borders.is_empty() {
        cell = edit_properties(&cell, "w:tcPr", |inner| set_child(inner, "w:tcBorders", &write_borders("w:tcBorders", CELL_SIDES, &style.borders), CELL_ORDER));
    }
    if style.has_paragraph_properties() {
        cell = edit_each(&cell, "w:p", |paragraph| style_paragraph(paragraph, style, false));
    }
    cell
}

/// `row`, a whole `<w:tr>`, with every cell styled.
pub(crate) fn style_row(row: &str, style: &Style) -> String {
    edit_each(row, "w:tc", |cell| style_cell(cell, style))
}

/// `shape`, a whole `<wps:wsp>` or VML shape, with the style's fill and outline colours.
pub(crate) fn style_shape(shape: &str, style: &Style) -> String {
    if style.shape_fill.is_none() && style.shape_stroke.is_none() {
        return shape.to_string();
    }
    if !shape.starts_with("<wps:") {
        let Some(open_end) = shape.find('>').map(|at| at + 1) else { return shape.to_string() };
        let mut open = shape[..open_end].to_string();
        if let Some(fill) = &style.shape_fill {
            open = set_attribute(&open, "fillcolor", &format!("#{fill}"));
            open = set_attribute(&open, "filled", "t");
        }
        if let Some(stroke) = &style.shape_stroke {
            open = set_attribute(&open, "strokecolor", &format!("#{stroke}"));
            open = set_attribute(&open, "stroked", "t");
        }
        return format!("{open}{}", &shape[open_end..]);
    }
    edit_each(shape, "wps:spPr", |properties| {
        let properties = if let Some(open) = properties.strip_suffix("/>") { format!("{open}></wps:spPr>") } else { properties.to_string() };
        let open_end = properties.find('>').map_or(0, |at| at + 1);
        let close = properties.len() - "</wps:spPr>".len();
        let mut inner = properties[open_end..close].to_string();
        if let Some(fill) = &style.shape_fill {
            for other in FILLS {
                inner = set_child(&inner, other, "", SHAPE_ORDER);
            }
            inner = set_child(&inner, "a:solidFill", &solid_fill(fill), SHAPE_ORDER);
        }
        if let Some(stroke) = &style.shape_stroke {
            inner = match read_child(&inner, "a:ln") {
                Some(_) => edit_each(&inner, "a:ln", |line| set_line_fill(line, stroke)),
                None => set_child(&inner, "a:ln", &format!("<a:ln>{}</a:ln>", solid_fill(stroke)), SHAPE_ORDER),
            };
        }
        format!("{}{inner}</wps:spPr>", &properties[..open_end])
    })
}

fn solid_fill(color: &str) -> String {
    format!("<a:solidFill><a:srgbClr val=\"{color}\"/></a:solidFill>")
}

fn set_line_fill(line: &str, color: &str) -> String {
    let line = if let Some(open) = line.strip_suffix("/>") { format!("{open}></a:ln>") } else { line.to_string() };
    let open_end = line.find('>').map_or(0, |at| at + 1);
    let close = line.len() - "</a:ln>".len();
    let mut inner = line[open_end..close].to_string();
    for other in FILLS {
        inner = set_child(&inner, other, "", LINE_ORDER);
    }
    inner = set_child(&inner, "a:solidFill", &solid_fill(color), LINE_ORDER);
    format!("{}{inner}</a:ln>", &line[..open_end])
}

/// `xml` with each run between a `{:stylestart}` mark and its `{:/style}` mark styled by the
/// ranges open around it. A run holding a mark is split there, and the marks go.
pub(crate) fn style_ranges(xml: &str, styles: &[Style]) -> String {
    let mut open: Vec<usize> = Vec::new();
    edit_each(xml, "w:r", |run| {
        let current = |open: &[usize]| open.iter().fold(None::<Style>, |style, index| Some(style.unwrap_or_default().and(styles[*index].clone())));
        if !run.contains([RANGE_START, RANGE_END]) {
            return match current(&open) {
                Some(style) => style_run(run, &style),
                None => run.to_string(),
            };
        }

        // The run's start tag and properties, repeated on each piece.
        let open_end = run.find('>').map_or(0, |at| at + 1);
        let rest = &run[open_end..run.len() - "</w:r>".len()];
        let (props, content) = match rest.strip_prefix("<w:rPr/>") {
            Some(content) => ("", content),
            None if rest.starts_with("<w:rPr>") => match rest.find("</w:rPr>") {
                Some(end) => rest.split_at(end + "</w:rPr>".len()),
                None => ("", rest),
            },
            None => ("", rest),
        };
        let start = &run[..open_end];
        let mut out = String::new();
        let mut piece = String::new();
        let write = |out: &mut String, piece: &str, style: Option<Style>| {
            let bare = piece.replace("<w:t xml:space=\"preserve\"></w:t>", "").replace("<w:t></w:t>", "");
            if bare.is_empty() {
                return;
            }
            let written = format!("{start}{props}{piece}</w:r>");
            out.push_str(&match style {
                Some(style) => style_run(&written, &style),
                None => written,
            });
        };
        let mut chars = content.char_indices().peekable();
        while let Some((_, c)) = chars.next() {
            if c != RANGE_START && c != RANGE_END {
                piece.push(c);
                continue;
            }
            piece.push_str("</w:t>");
            write(&mut out, &piece, current(&open));
            piece = "<w:t xml:space=\"preserve\">".to_string();
            if c == RANGE_END {
                open.pop();
                continue;
            }
            let mut index = String::new();
            for (_, digit) in chars.by_ref() {
                if digit == RANGE_START_END {
                    break;
                }
                index.push(digit);
            }
            if let Ok(index) = index.parse::<usize>().map(|index| index.min(styles.len().saturating_sub(1))) {
                open.push(index);
            }
        }
        write(&mut out, &piece, current(&open));
        out
    })
}

const THEME_COLOR_NAMES: &[&str] = &[
    "a:dk1", "a:lt1", "a:dk2", "a:lt2", "a:accent1", "a:accent2", "a:accent3", "a:accent4", "a:accent5", "a:accent6", "a:hlink", "a:folHlink",
];
const THEME_COLOR_TYPES: &[&str] = &["a:srgbClr", "a:scrgbClr", "a:hslClr", "a:sysClr", "a:schemeClr", "a:prstClr"];

pub(crate) fn validate_theme_colors(colors: &std::collections::BTreeMap<String, String>) -> Result<(), crate::FillError> {
    for (name, color) in colors {
        if !THEME_COLOR_NAMES.contains(&name.as_str()) {
            return Err(crate::FillError::new(format!("Unknown theme color \"{name}\"")));
        }
        if read_hex(color).is_none() {
            return Err(crate::FillError::new(format!("Invalid hex color \"{color}\" for {name}")));
        }
    }
    Ok(())
}

/// Replaces the color choice in each configured DrawingML theme slot.
pub(crate) fn set_theme_colors(xml: &str, colors: &std::collections::BTreeMap<String, String>) -> String {
    let mut xml = xml.to_string();
    for (name, color) in colors {
        let Some(color) = read_hex(color) else { continue };
        xml = edit_each(&xml, name, |element| {
            let Some(open_end) = element.find('>').map(|at| at + 1) else { return element.to_string() };
            let open = if element[..open_end].ends_with("/>") {
                format!("{}>", &element[..open_end - 2])
            } else {
                element[..open_end].to_string()
            };
            let close = format!("</{name}>");
            let mut inner = if element[..open_end].ends_with("/>") || !element.ends_with(&close) {
                String::new()
            } else {
                element[open_end..element.len() - close.len()].to_string()
            };
            for color_type in THEME_COLOR_TYPES {
                inner = edit_each(&inner, color_type, |_| String::new());
            }
            let replacement = format!("<a:srgbClr val=\"{color}\"/>");
            if let Some(ext) = inner.find("<a:extLst") {
                inner.insert_str(ext, &replacement);
            } else {
                inner.push_str(&replacement);
            }
            format!("{open}{inner}{close}")
        });
    }
    xml
}

/// Which style definitions a global styling tag changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StyleKind {
    Paragraph,
    Character,
}

impl StyleKind {
    fn name(self) -> &'static str {
        match self {
            StyleKind::Paragraph => "paragraph",
            StyleKind::Character => "character",
        }
    }
}

/// `styles`, a styles part, with the definition of style `id` (the default style of `kind` for
/// `None`) given the style's properties.
pub(crate) fn style_definition(styles: &str, kind: StyleKind, id: Option<&str>, style: &Style) -> String {
    edit_each(styles, "w:style", |definition| {
        let open_end = definition.find('>').map_or(0, |at| at + 1);
        let open = &definition[..open_end];
        if crate::xml::read_attribute(open, "w:type") != Some(kind.name()) {
            return definition.to_string();
        }
        let is_match = match id {
            Some(id) => crate::xml::read_attribute(open, "w:styleId") == Some(id),
            None => matches!(crate::xml::read_attribute(open, "w:default"), Some("1" | "true" | "on")),
        };
        if !is_match || open.ends_with("/>") {
            return definition.to_string();
        }
        let mut inner = definition[open_end..definition.len() - "</w:style>".len()].to_string();
        if kind == StyleKind::Paragraph && (style.align.is_some() || style.fill.is_some() || !style.borders.is_empty()) {
            let mut properties = read_child(&inner, "w:pPr").unwrap_or_default().to_string();
            if !style.borders.is_empty() {
                properties = set_child(&properties, "w:pBdr", &write_borders("w:pBdr", PARAGRAPH_SIDES, &style.borders), PARAGRAPH_ORDER);
            }
            if let Some(fill) = &style.fill {
                properties = set_child(&properties, "w:shd", &format!("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{fill}\"/>"), PARAGRAPH_ORDER);
            }
            if let Some(align) = style.align {
                properties = set_child(&properties, "w:jc", &format!("<w:jc w:val=\"{align}\"/>"), PARAGRAPH_ORDER);
            }
            inner = set_child(&inner, "w:pPr", &format!("<w:pPr>{properties}</w:pPr>"), STYLE_ORDER);
        }
        if style.has_run_properties() {
            let properties = style_run_properties(read_child(&inner, "w:rPr").unwrap_or_default(), style);
            inner = set_child(&inner, "w:rPr", &format!("<w:rPr>{properties}</w:rPr>"), STYLE_ORDER);
        }
        format!("{open}{inner}</w:style>")
    })
}
