mod common;

use common::{cell, document, fill, fill_with, paragraph, paragraph_texts, read_rows, row, table, text};
use docx_fill::{Delimiters, FillOptions, fill_part};
use serde_json::{Value, json};

#[test]
fn prints_numbers_as_javascript_does() {
    let cases: [(Value, &str); 9] = [
        (json!(-3), "-3"),
        (json!(-2.5), "-2.5"),
        (json!(0.1 + 0.2), "0.30000000000000004"),
        (json!(1e21), "1e+21"),
        (json!(1.5e22), "1.5e+22"),
        (json!(1e20), "100000000000000000000"),
        (json!(0.000001), "0.000001"),
        (json!(1.5e-7), "1.5e-7"),
        (json!(-0.0), "0"),
    ];
    for (value, expected) in cases {
        assert_eq!(text(&fill(&paragraph(&["{v}"]), json!({ "v": value }))), expected, "{value}");
    }
}

#[test]
fn finds_a_tag_split_over_several_runs_of_multibyte_text() {
    let body = fill(&paragraph(&["é{na", "m", "e}ü"]), json!({ "name": "✓" }));

    assert_eq!(text(&body), "é✓ü");
}

#[test]
fn looks_up_a_name_written_with_entities() {
    let body = fill(&paragraph(&["{a&amp;b}"]), json!({ "a&b": "yes" }));

    assert_eq!(text(&body), "yes");
}

#[test]
fn nests_loops_reaching_out_to_each_scope() {
    let body = fill(&paragraph(&["{#groups}{name}:{#items}{name}{label};{/items}|{/groups}"]), json!({
        "label": "!",
        "groups": [{ "name": "g1", "items": [{ "name": "a" }, { "label": "?" }] }, { "name": "g2", "items": [] }]
    }));

    assert_eq!(text(&body), "g1:a!;g1?;|g2:|");
}

#[test]
fn nests_a_row_loop_inside_a_paragraph_loop() {
    let template = format!(
        "{}{}{}",
        paragraph(&["{#sites}"]),
        table(&[row(&[cell(&["{#rows}{site}"]), cell(&["{v}{/rows}"])])]),
        paragraph(&["{/sites}"])
    );
    let body = fill(&template, json!({ "sites": [{ "site": "A", "rows": [{ "v": 1 }, { "v": 2 }] }, { "site": "B", "rows": [{ "v": 3 }] }] }));

    assert_eq!(read_rows(&body), vec![vec!["A", "1"], vec!["A", "2"], vec!["B", "3"]]);
    assert_eq!(body.matches("<w:tbl>").count(), 2);
}

#[test]
fn drops_a_raw_tags_paragraph_for_a_missing_value() {
    let body = fill(&(paragraph(&["{@xml}"]) + &paragraph(&["after"])), json!({}));

    assert_eq!(paragraph_texts(&body), ["after"]);
}

#[test]
fn prints_an_inverted_section_inside_a_loop_per_item() {
    let body = fill(&paragraph(&["{#items}{^done}todo{/done}{#done}ok{/done},{/items}"]), json!({ "items": [{ "done": true }, {}, { "done": false }] }));

    assert_eq!(text(&body), "ok,todo,todo,");
}

#[test]
fn rejects_loops_that_overlap() {
    let xml = document(&(paragraph(&["{#a}{#b}"]) + &paragraph(&["{/a}{/b}"])));
    let error = fill_part(&xml, &json!({}), &FillOptions::default()).unwrap_err();

    assert_eq!(error.message, r#"The tag "b" is closed by the tag "a""#);
}

#[test]
fn rejects_a_loop_with_one_tag_in_a_table_and_one_outside() {
    for body in [paragraph(&["{#s}"]) + &table(&[row(&[cell(&["{/s}"])])]), table(&[row(&[cell(&["{#s}"])])]) + &paragraph(&["{/s}"])] {
        let error = fill_part(&document(&body), &json!({}), &FillOptions::default()).unwrap_err();
        assert_eq!(error.message, r#"The position of the loop tags "s" would produce invalid XML"#);
    }
}

#[test]
fn keeps_xml_outside_paragraphs_as_written() {
    let template = r#"<w:bookmarkStart w:id="0" w:name="x"/><w:p><w:r><w:t>{v}</w:t></w:r></w:p><!-- note --><w:sectPr><w:pgSz w:w="11906"/></w:sectPr>"#;
    let body = fill(template, json!({ "v": 1 }));

    assert!(body.starts_with(r#"<w:bookmarkStart w:id="0" w:name="x"/>"#));
    assert!(body.ends_with(r#"<!-- note --><w:sectPr><w:pgSz w:w="11906"/></w:sectPr>"#));
}

#[test]
fn prints_core_only_tags_as_values_without_extensions() {
    let xml = fill_part(&document(&paragraph(&["[{?a}][{r@b}][{:stylepar c}]"])), &json!({ "?a": 1, "r@b": 2, ":stylepar c": 3 }), &FillOptions::core()).unwrap();

    assert_eq!(text(&xml), "[1][2][3]");
}

fn fill_error(body: &str, data: Value) -> String {
    fill_part(&document(body), &data, &FillOptions::default()).unwrap_err().message
}

#[test]
fn refuses_a_raw_tag_beside_other_text() {
    for texts in [["a {@x}"], ["{@x}{@y}"]] {
        assert_eq!(fill_error(&paragraph(&texts), json!({ "x": "<w:p/>" })), r#"The raw tag "x" should be the only text in its paragraph"#);
    }
}

#[test]
fn prints_nothing_for_a_raw_value_that_is_not_xml() {
    for value in [json!(3), json!(true), json!(false), json!({ "a": 1 }), json!(0)] {
        let body = fill(&(paragraph(&["{@x}"]) + &paragraph(&["after"])), json!({ "x": value }));
        assert_eq!(paragraph_texts(&body), ["after"], "{value}");
        assert!(!body.contains("</w:p>3") && !body.contains("true") && !body.contains("object"), "{value}");
    }
}

#[test]
fn refuses_a_loop_across_cells_beside_another_loop_in_its_end_cells() {
    let data = json!({ "a": [{}], "b": [{}, {}] });
    for cells in [["{#a}1{/a}{#b}", "2{/b}"], ["{#a}1{/a}x{#b}", "2{/b}"], ["{#b}1", "2{/b}{#a}3{/a}"]] {
        let body = table(&[row(&[cell(&[cells[0]]), cell(&[cells[1]])])]);
        assert!(fill_error(&body, data.clone()).starts_with("Unbalanced loop tags "), "{cells:?}");
    }
}

#[test]
fn fills_loops_that_share_a_cell_without_crossing() {
    let data = json!({ "a": [{}], "b": [{}, {}] });
    let one_cell = fill(&table(&[row(&[cell(&["{#a}x{/a}{#b}y{/b}"])])]), data.clone());
    assert_eq!(read_rows(&one_cell), [["xyy"]]);
    let nested = fill(&table(&[row(&[cell(&["{#b}1"]), cell(&["{#a}2{/a}{/b}"])])]), data.clone());
    assert_eq!(read_rows(&nested), [["1", "2"], ["1", "2"]]);
    let after_text = fill(&table(&[row(&[cell(&["{#b}1"]), cell(&["2{/b}x"])])]), data);
    assert_eq!(read_rows(&after_text), [["1", "2x"], ["1", "2x"]]);
}

#[test]
fn drops_a_break_inside_a_tag_split_across_runs() {
    let breaks = |body: &str| fill(body, json!({ "name": "N" })).matches("<w:br").count();

    assert_eq!(breaks(r#"<w:p><w:r><w:t>A{na</w:t><w:br/></w:r><w:r><w:t>me}B</w:t></w:r></w:p>"#), 0);
    assert_eq!(breaks(r#"<w:p><w:r><w:t>A{na</w:t></w:r><w:r><w:br w:type="page"/><w:t>me}B</w:t></w:r></w:p>"#), 0);
    assert_eq!(breaks(r#"<w:p><w:r><w:t>A</w:t><w:br/><w:t>{name}</w:t><w:br/></w:r><w:r><w:t>B</w:t></w:r></w:p>"#), 2);
}

#[test]
fn keeps_a_break_inside_a_closing_delimiter() {
    let options = FillOptions { delimiters: Delimiters { start: "[[".into(), end: "]]".into() }, ..FillOptions::default() };
    let breaks = |body: &str| fill_with(body, json!({ "name": "N" }), &options).matches("<w:br").count();

    assert_eq!(breaks(r#"<w:p><w:r><w:t>A[[name]</w:t><w:br/></w:r><w:r><w:t>]B</w:t></w:r></w:p>"#), 1);
    assert_eq!(breaks(r#"<w:p><w:r><w:t>A[[name</w:t><w:br/></w:r><w:r><w:t>]]B</w:t></w:r></w:p>"#), 0);
    assert_eq!(breaks(r#"<w:p><w:r><w:t>A[</w:t><w:br/></w:r><w:r><w:t>[name]]B</w:t></w:r></w:p>"#), 0);
}

#[test]
fn keeps_tabs_inside_a_tag_split_across_runs() {
    let xml = fill(r#"<w:p><w:r><w:t>A{na</w:t><w:tab/></w:r><w:r><w:t>me}B</w:t></w:r></w:p>"#, json!({ "name": "N" }));

    assert!(xml.contains("<w:tab/>"), "{xml}");
}
