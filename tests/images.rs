//! Mirrors `apps/web/src/components/pdf/wordcraft/__tests__/reportPhotos.test.ts` in Scope's
//! frontend: `{%tag}` draws an image filling its table cell, else the page's width.

use std::borrow::Cow;
use std::sync::Arc;

use docx_fill::{FillHooks, FillOptions, Image, PackageTemplate};
use serde_json::{Value, json};

const NAMESPACES: &str = concat!(
    r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" "#,
    r#"xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" "#,
    r#"xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing""#
);
const EMU_PER_PX: f64 = 9525.0;
// 1440 twips is an inch, 96 px.
const CELL_TWIPS: u32 = 2880;
// Less 5% for cell padding.
const FILLED_CELL_PX: i64 = 182;
// A4 with 1 inch margins.
const SECTION: &str = r#"<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440"/></w:sectPr>"#;
const CONTENT_TYPES: &str = r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/></Types>"#;

/// Draws `"photo:<key>"` values as a 400 by 200 JPEG named `<key>`.
struct Photos;

impl FillHooks for Photos {
    fn image(&mut self, _part: &str, value: &Value) -> Option<Image> {
        let key = value.as_str()?.strip_prefix("photo:")?;
        Some(Image { key: key.to_string(), bytes: Arc::from(&[0xff, 0xd8, 0xff][..]), extension: "jpg".to_string(), width: 400.0, height: 200.0 })
    }
}

fn paragraph(text: &str) -> String {
    format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")
}

fn cell(text: &str) -> String {
    format!(r#"<w:tbl><w:tr><w:tc><w:tcPr><w:tcW w:w="{CELL_TWIPS}" w:type="dxa"/></w:tcPr>{}</w:tc></w:tr></w:tbl>"#, paragraph(text))
}

fn fill(parts: Vec<(&str, String)>, tags: Value) -> Vec<(String, String)> {
    let parts = parts.into_iter().map(|(name, xml)| (name.to_string(), xml.into_bytes()));
    let template = PackageTemplate::parse(parts, &FillOptions::default()).unwrap();
    template.render_with(&tags, &mut Photos).into_iter().map(|(name, bytes): (String, Cow<[u8]>)| (name, String::from_utf8_lossy(&bytes).into_owned())).collect()
}

fn document(body: &str) -> Vec<(&'static str, String)> {
    vec![
        ("[Content_Types].xml", CONTENT_TYPES.to_string()),
        ("word/document.xml", format!("<w:document {NAMESPACES}><w:body>{body}{SECTION}</w:body></w:document>")),
        ("word/_rels/document.xml.rels", r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"></Relationships>"#.to_string()),
    ]
}

fn part<'a>(parts: &'a [(String, String)], name: &str) -> Option<&'a str> {
    parts.iter().find(|(each, _)| each == name).map(|(_, xml)| xml.as_str())
}

fn extents(xml: &str) -> Vec<(i64, i64)> {
    xml.match_indices("<wp:extent cx=\"")
        .map(|(at, _)| {
            let rest = &xml[at + 15..];
            let cx: f64 = rest[..rest.find('"').unwrap()].parse().unwrap();
            let rest = &rest[rest.find("cy=\"").unwrap() + 4..];
            let cy: f64 = rest[..rest.find('"').unwrap()].parse().unwrap();
            ((cx / EMU_PER_PX).round() as i64, (cy / EMU_PER_PX).round() as i64)
        })
        .collect()
}

#[test]
fn draws_an_image_filling_its_cell_keeping_its_shape() {
    let filled = fill(document(&cell("{%photo:a.value.image}")), json!({ "photo:a.value.image": "photo:loaded" }));

    let xml = part(&filled, "word/document.xml").unwrap();
    assert_eq!(extents(xml), [(FILLED_CELL_PX, FILLED_CELL_PX / 2)]);
    assert!(xml.contains(r#"r:embed="rIdDocxFill_loaded""#));
    assert!(part(&filled, "word/media/docx_fill_loaded.jpg").is_some());
    assert!(part(&filled, "word/_rels/document.xml.rels").unwrap().contains(r#"<Relationship Id="rIdDocxFill_loaded" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/docx_fill_loaded.jpg"/>"#));
    assert!(part(&filled, "[Content_Types].xml").unwrap().contains(r#"<Default Extension="jpg" ContentType="image/jpeg"/>"#));
}

#[test]
fn prints_the_drawing_in_the_tags_run_and_nothing_for_an_empty_value() {
    let body = paragraph("Before {%photo:b.value.image} after") + &paragraph("{%%photo:empty.value.image}");

    let filled = fill(document(&body), json!({ "photo:b.value.image": "photo:b", "photo:empty.value.image": "" }));

    let xml = part(&filled, "word/document.xml").unwrap();
    assert_eq!(xml.matches("<w:drawing>").count(), 1);
    assert!(xml.contains("Before </w:t><w:drawing>"));
    assert!(xml.contains(r#"<w:t xml:space="preserve"> after</w:t>"#));
}

#[test]
fn fills_the_bodys_width_outside_a_cell_and_fits_its_height() {
    let filled = fill(document(&paragraph("{%wide}{%tall}")), json!({ "wide": "photo:wide", "tall": "photo:tall" }));

    // A4 less 1 inch margins: 602 px by 931 px, less 5%.
    let xml = part(&filled, "word/document.xml").unwrap();
    assert_eq!(extents(xml), [(572, 286), (572, 286)]);
}

#[test]
fn shares_one_media_part_between_drawings_of_one_image() {
    let filled = fill(document(&(paragraph("{%a}") + &paragraph("{%b}"))), json!({ "a": "photo:same", "b": "photo:same" }));

    let xml = part(&filled, "word/document.xml").unwrap();
    assert_eq!(xml.matches("<wp:docPr id=").count(), 2);
    assert!(xml.contains(r#"<wp:docPr id="1" "#) && xml.contains(r#"<wp:docPr id="2" "#));
    assert_eq!(filled.iter().filter(|(name, _)| name.starts_with("word/media/")).count(), 1);
    assert_eq!(part(&filled, "word/_rels/document.xml.rels").unwrap().matches("rIdDocxFill_same").count(), 1);
}

#[test]
fn numbers_drawings_after_the_templates_own() {
    let body = r#"<w:p><w:r><w:drawing><wp:inline><wp:docPr id="41" name="logo"/></wp:inline></w:drawing></w:r></w:p>"#.to_string() + &paragraph("{%a}");

    let filled = fill(document(&body), json!({ "a": "photo:a" }));

    assert!(part(&filled, "word/document.xml").unwrap().contains(r#"<wp:docPr id="42" name="Photo 42"/>"#));
}

#[test]
fn draws_an_image_in_a_header_with_the_headers_own_relationships() {
    let mut parts = document("");
    parts.push(("word/header1.xml", format!("<w:hdr {NAMESPACES}>{}</w:hdr>", paragraph("{%logo}"))));

    let filled = fill(parts, json!({ "logo": "photo:logo" }));

    assert!(part(&filled, "word/header1.xml").unwrap().contains(r#"r:embed="rIdDocxFill_logo""#));
    assert!(part(&filled, "word/_rels/header1.xml.rels").unwrap().contains(r#"Id="rIdDocxFill_logo""#));
    assert_eq!(extents(part(&filled, "word/header1.xml").unwrap()), [(572, 286)]);
}

#[test]
fn prints_nothing_when_the_hook_has_no_image() {
    let filled = fill(document(&paragraph("[{%x}]")), json!({ "x": "not a photo" }));

    assert!(!part(&filled, "word/document.xml").unwrap().contains("<w:drawing>"));
}

#[test]
fn centres_a_double_percent_images_paragraph() {
    let filled = fill(document(&(paragraph("{%%centred}") + &paragraph("{%plain}"))), json!({ "centred": "photo:c", "plain": "photo:p" }));

    let xml = part(&filled, "word/document.xml").unwrap();
    let paragraphs: Vec<&str> = xml.split("</w:p>").collect();
    assert!(paragraphs[0].contains(r#"<w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r>"#));
    assert!(!paragraphs[1].contains("w:jc"));
}

#[test]
fn leaves_a_centred_tags_paragraph_alone_when_it_draws_nothing() {
    let filled = fill(document(&paragraph("{%%centred}")), json!({ "centred": "" }));

    assert!(!part(&filled, "word/document.xml").unwrap().contains("w:jc"));
}

#[test]
fn rejects_a_centred_image_beside_other_text() {
    let parts = document(&paragraph("Logo {%%logo}")).into_iter().map(|(name, xml)| (name.to_string(), xml.into_bytes()));
    let error = PackageTemplate::parse(parts, &FillOptions::default()).unwrap_err();

    assert_eq!(error.message, r#"Centered Images should be placed in empty paragraphs, but the tag "%%logo" is not"#);
}

#[test]
fn keeps_images_apart_when_their_keys_differ_only_in_symbols() {
    let filled = fill(document(&(paragraph("{%a}") + &paragraph("{%b}"))), json!({ "a": "photo:a-1", "b": "photo:a1" }));

    assert_eq!(filled.iter().filter(|(name, _)| name.starts_with("word/media/")).count(), 2);
    assert_eq!(part(&filled, "word/_rels/document.xml.rels").unwrap().matches("<Relationship ").count(), 2);
}

#[test]
fn keeps_a_very_tall_image_in_a_cell_within_word_limits() {
    struct Tall;
    impl FillHooks for Tall {
        fn image(&mut self, _part: &str, _value: &Value) -> Option<Image> {
            Some(Image { key: "tall".into(), bytes: Arc::from(&[0_u8][..]), extension: "png".into(), width: 1.0, height: 2_147_483_647.0 })
        }
    }
    let parts = document(&cell("{%photo}")).into_iter().map(|(name, xml)| (name.to_string(), xml.into_bytes()));
    let template = PackageTemplate::parse(parts, &FillOptions::default()).unwrap();
    let filled = template.render_with(&json!({ "photo": "x" }), &mut Tall);
    let xml = String::from_utf8_lossy(&filled.iter().find(|(name, _)| name == "word/document.xml").unwrap().1).into_owned();

    for (cx, cy) in extents(&xml) {
        assert!(cx >= 0 && cy >= 0 && cy <= (i32::MAX as f64 / EMU_PER_PX).round() as i64, "{cx}x{cy}");
    }
}

#[test]
fn numbers_drawings_after_the_largest_drawing_id() {
    let body = r#"<w:p><w:r><w:drawing><wp:inline><wp:docPr id="4294967295" name="logo"/></wp:inline></w:drawing></w:r></w:p>"#.to_string() + &paragraph("{%a}");

    let filled = fill(document(&body), json!({ "a": "photo:a" }));

    let xml = part(&filled, "word/document.xml").unwrap();
    assert_eq!(xml.matches("<wp:docPr id=\"4294967295\"").count(), 1);
}

#[test]
fn draws_a_positive_size_when_margins_exceed_the_page() {
    let body = paragraph("{%a}");
    let mut parts = document(&body);
    parts[1].1 = parts[1].1.replace(r#"w:right="1440""#, r#"w:right="20000""#);

    let filled = fill(parts, json!({ "a": "photo:a" }));

    assert!(extents(part(&filled, "word/document.xml").unwrap()).iter().all(|(cx, cy)| *cx > 0 && *cy > 0));
}

#[test]
fn draws_a_visible_image_in_a_cell_of_zero_width() {
    let body = cell("{%a}").replace(&format!(r#"w:w="{CELL_TWIPS}""#), r#"w:w="0""#);

    let filled = fill(document(&body), json!({ "a": "photo:a" }));

    assert_eq!(extents(part(&filled, "word/document.xml").unwrap()), [(572, 286)]);
}
