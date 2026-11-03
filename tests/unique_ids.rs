//! Ids that a loop copies are renumbered, so each stays unique in the part.

mod common;

use std::collections::BTreeSet;

use common::{fill, fill_with, paragraph};
use docx_fill::FillOptions;
use serde_json::json;

fn values<'a>(xml: &'a str, attribute: &str) -> Vec<&'a str> {
    xml.split(&format!(" {attribute}=\"")).skip(1).map(|rest| &rest[..rest.find('"').unwrap()]).collect()
}

/// The `w:id` of each `element` in order.
fn element_ids<'a>(xml: &'a str, element: &str) -> Vec<&'a str> {
    xml.split(&format!("<{element}")).skip(1).flat_map(|rest| values(&rest[..rest.find("/>").unwrap()], "w:id")).collect()
}

fn are_unique(values: &[&str]) -> bool {
    values.iter().collect::<BTreeSet<_>>().len() == values.len()
}

fn three_items() -> serde_json::Value {
    json!({ "l": [{}, {}, {}] })
}

#[test]
fn renumbers_a_picture_in_a_loop() {
    let picture = r#"<w:p><w:r><w:drawing><wp:inline><wp:docPr id="7" name="logo"/></wp:inline></w:drawing></w:r></w:p>"#;
    let body = fill(&(paragraph(&["{#l}"]) + picture + &paragraph(&["{/l}"])), three_items());
    assert_eq!(values(&body, "id"), ["7", "8", "9"]);
}

#[test]
fn renumbers_a_bookmark_in_a_loop_and_keeps_its_end_paired() {
    let marked = r#"<w:p><w:bookmarkStart w:id="3" w:name="total"/><w:r><w:t>x</w:t></w:r><w:bookmarkEnd w:id="3"/></w:p>"#;
    let body = fill(&(paragraph(&["{#l}"]) + marked + &paragraph(&["{/l}"])), three_items());
    let starts = element_ids(&body, "w:bookmarkStart");
    assert_eq!(starts, ["3", "4", "5"]);
    assert_eq!(element_ids(&body, "w:bookmarkEnd"), starts);
    assert_eq!(values(&body, "w:name"), ["total", "total_2", "total_3"]);
}

#[test]
fn renumbers_a_repeated_paragraph_id() {
    let tagged = r#"<w:p w14:paraId="0000000A" w14:textId="77777777"><w:r><w:t>{x}</w:t></w:r></w:p>"#;
    let body = fill(&(paragraph(&["{#l}"]) + tagged + &paragraph(&["{/l}"])), three_items());
    let ids = values(&body, "w14:paraId");
    assert_eq!(ids.len(), 3);
    assert!(are_unique(&ids), "{ids:?}");
    assert_eq!(ids[0], "0000000A");
}

#[test]
fn leaves_unrepeated_ids_alone() {
    let body = r#"<w:p w14:paraId="00000001"><w:bookmarkStart w:id="0" w:name="a"/><w:r><w:t>x</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>"#;
    assert_eq!(fill(body, json!({})), body);
}

#[test]
fn reserves_all_revision_ids() {
    let changed = r#"<w:p><w:ins w:id="2" w:author="Editor"><w:r><w:rPr><w:rPrChange w:id="0" w:author="Editor"><w:rPr/></w:rPrChange></w:rPr><w:t>x</w:t></w:r></w:ins></w:p>"#;
    let removed = r#"<w:p><w:del w:id="1" w:author="Editor"><w:r><w:delText>old</w:delText></w:r></w:del></w:p>"#;
    let body = fill(&(paragraph(&["{#l}"]) + changed + &paragraph(&["{/l}"]) + removed), three_items());
    let ids = values(&body, "w:id");

    assert_eq!(ids.len(), 7);
    assert!(are_unique(&ids), "{ids:?}");
    assert_eq!(&ids[..2], ["2", "0"]);
    assert_eq!(ids.last(), Some(&"1"));
    assert_eq!(body.matches("w:author=\"Editor\"").count(), 7);
}

#[test]
fn handles_large_revision_ids() {
    for id in ["-1", "2147483647"] {
        let changed = format!("<w:p><w:ins w:id=\"{id}\" w:author=\"Editor\"><w:r><w:t>x</w:t></w:r></w:ins></w:p>");
        let body = fill(&(paragraph(&["{#l}"]) + &changed + &paragraph(&["{/l}"])), three_items());
        let ids = values(&body, "w:id");
        assert_eq!(ids.len(), 3);
        assert_eq!(ids[0], id);
        assert!(are_unique(&ids));
    }
}

#[test]
fn keeps_the_names_of_continued_bookmarks() {
    let options = FillOptions { continued_bookmarks: Some("tag_".into()), ..FillOptions::default() };
    let marked = r#"<w:p><w:bookmarkStart w:id="3" w:name="tag_0"/><w:bookmarkEnd w:id="3"/><w:r><w:t>x</w:t></w:r></w:p>"#;
    let body = fill_with(&(paragraph(&["{#l}"]) + marked + &paragraph(&["{/l}"])), three_items(), &options);
    assert_eq!(values(&body, "w:name"), ["tag_0", "tag_0", "tag_0"]);
    assert_eq!(element_ids(&body, "w:bookmarkStart"), ["3", "4", "5"]);
}
