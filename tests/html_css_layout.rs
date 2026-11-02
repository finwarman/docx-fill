mod common;

use common::{elements, fill, paragraph_texts};
use serde_json::json;

fn render(html: &str) -> String {
    fill("<w:p><w:r><w:t>{~~html}</w:t></w:r></w:p>", json!({"html":html}))
}

#[test]
fn writes_spacing_and_indents() {
    let xml = render("<p style='margin-top:2px;padding-top:3px;margin-bottom:4px;padding-bottom:1px;padding-left:8px;margin-right:5px;padding-right:2px;font-size:20px;text-indent:-1em;line-height:1.5'>A</p>");

    assert!(xml.contains("<w:spacing w:before=\"75\" w:after=\"75\" w:line=\"360\" w:lineRule=\"auto\"/>"));
    assert!(xml.contains("<w:ind w:left=\"120\" w:right=\"105\" w:hanging=\"300\"/>"));
}

#[test]
fn writes_fixed_and_percent_lines() {
    let xml = render("<p style='line-height:20px'>A</p><p style='line-height:120%'>B</p>");

    assert!(xml.contains("w:line=\"300\" w:lineRule=\"exact\""));
    assert!(xml.contains("w:line=\"288\" w:lineRule=\"auto\""));
}

#[test]
fn writes_cell_padding_and_align() {
    let xml = render("<table><tr><td style='padding-top:1px;padding-right:2px;padding-bottom:3px;padding-left:4px;vertical-align:middle'>A</td></tr></table>");

    assert!(xml.contains("<w:tcMar><w:top w:w=\"15\" w:type=\"dxa\"/><w:left w:w=\"60\" w:type=\"dxa\"/><w:bottom w:w=\"45\" w:type=\"dxa\"/><w:right w:w=\"30\" w:type=\"dxa\"/></w:tcMar>"));
    assert!(xml.contains("<w:vAlign w:val=\"center\"/>"));
}

#[test]
fn preserves_css_pre_and_rtl() {
    let xml = render("<p style='white-space:pre;direction:rtl'>  A  B\n C </p>");

    assert_eq!(paragraph_texts(&xml), ["  A  B C "]);
    assert!(xml.contains("<w:br/>") && xml.contains("<w:bidi/>") && xml.contains("<w:rtl/>"));
}

#[test]
fn writes_page_break_boundaries() {
    let xml = render("<div style='break-before:page;break-after:page;margin-top:12pt'><p>A</p><p>B</p></div>");
    let paragraphs = elements(&xml, "w:p");

    assert!(paragraphs[0].contains("<w:pageBreakBefore/>"));
    assert!(!paragraphs[1].contains("<w:pageBreakBefore/>"));
    assert!(paragraphs[1].contains("<w:br w:type=\"page\"/>"));
    assert!(paragraphs[0].contains("w:before=\"240\""));
}

#[test]
fn orders_cell_properties_with_row_spans() {
    let xml = render("<table><tr><td rowspan='2' style='padding-top:1px;vertical-align:bottom'>A</td><td>B</td></tr><tr><td>C</td></tr></table>");

    assert!(xml.contains("<w:vMerge w:val=\"restart\"/><w:tcMar><w:top w:w=\"15\" w:type=\"dxa\"/></w:tcMar><w:vAlign w:val=\"bottom\"/></w:tcPr>"));
}
