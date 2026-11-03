//! Long and deep HTML values must not overflow the stack, which no caller can catch.

mod common;

use common::{elements, fill, paragraph, paragraph_texts, text};
use serde_json::json;

fn html(value: &str) -> String {
    fill(&paragraph(&["{~~v}"]), json!({ "v": value }))
}

#[test]
fn prints_a_table_left_open() {
    assert_eq!(text(&html("<table><tr><td>x")), "x");
    assert_eq!(text(&html("<ul><li><table><td>x")), "•x");
}

#[test]
fn prints_a_long_inline_loop_in_order() {
    const ITEMS: usize = 20_000;
    let items: Vec<_> = (0..ITEMS).map(|index| json!({ "h": format!("{index}<b>,</b>") })).collect();

    let body = fill(&paragraph(&["a{#l}{~h}{/l}z"]), json!({ "l": items }));

    let expected: String = (0..ITEMS).map(|index| format!("{index},")).collect();
    assert_eq!(text(&body), format!("a{expected}z"));
}

#[test]
fn keeps_text_and_formatting_between_inline_values_in_a_run() {
    let template = "<w:p><w:r><w:rPr><w:i/></w:rPr><w:t>a{~x}b{~y}c{~z}d</w:t></w:r></w:p>";

    let body = fill(template, json!({ "x": "1", "y": "<b>2</b>", "z": "3" }));

    assert_eq!(text(&body), "a1b2c3d");
    assert!(elements(&body, "w:r").iter().all(|run| run.contains("<w:i/>")), "{body}");
}

#[test]
fn keeps_inline_values_in_their_own_runs_and_other_runs_as_they_were() {
    let template = "<w:p><w:r><w:t>{~x}</w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>keep</w:t></w:r><w:r><w:rPr><w:i/></w:rPr><w:t>{~y}</w:t></w:r></w:p>";

    let body = fill(template, json!({ "x": "1", "y": "2" }));

    assert_eq!(text(&body), "1keep2");
    assert!(body.contains("<w:rPr><w:b/></w:rPr><w:t>keep</w:t>"), "{body}");
}

#[test]
fn prints_a_block_value_alone_once_the_values_before_it_print_nothing() {
    let body = fill(&paragraph(&["{~~x}{~~y}"]), json!({ "x": "", "y": "<p>a</p><p>b</p>" }));

    assert_eq!(paragraph_texts(&body), ["a", "b"]);
}

#[test]
fn numbers_links_in_the_order_they_print() {
    let body = fill(&paragraph(&["{~x}{~y}{~z}"]), json!({ "x": "<a href=\"http://a.test\">a</a>", "y": "<a href=\"http://b.test\">b</a>", "z": "<a href=\"http://c.test\">c</a>" }));

    let ids: Vec<&str> = body.match_indices("r:id=\"").map(|(at, found)| body[at + found.len()..].split('"').next().unwrap()).collect();
    assert_eq!(ids.len(), 3, "{body}");
    assert_eq!(text(&body), "abc");
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 3, "{ids:?}");
}

#[test]
fn refuses_xml_that_would_copy_a_mark_into_a_runs_properties() {
    let broken = "<w:p><w:r><w:rPr><w:b/></w:x><w:t>a{~h}b</w:t></w:r><w:rPr><w:b/></w:rPr></w:p>";

    let error = docx_fill::fill_part(&common::document(broken), &json!({ "h": "x" }), &docx_fill::FillOptions::default()).unwrap_err();

    assert_eq!(error.message, "The XML is malformed: </w:x> closes <w:rPr>");
}

#[test]
fn does_not_overflow_on_deeply_nested_tables() {
    let open = "<table><td>".repeat(100_000);

    assert_eq!(text(&html(&format!("{open}x"))), "x");
    assert_eq!(text(&fill(&paragraph(&["a{~v}b"]), json!({ "v": format!("{open}x") }))), "axb");
}

#[test]
fn inherits_formatting_through_deep_inline_elements() {
    const DEPTH: usize = 20_000;
    let value = format!("<b>{}z{}</b>after", "<span>x".repeat(DEPTH), "</span>".repeat(DEPTH));

    let body = html(&value);
    assert_eq!(text(&body), format!("{}zafter", "x".repeat(DEPTH)));
    let runs = elements(&body, "w:r");
    assert!(runs[..runs.len() - 1].iter().all(|run| run.contains("<w:b/>")));
    assert!(!runs.last().unwrap().contains("<w:b/>"));
}

#[test]
fn inherits_layout_through_deep_containers() {
    const DEPTH: usize = 10_000;
    let value = format!("<div style=\"margin-left:1pt\">{}z{}</div><p>after</p>", "<div>x".repeat(DEPTH), "</div>".repeat(DEPTH));

    let body = html(&value);
    let paragraphs = elements(&body, "w:p");
    assert_eq!(paragraphs.len(), DEPTH + 1);
    assert!(paragraphs[..DEPTH].iter().all(|paragraph| paragraph.contains("w:left=\"20\"")));
    assert!(!paragraphs.last().unwrap().contains("<w:ind"));
    assert_eq!(text(&body), format!("{}zafter", "x".repeat(DEPTH)));
}

#[test]
fn updates_inheritance_after_crossed_closing_tags() {
    let body = html("<div style=\"margin-left:1pt\"><section style=\"margin-left:2pt\"><b><i>a</b>b</div>c</i></section><p>d</p>");
    let paragraphs = elements(&body, "w:p");
    assert!(paragraphs[0].contains("w:left=\"60\""));
    assert!(paragraphs[1].contains("w:left=\"40\""));
    let runs = elements(&body, "w:r");
    assert!(runs[0].contains("<w:b/>") && runs[0].contains("<w:i/>"));
    assert!(!runs[1].contains("<w:b/>") && runs[1].contains("<w:i/>"));
    assert!(!runs.last().unwrap().contains("<w:i/>"));
    assert_eq!(text(&body), "abcd");
}

#[test]
fn keeps_the_text_after_tables_past_the_nesting_limit() {
    let value = format!("{}x{}after", "<table><td>".repeat(100), "</td></table>".repeat(100));

    assert_eq!(text(&html(&value)), "xafter");
}

#[test]
fn does_not_panic_on_values_holding_the_crates_own_marks() {
    let values = ["\u{E000}9\u{E001}", "\u{E000}", "\u{E001}", "\u{E000}x\u{E001}", "\u{E002}9\u{E003}y\u{E004}", "\u{E005}9\u{E006}", "\u{E007}9\u{E008}"];
    for value in values {
        fill(&paragraph(&["{v}"]), json!({ "v": value }));
        fill(&paragraph(&["{v}{~h}"]), json!({ "v": value, "h": "x" }));
        fill(&paragraph(&["{~~h}"]), json!({ "h": value }));
        fill(&paragraph(&["{v}{:stylestart s}x{:/style}"]), json!({ "v": value, "s": { "bold": true } }));
    }
}
