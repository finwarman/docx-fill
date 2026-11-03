//! Mirrors `apps/web/src/components/pdf/wordcraft/__tests__/richText.test.ts` in Scope's
//! frontend: `{~~tag}` prints HTML as Word paragraphs and runs.

mod common;

use common::{elements, fill, text};
use serde_json::json;

const BASE_PPR: &str = r#"<w:pPr><w:pStyle w:val="Body"/></w:pPr>"#;
const BASE_RPR: &str = r#"<w:rPr><w:rFonts w:ascii="Arial"/></w:rPr>"#;

fn tag_paragraph(tag: &str, before: &str) -> String {
    format!(
        r#"<w:p>{BASE_PPR}<w:bookmarkStart w:id="1" w:name="scope_tag_0"/><w:bookmarkEnd w:id="1"/><w:r>{BASE_RPR}<w:t xml:space="preserve">{before}{{{tag}}}</w:t></w:r></w:p>"#
    )
}

fn fill_html(html: &str, before: &str) -> String {
    fill(&tag_paragraph("~~html:a.value", before), json!({ "html:a.value": html }))
}

fn paragraphs(xml: &str) -> Vec<&str> {
    elements(xml, "w:p")
}

fn texts(xml: &str) -> Vec<String> {
    paragraphs(xml).into_iter().map(text).collect()
}

#[test]
fn prints_each_html_paragraph_as_a_word_paragraph_with_the_tags_formatting() {
    let body = fill_html("<p>first</p><p>second &amp; more</p>", "");

    assert_eq!(texts(&body), ["first", "second &amp; more"]);
    for paragraph in paragraphs(&body) {
        assert!(paragraph.contains(r#"<w:pStyle w:val="Body"/>"#));
        assert!(paragraph.contains(r#"<w:rFonts w:ascii="Arial"/>"#));
    }
}

#[test]
fn keeps_the_tags_bookmark_on_the_first_paragraph_only() {
    let body = fill_html("<p>first</p><p>second</p>", "");

    let printed = paragraphs(&body);
    assert!(printed[0].contains(r#"w:name="scope_tag_0""#));
    assert!(!printed[1].contains("bookmark"));
}

#[test]
fn marks_bold_italic_and_underlined_text_on_its_runs() {
    let body = fill_html("<p>plain <strong>bold <em>both</em></strong> <u>under</u></p>", "");

    let runs = elements(&body, "w:r");
    let run_for = |value: &str| runs.iter().find(|run| text(run) == value).copied().unwrap_or_default();
    assert!(!run_for("plain ").contains("<w:b/>") && !run_for("plain ").contains("<w:i/>") && !run_for("plain ").contains("<w:u "));
    assert!(run_for("bold ").contains("<w:b/>"));
    assert!(run_for("both").contains("<w:b/>") && run_for("both").contains("<w:i/>"));
    assert!(run_for("under").contains(r#"<w:u w:val="single"/>"#));
}

#[test]
fn prints_list_items_as_indented_paragraphs_with_their_bullet_or_number() {
    let body = fill_html("<ul><li>one<ol><li>nested</li><li>again</li></ol></li><li>two</li></ul>", "");

    let printed = paragraphs(&body);
    assert_eq!(texts(&body), ["\u{2022}one", "1.nested", "2.again", "\u{2022}two"]);
    assert!(printed[0].contains(r#"<w:ind w:left="360" w:hanging="360"/>"#));
    assert!(printed[1].contains(r#"<w:ind w:left="720" w:hanging="360"/>"#));
    assert!(printed[0].contains("<w:tab/>"));
}

#[test]
fn prints_headings_bold_and_larger() {
    let body = fill_html("<h1>Title</h1><p>body</p>", "");

    let printed = paragraphs(&body);
    assert!(printed[0].contains("<w:b/>"));
    assert!(printed[0].contains(r#"<w:sz w:val="32"/>"#));
    assert!(!printed[1].contains("<w:b/>"));
}

#[test]
fn keeps_a_blank_line_for_an_empty_paragraph_and_a_break_for_br() {
    let body = fill_html("<p>one<br>two</p><br><p>three</p>", "");

    assert_eq!(texts(&body), ["onetwo", "", "three"]);
    assert!(paragraphs(&body)[0].contains("<w:br/>"));
}

#[test]
fn keeps_the_tags_paragraph_and_bookmark_when_the_value_is_empty() {
    let body = fill_html("", "");

    let printed = paragraphs(&body);
    assert_eq!(printed.len(), 1);
    assert!(printed[0].contains(r#"w:name="scope_tag_0""#));
    assert_eq!(text(printed[0]), "");
}

#[test]
fn prints_inline_joined_by_breaks_when_the_paragraph_has_other_text() {
    let body = fill_html("<p>one</p><p>two</p>", "Notes: ");

    let printed = paragraphs(&body);
    assert_eq!(printed.len(), 1);
    assert_eq!(text(printed[0]), "Notes: onetwo");
    assert!(printed[0].contains("<w:br/>"));
}

#[test]
fn fills_every_repeat_of_a_tag_inside_a_loop() {
    let template = format!("<w:p><w:r><w:t>{{#rows}}</w:t></w:r></w:p>{}<w:p><w:r><w:t>{{/rows}}</w:t></w:r></w:p>", tag_paragraph("~~note", ""));

    let body = fill(&template, json!({ "rows": [{ "note": "<p>a</p>" }, { "note": "<p>b</p>" }] }));

    assert_eq!(texts(&body), ["a", "b"]);
}

#[test]
fn prints_an_inline_tag_in_its_paragraph() {
    let body = fill(&tag_paragraph("~html", ""), json!({ "html": "<p>one</p><p>two</p>" }));

    assert_eq!(texts(&body), ["onetwo"]);
}

#[test]
fn prints_rich_text_in_a_text_box_paragraph() {
    let template = format!("<w:p><w:r><w:t>anchor</w:t></w:r><w:r><w:drawing><w:txbxContent>{}</w:txbxContent></w:drawing></w:r></w:p>", tag_paragraph("~~html", ""));

    let body = fill(&template, json!({ "html": "<p>a</p><p>b</p>" }));

    assert_eq!(text(&body), "anchorab");
    assert_eq!(body.matches("<w:pStyle").count(), 2);
}

#[test]
fn numbers_the_tag_bookmark_on_later_paragraphs() {
    let options = docx_fill::FillOptions { continued_bookmarks: Some("scope_tag_".to_string()), ..docx_fill::FillOptions::default() };
    let template = format!(r#"{}<w:p><w:bookmarkStart w:id="7" w:name="other"/><w:bookmarkEnd w:id="7"/></w:p>"#, tag_paragraph("~~html:a.value", ""));
    let xml = docx_fill::fill_part(&template, &json!({ "html:a.value": "<p>a</p><p>b</p><p>c</p>" }), &options).unwrap();
    let paragraphs = paragraphs(&xml);

    assert!(paragraphs[0].contains(r#"w:name="scope_tag_0""#));
    assert!(paragraphs[1].contains(r#"<w:bookmarkStart w:id="8" w:name="scope_tag_0_1"/><w:bookmarkEnd w:id="8"/>"#), "{xml}");
    assert!(paragraphs[2].contains(r#"<w:bookmarkStart w:id="9" w:name="scope_tag_0_2"/>"#));

    let xml = fill_html("<p>a</p><p>b</p>", "");
    assert!(!xml.contains("scope_tag_0_1"));
}

#[test]
fn keeps_run_properties_in_schema_order() {
    let tag = r#"<w:p><w:r><w:rPr><w:lang w:val="en-GB"/></w:rPr><w:t>{~~html}</w:t></w:r></w:p>"#;
    let heading = fill(tag, json!({ "html": "<h1>T</h1>" }));
    let mut options = docx_fill::FillOptions::default();
    options.extensions.html_extras = false;
    let underline = common::fill_with(tag, json!({ "html": "<p><u>U</u></p>" }), &options);

    assert!(heading.contains(r#"<w:sz w:val="32"/><w:szCs w:val="32"/><w:lang w:val="en-GB"/>"#), "{heading}");
    assert!(underline.contains(r#"<w:u w:val="single"/><w:lang w:val="en-GB"/>"#), "{underline}");
}

#[test]
fn keeps_tracked_property_changes_whole() {
    let ppr = r#"<w:pPr><w:pStyle w:val="Body"/><w:pPrChange w:id="8" w:author="E"><w:pPr><w:jc w:val="center"/></w:pPr></w:pPrChange></w:pPr>"#;
    let rpr = r#"<w:rPr><w:b/><w:rPrChange w:id="9" w:author="E"><w:rPr><w:i/></w:rPr></w:rPrChange></w:rPr>"#;
    for (before, tags) in [("", "{~~html:a.value}"), ("x ", "{~~html:a.value}"), ("", "{~~html:a.value} {~~html:b.value}")] {
        let body = format!(r#"<w:p>{ppr}<w:r>{rpr}<w:t xml:space="preserve">{before}{tags}</w:t></w:r></w:p>"#);
        for html in ["<ul><li>one</li></ul>", "<p>plain</p>"] {
            let xml = fill(&body, json!({ "html:a.value": html, "html:b.value": html }));
            for name in ["w:pPr", "w:rPr", "w:pPrChange", "w:rPrChange"] {
                assert_eq!(xml.matches(&format!("<{name}>")).count() + xml.matches(&format!("<{name} ")).count(), xml.matches(&format!("</{name}>")).count(), "{name} in {xml}");
            }
        }
    }
}
