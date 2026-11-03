mod common;

use common::{document, paragraph, paragraph_texts, text};
use docx_fill::{FillOptions, PackageTemplate, fill_part};
use serde_json::json;

fn fill(body: &str, data: serde_json::Value) -> String {
    fill_part(&document(body), &data, &FillOptions { expressions: true, ..FillOptions::default() }).unwrap()
}

#[test]
fn stores_values_without_printing() {
    let xml = fill(&paragraph(&["{full = a + b}{full}|{x=y=2}{x}{y}"]), json!({"a":2,"b":3}));

    assert_eq!(text(&xml), "5|22");
}

#[test]
fn returns_last_statement() {
    let xml = fill(&paragraph(&["{a=1;a}|{a;2}|{;}|{1;a=2}{a}|{1+(a=1)}"]), json!({}));

    assert_eq!(text(&xml), "1|2||2|2");
}

#[test]
fn writes_to_current_loop_item() {
    let data = json!({"value":9,"rows":[{"n":2},{"n":4}]});
    let xml = fill(&paragraph(&["{#rows}{value=n+1}{value},{/rows}|{value}"]), data.clone());

    assert_eq!(text(&xml), "3,5,|9");
    assert_eq!(data["value"], 9);
    assert!(data["rows"][0].get("value").is_none());
}

#[test]
fn supports_nested_targets() {
    let xml = fill(&paragraph(&["{a.b=2}{a.b}|{items[1]=4}{items[1]}|{a.c.d=5}{a.c.d}"]), json!({"a":{"b":1},"items":[0,0]}));

    assert_eq!(text(&xml), "2|4|5");
}

#[test]
fn raw_assignment_drops_paragraph() {
    let body = paragraph(&["{@full=a}"]) + &paragraph(&["{full}"]);
    let xml = fill(&body, json!({"a":"value"}));

    assert_eq!(paragraph_texts(&xml), ["value"]);
}

#[test]
fn assignments_share_package_scope() {
    let options = FillOptions { expressions: true, ..FillOptions::default() };
    let parts = [
        ("word/document.xml".into(), document(&paragraph(&["{name='filled'}"])).into_bytes()),
        ("word/header1.xml".into(), b"<w:hdr xmlns:w=\"w\"><w:p><w:r><w:t>{name}</w:t></w:r></w:p></w:hdr>".to_vec()),
    ];
    let template = PackageTemplate::parse(parts, &options).unwrap();
    let parts = template.render(&json!({}));
    let header = parts.iter().find(|(name, _)| name == "word/header1.xml").unwrap();

    assert_eq!(text(&String::from_utf8_lossy(&header.1)), "filled");
}

#[test]
fn reads_loop_item_assignments_through_paths() {
    let xml = fill(&paragraph(&["{#rows}{c=n}{/rows}|{rows[0].c}|{c}"]), json!({"rows":[{"n":1}]}));

    assert_eq!(text(&xml), "|1|");
}

#[test]
fn keeps_assignments_from_loops_over_plain_values() {
    let xml = fill(&paragraph(&["{#vals}{t=1}{t}{/vals}|{t}|{#flag}{u=2}{/flag}{u}"]), json!({"vals":[1,2],"flag":true}));

    assert_eq!(text(&xml), "11|1|2");
}

struct ObjectHooks;

impl docx_fill::FillHooks for ObjectHooks {
    fn tag_value(&mut self, name: &str, _scopes: &[&serde_json::Value]) -> Option<serde_json::Value> {
        (name == "obj").then(|| json!({"k":1}))
    }
}

#[test]
fn computed_scope_assignments_end_with_their_part() {
    let options = FillOptions { expressions: true, ..FillOptions::default() };
    let header: String = (0..40).map(|_| "<w:p><w:r><w:t>{#obj}{x}{/obj}</w:t></w:r></w:p>").collect();
    let parts = [
        ("word/document.xml".into(), document(&paragraph(&["{#obj}{x='leak'}{/obj}"])).into_bytes()),
        ("word/header1.xml".into(), format!("<w:hdr xmlns:w=\"w\">{header}</w:hdr>").into_bytes()),
    ];
    let template = PackageTemplate::parse(parts, &options).unwrap();
    let parts = template.render_with(&json!({}), &mut ObjectHooks);
    let header = parts.iter().find(|(name, _)| name == "word/header1.xml").unwrap();

    assert_eq!(text(&String::from_utf8_lossy(&header.1)), "");
}

#[test]
fn placeholder_assignments_run_once() {
    let xml = fill(&paragraph(&["{?n = n + 1; n}"]), json!({"n":1}));

    assert_eq!(text(&xml), "2");
}
