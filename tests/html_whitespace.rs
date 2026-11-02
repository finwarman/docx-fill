mod common;

use common::{fill, paragraph_texts};
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
