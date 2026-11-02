//! Delimiters other than `{ }`, as a signatures pass uses `[[ ]]`.

mod common;

use common::{document, paragraph, text};
use docx_fill::{Delimiters, FillOptions, fill_part};
use serde_json::json;

fn fill_with(body: &str, start: &str, end: &str) -> Result<String, String> {
    let options = FillOptions { delimiters: Delimiters { start: start.into(), end: end.into() }, ..FillOptions::default() };
    fill_part(&document(body), &json!({ "name": "N", "rows": [{ "x": 1 }, { "x": 2 }] }), &options).map(|xml| text(&xml)).map_err(|error| error.message)
}

#[test]
fn fills_tags_between_two_character_delimiters() {
    assert_eq!(fill_with(&paragraph(&["[[name]] [[#rows]][[x]][[/rows]]"]), "[[", "]]").unwrap(), "N 12");
}

#[test]
fn leaves_the_other_delimiters_as_text() {
    assert_eq!(fill_with(&paragraph(&["{name} [ x ] [[na", "me]]"]), "[[", "]]").unwrap(), "{name} [ x ] N");
}

#[test]
fn rejects_an_unclosed_tag() {
    assert_eq!(fill_with(&paragraph(&["[[name"]), "[[", "]]").unwrap_err(), r#"The tag beginning with "[[name" is unclosed"#);
}

#[test]
fn fills_tags_between_doubled_braces() {
    assert_eq!(fill_with(&paragraph(&["{{name}}"]), "{{", "}}").unwrap(), "N");
}

#[test]
fn fills_tags_when_start_and_end_delimiter_are_the_same() {
    assert_eq!(fill_with(&paragraph(&["%name%"]), "%", "%").unwrap(), "N");
}

#[test]
fn fills_several_same_delimiter_tags_in_one_paragraph() {
    let options = FillOptions { delimiters: Delimiters { start: "%".into(), end: "%".into() }, ..FillOptions::default() };
    let xml = fill_part(&document(&paragraph(&["%a% and %b%"])), &json!({ "a": "one", "b": "two" }), &options)
        .map(|xml| text(&xml))
        .unwrap();
    assert_eq!(xml, "one and two");
}

#[test]
fn fills_tags_between_delimiters_xml_escapes() {
    for (start, end) in [("<<", ">>"), ("<%", "%>"), ("&", "&")] {
        let written = format!("{start}name{end}").replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        assert_eq!(fill_with(&paragraph(&[&written]), start, end).unwrap(), "N", "{start}name{end}");
    }
}

#[test]
fn fills_a_greater_than_delimiter_written_raw() {
    assert_eq!(fill_with(&paragraph(&["&lt;&lt;name>>"]), "<<", ">>").unwrap(), "N");
}

#[test]
fn names_a_doubled_open_delimiter() {
    assert_eq!(fill_with(&paragraph(&["x {{name} y"]), "{", "}").unwrap_err(), r#"The tag beginning with "{{name" has duplicate open tags"#);
}

#[test]
fn names_a_doubled_close_delimiter() {
    assert_eq!(fill_with(&paragraph(&["x {name}} y"]), "{", "}").unwrap_err(), r#"The tag ending with "name}}" has duplicate close tags"#);
}

#[test]
fn keeps_unclosed_and_unopened_apart_from_doubled_delimiters() {
    assert_eq!(fill_with(&paragraph(&["{a {b}"]), "{", "}").unwrap_err(), r#"The tag beginning with "{a " is unclosed"#);
    assert_eq!(fill_with(&paragraph(&["{a} b}"]), "{", "}").unwrap_err(), r#"The tag beginning with " b" is unopened"#);
}

#[test]
fn keeps_escaped_text_apart_from_an_ampersand_delimiter() {
    assert_eq!(fill_with(&paragraph(&["a &lt; b &amp;name&amp;"]), "&", "&").unwrap(), "a &lt; b N");
}

#[test]
fn names_a_doubled_open_delimiter_before_non_ascii_text() {
    for (template, context) in [("{{prénom}", "{{prénom"), ("é{{日本}", "{{日本"), ("{{🎉", "{{🎉")] {
        let error = fill_part(&document(&paragraph(&[template])), &json!({}), &FillOptions::core()).unwrap_err();

        assert_eq!(error.message, format!("The tag beginning with \"{context}\" has duplicate open tags"));
    }
}

fn fill_set(body: &str, options: &FillOptions) -> Result<String, String> {
    fill_part(&document(body), &json!({ "name": "V" }), options).map(|xml| text(&xml)).map_err(|error| error.message)
}

#[test]
fn sets_new_delimiters_from_a_set_delimiter_tag() {
    let options = FillOptions::default();
    let cases = [
        ("{=[[ ]]=}[[name]]", "V"),
        ("{=[[ ]]=}[[name]][[={ }=]]{name}", "VV"),
        ("{=&lt;% %&gt;=}&lt;%name%&gt;", "V"),
        ("{=[[ ]]=}{name}[[name]]", "{name}V"),
    ];
    for (template, printed) in cases {
        assert_eq!(fill_set(&paragraph(&[template]), &options).unwrap(), printed, "{template}");
    }
}

#[test]
fn keeps_set_delimiters_into_later_paragraphs() {
    let body = paragraph(&["{=[[ ]]=}"]) + &paragraph(&["[[name]]"]);
    assert_eq!(fill_set(&body, &FillOptions::default()).unwrap(), "V");
}

#[test]
fn refuses_a_set_delimiter_tag_it_cannot_read() {
    for template in ["{=[[]]=}", "{=[[ ]] x=}", "{=[[ ]]}", "{= [[ ]] =}", "{=a=b c=}"] {
        let error = fill_set(&paragraph(&[template]), &FillOptions::default()).unwrap_err();
        assert!(error.starts_with("New delimiters cannot be parsed"), "{template}: {error}");
    }
}

#[test]
fn reads_set_delimiter_tags_with_another_prefix_or_none() {
    let dollar = FillOptions { change_delimiter_prefix: Some("$".into()), ..FillOptions::default() };
    assert_eq!(fill_set(&paragraph(&["{$[[ ]]$}[[name]]"]), &dollar).unwrap(), "V");

    let off = FillOptions { change_delimiter_prefix: None, ..FillOptions::default() };
    assert_eq!(fill_set(&paragraph(&["{=[[ ]]=}[[name]]"]), &off).unwrap(), "[[name]]");
}

#[test]
fn prints_stray_delimiters_as_text_when_allowed() {
    let both = FillOptions { allow_unopened_tag: true, allow_unclosed_tag: true, ..FillOptions::default() };
    let unopened = FillOptions { allow_unopened_tag: true, ..FillOptions::default() };
    let unclosed = FillOptions { allow_unclosed_tag: true, ..FillOptions::default() };
    let cases = [
        ("a} {name}", &unopened, "a} V"),
        ("{name}} x", &unopened, "V} x"),
        ("{a {name}", &unclosed, "{a V"),
        ("x {a", &unclosed, "x {a"),
        ("{{name} x", &unclosed, "{V x"),
        ("} {name} {", &both, "} V {"),
        ("{a {name} c}", &both, "{a V c}"),
    ];
    for (template, options, printed) in cases {
        assert_eq!(fill_set(&paragraph(&[template]), options).unwrap(), printed, "{template}");
    }
}

#[test]
fn keeps_stray_delimiters_errors_by_default() {
    assert!(fill_set(&paragraph(&["a} {name}"]), &FillOptions { allow_unclosed_tag: true, ..FillOptions::default() }).is_err());
    assert!(fill_set(&paragraph(&["x {a"]), &FillOptions { allow_unopened_tag: true, ..FillOptions::default() }).is_err());
}

#[test]
fn refuses_a_delimiter_overlapping_another() {
    let options = FillOptions { delimiters: Delimiters { start: "[[".into(), end: "]]".into() }, ..FillOptions::default() };
    let error = |text: &str| fill_part(&document(&paragraph(&[text])), &json!({ "m": "M" }), &options).unwrap_err().message;

    assert_eq!(error("x[[[m]]"), "The tag beginning with \"[[[m]]\" is unclosed");
    assert_eq!(error("[[m]]]"), "The tag ending with \"m]]]\" has duplicate close tags");
}

#[test]
fn reads_overlapping_delimiters_as_text_when_allowed() {
    let options = FillOptions { delimiters: Delimiters { start: "[[".into(), end: "]]".into() }, allow_unclosed_tag: true, allow_unopened_tag: true, ..FillOptions::default() };

    let fill = |template: &str| text(&fill_part(&document(&paragraph(&[template])), &json!({ "m": "M", "[m": "B" }), &options).unwrap());

    assert_eq!(fill("[[[m]]]"), "[M]");
    assert_eq!(fill("x[[[m]]"), "x[M");
    assert_eq!(fill("[[m]]]"), "M]");
}
