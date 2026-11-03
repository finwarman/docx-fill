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
fn rejects_prefixes_claimed_by_extensions() {
    for prefix in ["$", "$raw ", "?", ":table ", ":footnote ", ":include ", ":stylepar ", ":vt#", ":stylecell ", ":page ", ":merge-cells-col "] {
        let result = FillOptions::default().read_json(&json!({"tagPrefixes":{"raw":prefix}}));
        let error = result.unwrap_err();
        assert!(error.message.contains("raw") && error.message.contains("enabled extension"), "{prefix:?}: {error}");
    }
    let mut options = FillOptions::default();
    options.tag_prefixes.loop_start = "$".into();
    assert!(Template::parse(&document(&paragraph(&["{$rows}{/rows}"])), &options).is_err());
    assert!(docx_fill::PackageTemplate::parse([], &options).is_err());
}

#[test]
fn allows_disabled_extension_prefixes() {
    for (prefix, extension) in [("$", "charts"), ("?", "paragraphPlaceholder"), (":table ", "fullTables"), (":footnote ", "footnotes"), (":include ", "subtemplates"), (":stylepar ", "styling"), (":vt#", "verticalTables"), (":stylecell ", "cellStyling"), (":page ", "sections"), (":merge-cells-col ", "tableMerge")] {
        let options = FillOptions::default().read_json(&json!({"tagPrefixes":{"raw":prefix},"extensions":{extension:false}})).unwrap();
        let tags = Template::parse(&document(&paragraph(&[&format!("{{{prefix}xml}}")])), &options).unwrap().tags();
        assert_eq!(tags[0].kind, TagType::Raw, "{prefix:?}");
        assert_eq!(tags[0].name, "xml");
    }
}

#[test]
fn allows_distinct_shorter_prefixes() {
    let options = FillOptions::default().read_json(&json!({"tagPrefixes":{"raw":":"}})).unwrap();
    let tags = Template::parse(&document(&paragraph(&["{:footnote note}"])), &options).unwrap().tags();
    assert_eq!(tags[0].kind, TagType::Footnote);

    let options = FillOptions::default().read_json(&json!({"tagPrefixes":{"image":"$"},"extensions":{"images":false}})).unwrap();
    assert!(options.read_json(&json!({"extensions":{"images":true}})).is_err());
}

#[test]
fn keeps_custom_module_precedence() {
    let options = FillOptions::default().read_json(&json!({"tagPrefixes":{"raw":"$"},"modulePrefixes":["$"]})).unwrap();
    let tags = Template::parse(&document(&paragraph(&["{$value}"])), &options).unwrap().tags();
    assert_eq!(tags[0].kind, TagType::Module);
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
