mod common;

use common::{fill, paragraph, paragraph_texts};
use serde_json::json;

const TAG: &str = "<w:p><w:r><w:t>{~~html}</w:t></w:r></w:p>";

#[test]
fn collapses_inline_spaces() {
    let xml = fill(TAG, json!({ "html": "<p>  A \n <b> B\t </b> \r\n C  </p>" }));

    assert_eq!(paragraph_texts(&xml), ["A B C"]);
}

#[test]
fn keeps_nonbreaking_spaces() {
    let xml = fill(TAG, json!({ "html": "<p>&nbsp;A&nbsp;&nbsp;B&nbsp;</p>" }));

    assert_eq!(paragraph_texts(&xml), ["\u{a0}A\u{a0}\u{a0}B\u{a0}"]);
}

#[test]
fn trims_break_spaces() {
    let xml = fill(TAG, json!({ "html": "<p>A \t<br> \nB</p>" }));

    assert_eq!(paragraph_texts(&xml), ["AB"]);
    assert!(xml.contains("<w:br/>"));
}

#[test]
fn preserves_preformatted_spaces() {
    let xml = fill(TAG, json!({ "html": "<pre>  A  B  \n C </pre>" }));

    assert_eq!(paragraph_texts(&xml), ["  A  B   C "]);
    assert!(xml.contains("<w:br/>"));
}

#[test]
fn trims_blocks_and_cells() {
    let xml = fill(TAG, json!({ "html": "<p> A </p><table><tr><td> B </td><td> C </td></tr></table><p> D </p>" }));

    assert_eq!(paragraph_texts(&xml), ["A", "B", "C", "D"]);
}

#[test]
fn keeps_inline_edge_spaces() {
    let xml = fill(&paragraph(&["before{~html}after"]), json!({ "html": " \t<b>middle</b>\n " }));

    assert_eq!(paragraph_texts(&xml), ["before middle after"]);
    assert!(xml.contains("<w:b/>") && xml.contains("<w:bCs/>"));
}

#[test]
fn separates_adjacent_values() {
    let data = json!({ "a": " A ", "b": " <b>B</b> ", "c": " C " });
    for runs in [["start{~a}{~b}{~c}end"].as_slice(), ["start", "{~a}", "{~b}", "{~c}", "end"].as_slice()] {
        let xml = fill(&paragraph(runs), data.clone());

        assert_eq!(paragraph_texts(&xml), ["start A B C end"]);
    }
}

#[test]
fn keeps_space_only_separator() {
    let xml = fill(&paragraph(&["A{~html}B"]), json!({ "html": " \t\n " }));

    assert_eq!(paragraph_texts(&xml), ["A B"]);
}

#[test]
fn keeps_template_spacing() {
    let xml = fill(&paragraph(&["A  {~html}  B"]), json!({ "html": " <b>word</b> " }));

    assert_eq!(paragraph_texts(&xml), ["A  word  B"]);
}

#[test]
fn trims_inline_line_edges() {
    let body = "<w:p><w:r><w:t>A</w:t><w:br/></w:r><w:r><w:t>{~html}</w:t></w:r><w:r><w:br/><w:t>B</w:t></w:r></w:p>";
    let xml = fill(body, json!({ "html": " word " }));

    assert_eq!(paragraph_texts(&xml), ["AwordB"]);
    assert_eq!(xml.matches("<w:br/>").count(), 2);
    assert_eq!(paragraph_texts(&fill(&paragraph(&["{~html}"]), json!({ "html": " word " }))), ["word"]);
}

#[test]
fn keeps_spaces_next_to_drawings() {
    let body = "<w:p><w:r><w:drawing/></w:r><w:r><w:t>{~html}</w:t></w:r><w:r><w:drawing/></w:r></w:p>";
    let xml = fill(body, json!({ "html": " word " }));

    assert_eq!(paragraph_texts(&xml), [" word "]);
}

#[test]
fn ignores_empty_inline_neighbors() {
    let data = json!({ "empty": "<b></b>", "blank": "", "text": " A " });
    let xml = fill(&paragraph(&["{~empty}{~text}{~blank}"]), data);

    assert_eq!(paragraph_texts(&xml), ["A"]);
}
