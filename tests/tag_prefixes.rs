mod common;

use common::{document, paragraph, text};
use docx_fill::{FillOptions, TagType, Template, fill_part};
use serde_json::json;

fn options() -> FillOptions {
    FillOptions::default().read_json(&json!({"tagPrefixes":{
        "loopStart":"if ", "loopEnd":"endif", "invertedLoop":"unless ", "raw":"raw ",
        "image":"img ", "centeredImage":"centerImg ", "html":"html ", "blockHtml":"blockHtml ", "wordRun":"wordRun "
    }})).unwrap()
}

#[test]
fn reads_custom_loop_prefixes() {
    let xml = document(&paragraph(&["{if rows}{name}{endif}{unless absent}empty{endif}"]));
    let output = fill_part(&xml, &json!({"rows":[{"name":"A"},{"name":"B"}]}), &options()).unwrap();

    assert_eq!(text(&output), "ABempty");
}

#[test]
fn reads_custom_module_prefixes() {
    let xml = document(&format!("{}{}{}{}{}", paragraph(&["{img photo}"]), paragraph(&["{centerImg photo}"]), paragraph(&["{html note}"]), paragraph(&["{blockHtml note}"]), paragraph(&["{wordRun xml}"])));
    let tags = Template::parse(&xml, &options()).unwrap().tags();

    assert_eq!(tags.iter().map(|tag| tag.kind).collect::<Vec<_>>(), [TagType::Image, TagType::Image, TagType::RichText, TagType::RichText, TagType::RawRun]);
    assert_eq!(tags.iter().map(|tag| tag.name.as_str()).collect::<Vec<_>>(), ["photo", "photo", "note", "note", "xml"]);
}

#[test]
fn uses_longest_prefix() {
    let mut options = options();
    options.tag_prefixes.html = "i".into();
    let tags = Template::parse(&document(&paragraph(&["{img photo}"])), &options).unwrap().tags();

    assert_eq!(tags[0].kind, TagType::Image);
}

#[test]
fn disables_replaced_prefixes() {
    let output = fill_part(&document(&paragraph(&["{#value}|{%photo}|{raw xml}"])), &json!({"#value":"literal", "%photo":"name", "xml":"<w:p/>"}), &options());

    // A raw tag still has to occupy its own paragraph.
    assert!(output.is_err());
    let output = fill_part(&document(&paragraph(&["{#value}|{%photo}"])), &json!({"#value":"literal", "%photo":"name"}), &options()).unwrap();
    assert_eq!(text(&output), "literal|name");
}

#[test]
fn rejects_ambiguous_prefixes() {
    for value in [json!({"tagPrefixes":{"loopStart":""}}), json!({"tagPrefixes":{"html":"%"}})] {
        assert!(FillOptions::default().read_json(&value).is_err());
    }
}

#[test]
fn allows_unprefixed_html() {
    let options = FillOptions::default().read_json(&json!({"tagPrefixes":{"html":""}})).unwrap();
    let output = fill_part(&document(&paragraph(&["{value}"])), &json!({"value":"<b>word</b>"}), &options).unwrap();

    assert_eq!(text(&output), "word");
    assert!(output.contains("<w:b/>"));
}

#[test]
fn keeps_element_loops_with_unprefixed_html() {
    let options = FillOptions::default().read_json(&json!({"tagPrefixes":{"html":""}})).unwrap();
    let row = "<w:tr><w:tc><w:p><w:r><w:t>{-w:tr rows}{value}{/rows}</w:t></w:r></w:p></w:tc></w:tr>";
    let output = fill_part(&document(&format!("<w:tbl>{row}</w:tbl>")), &json!({"rows":[{"value":"<b>A</b>"},{"value":"B"}]}), &options).unwrap();

    assert_eq!(output.matches("<w:tr>").count(), 2);
    assert_eq!(text(&output), "AB");
}
