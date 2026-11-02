mod common;

use common::{elements, fill_with};
use docx_fill::{Extensions, FillOptions};
use serde_json::json;

const TAG: &str = "<w:p><w:r><w:rPr><w:b w:val=\"0\"/><w:bCs w:val=\"0\"/><w:i w:val=\"0\"/><w:iCs w:val=\"0\"/></w:rPr><w:t>{~~html}</w:t></w:r></w:p>";

#[test]
fn html_emphasis_covers_arabic() {
    for extras in [false, true] {
        let options = FillOptions { extensions: Extensions { html_extras: extras, ..Extensions::ALL }, ..FillOptions::default() };
        let xml = fill_with(TAG, json!({ "html": "<b>عريض</b><i>مائل</i>" }), &options);
        let runs = elements(&xml, "w:r");

        assert!(runs[0].contains("<w:b/>") && runs[0].contains("<w:bCs/>"));
        assert!(runs[1].contains("<w:i/>") && runs[1].contains("<w:iCs/>"));
    }
}

#[test]
fn resets_script_emphasis() {
    let template = TAG.replace("val=\"0\"", "val=\"1\"");
    let xml = fill_with(&template, json!({ "html": "<b><span style=\"font-weight:normal\">عادي</span></b>" }), &FillOptions::default());
    let runs = elements(&xml, "w:r");

    assert!(runs[0].contains("<w:b w:val=\"0\"/>") && runs[0].contains("<w:bCs w:val=\"0\"/>"));
}
