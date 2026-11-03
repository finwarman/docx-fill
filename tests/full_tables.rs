mod common;

use common::{cell, document, elements, fill, paragraph, paragraph_texts, read_rows, row, table, text};
use docx_fill::{FillOptions, fill_part};
use serde_json::json;

fn error(body: &str) -> String {
    fill_part(&document(body), &json!({}), &FillOptions::default()).unwrap_err().message
}

#[test]
fn builds_a_table_from_rows_in_place_of_the_tags_paragraph() {
    let body = fill(&(paragraph(&["{:table t}"]) + &paragraph(&["after"])), json!({ "t": { "data": [["a", 1], ["b", 2.5]] } }));

    assert_eq!(read_rows(&body), vec![vec!["a", "1"], vec!["b", "2.5"]]);
    assert!(body.ends_with(&paragraph(&["after"])));
    assert!(!body.contains("{:table"));
}

#[test]
fn draws_single_black_borders_by_default() {
    let body = fill(&paragraph(&["{:table t}"]), json!({ "t": { "data": [["a"]] } }));

    assert!(body.contains(r#"<w:tblBorders><w:top w:val="single" w:sz="4" w:space="0" w:color="000000"/>"#));
    assert!(body.contains(r#"<w:insideV w:val="single" w:sz="4" w:space="0" w:color="000000"/></w:tblBorders>"#));
}

#[test]
fn chooses_outer_inner_or_no_borders() {
    let borders = |border: serde_json::Value| fill(&paragraph(&["{:table t}"]), json!({ "t": { "data": [["a"]], "border": border } }));

    let outer = borders(json!({ "type": "border", "size": 8, "color": "#ff0000" }));
    assert!(outer.contains(r#"<w:top w:val="single" w:sz="8" w:space="0" w:color="FF0000"/>"#));
    assert!(outer.contains(r#"<w:insideH w:val="nil"/>"#));
    assert!(borders(json!({ "type": "inner" })).contains(r#"<w:top w:val="nil"/>"#));
    assert!(!borders(json!("none")).contains("single"));
    assert!(borders(json!({ "type": "none", "bottom": { "val": "double", "size": 6 } })).contains(r#"<w:bottom w:val="double" w:sz="6" w:space="0" w:color="000000"/>"#));
}

#[test]
fn prints_a_bold_repeating_header_and_an_italic_subheader() {
    let body = fill(&paragraph(&["{:table t}"]), json!({ "t": { "header": ["H"], "subheader": ["S"], "data": [["d"]] } }));

    let rows = elements(&body, "w:tr");
    assert!(rows[0].contains("<w:tblHeader/>") && rows[0].contains("<w:rPr><w:b/></w:rPr>"));
    assert!(rows[1].contains("<w:rPr><w:i/></w:rPr>") && !rows[1].contains("tblHeader"));
    assert!(!rows[2].contains("w:rPr"));
}

#[test]
fn sizes_columns_in_table_pixels_percentages_or_evenly() {
    // 75 px to the inch; the default page body is 698 px at 96 to the inch, 10470 twips.
    let widths = |options: serde_json::Value| {
        let mut value = json!({ "data": [["a", "b"]] });
        value.as_object_mut().unwrap().extend(options.as_object().unwrap().clone());
        let body = fill(&paragraph(&["{:table t}"]), json!({ "t": value }));
        elements(&body, "w:tblGrid")[0].to_string()
    };

    assert_eq!(widths(json!({ "widths": [75, "25%"] })), r#"<w:tblGrid><w:gridCol w:w="1440"/><w:gridCol w:w="2618"/></w:tblGrid>"#);
    assert_eq!(widths(json!({ "totalWidth": 150 })), r#"<w:tblGrid><w:gridCol w:w="1440"/><w:gridCol w:w="1440"/></w:tblGrid>"#);
    assert_eq!(widths(json!({})), r#"<w:tblGrid><w:gridCol w:w="5235"/><w:gridCol w:w="5235"/></w:tblGrid>"#);
}

#[test]
fn spans_styles_and_splits_cells() {
    let body = fill(
        &paragraph(&["{:table t}"]),
        json!({ "t": {
            "style": { "normal": { "textAlign": "right", "runProperties": "<w:color w:val=\"333333\"/>" } },
            "data": [
                [{ "text": "wide", "colspan": 2, "style": { "shade": "#ddDD33", "verticalAlign": "center" } }],
                [{ "paragraphs": [{ "text": "one" }, { "text": "two", "style": { "textAlign": "center" } }] }, "line\nbreak"],
                ["short"]
            ]
        } }),
    );

    let rows = elements(&body, "w:tr");
    assert!(rows[0].contains(r#"<w:gridSpan w:val="2"/><w:shd w:val="clear" w:color="auto" w:fill="DDDD33"/><w:vAlign w:val="center"/>"#));
    let second = elements(rows[1], "w:tc");
    assert_eq!(paragraph_texts(second[0]), ["one", "two"]);
    assert!(second[0].contains(r#"<w:jc w:val="right"/>"#) && second[0].contains(r#"<w:jc w:val="center"/>"#));
    assert!(second[1].contains(r#"<w:t xml:space="preserve">line</w:t><w:br/><w:t xml:space="preserve">break</w:t>"#));
    assert!(second[1].contains(r#"<w:rPr><w:color w:val="333333"/></w:rPr>"#));
    assert_eq!(elements(rows[2], "w:tc").len(), 2);
}

#[test]
fn splits_rows_into_tables_with_page_breaks_between() {
    let data = json!([["1"], ["2"], ["3"]]);
    let split = |options: serde_json::Value| {
        let mut value = json!({ "header": ["H"], "data": data, "chunkSize": 2 });
        value.as_object_mut().unwrap().extend(options.as_object().unwrap().clone());
        fill(&paragraph(&["{:table t}"]), json!({ "t": value }))
    };

    let broken = split(json!({}));
    assert_eq!(broken.matches("<w:tbl>").count(), 2);
    assert_eq!(read_rows(&broken), vec![vec!["H"], vec!["1"], vec!["2"], vec!["H"], vec!["3"]]);
    assert!(broken.contains(r#"</w:tbl><w:p><w:r><w:br w:type="page"/></w:r></w:p><w:tbl>"#));
    assert!(split(json!({ "pageBreakBetween": false })).contains("</w:tbl><w:p/><w:tbl>"));
    assert!(split(json!({ "finalPageBreak": true })).ends_with(r#"</w:tbl><w:p><w:r><w:br w:type="page"/></w:r></w:p>"#));
}

#[test]
fn writes_table_level_options() {
    let body = fill(
        &paragraph(&["{:table t}"]),
        json!({ "t": { "data": [["a"]], "indent": 200, "align": "center", "layout": "fixed", "cellMargin": { "left": 50, "right": 60 }, "height": 30, "heightRule": "exact", "spacingAfter": 0 } }),
    );

    assert!(body.contains(r#"<w:jc w:val="center"/><w:tblInd w:w="200" w:type="dxa"/><w:tblBorders>"#));
    assert!(body.contains(r#"</w:tblBorders><w:tblLayout w:type="fixed"/><w:tblCellMar><w:left w:w="50" w:type="dxa"/><w:right w:w="60" w:type="dxa"/></w:tblCellMar></w:tblPr>"#));
    assert!(body.contains(r#"<w:trPr><w:trHeight w:val="576" w:hRule="exact"/></w:trPr>"#));
    assert!(body.contains(r#"<w:pPr><w:spacing w:after="0"/></w:pPr>"#));
}

#[test]
fn prints_nothing_without_rows_and_ends_a_cell_with_a_paragraph() {
    for value in [json!(null), json!({ "data": [] }), json!({ "header": ["H"] })] {
        assert_eq!(fill(&paragraph(&["{:table t}"]), json!({ "t": value })), "");
    }
    let body = fill(&table(&[row(&[cell(&["{:table t}"])])]), json!({ "t": { "data": [["in"]] } }));
    assert!(body.contains("</w:tbl><w:p/></w:tc>"));
}

#[test]
fn rejects_a_table_tag_beside_other_text() {
    assert_eq!(error(&paragraph(&["x {:table t}"])), r#"The table tag ":table t" should be the only text in its paragraph"#);
}

#[test]
fn prints_the_grids_paragraphs_per_item_row_by_row() {
    let template = [paragraph(&["{:#grid people}"]), paragraph(&["{name}"]), paragraph(&["{role}"]), paragraph(&["{:/grid}"])].concat();
    let items = json!([{ "name": "A", "role": "x" }, { "name": "B", "role": "y" }, { "name": "C", "role": "z" }]);
    let body = fill(&template, json!({ "people": { "size": [2, 2], "data": items } }));

    assert_eq!(read_rows(&body), vec![vec!["Ax", "By"], vec!["Cz", ""]]);
    let cells: Vec<&str> = elements(&body, "w:tc");
    assert_eq!(paragraph_texts(cells[0]), ["A", "x"]);
    assert!(cells[3].ends_with("<w:p/></w:tc>"));
    assert!(!body.contains("tblBorders"));
    assert!(!text(&body).contains("grid"));
}

#[test]
fn starts_a_new_table_on_a_new_page_when_a_grid_fills() {
    let template = [paragraph(&["{:#grid g}"]), paragraph(&["{.}"]), paragraph(&["{:/grid}"])].concat();
    let body = fill(&template, json!({ "g": { "size": [1, 2], "data": ["1", "2", "3"], "widths": [100, 50], "height": 75 } }));

    assert_eq!(body.matches("<w:tbl>").count(), 2);
    assert_eq!(read_rows(&body), vec![vec!["1", "2"], vec!["3", ""]]);
    assert!(body.contains(r#"<w:gridCol w:w="1920"/><w:gridCol w:w="960"/>"#));
    assert!(body.contains(r#"<w:trHeight w:val="1440" w:hRule="atLeast"/>"#));
    assert!(body.contains(r#"<w:br w:type="page"/>"#));
}

#[test]
fn prints_nothing_for_a_grid_without_size_or_data() {
    let template = [paragraph(&["{:#grid g}"]), paragraph(&["x"]), paragraph(&["{:/grid}"]), paragraph(&["after"])].concat();

    for value in [json!(null), json!({ "data": [1] }), json!({ "size": [1], "data": [1] }), json!({ "size": [1, 1] })] {
        assert_eq!(paragraph_texts(&fill(&template, json!({ "g": value }))), ["after"]);
    }
}

#[test]
fn rejects_grid_tags_sharing_a_paragraph() {
    assert_eq!(error(&paragraph(&["{:#grid g}x{:/grid}"])), r#"The grid tags "g" should each be alone in their own paragraph"#);
}

#[test]
fn survives_a_grid_size_whose_cell_count_overflows() {
    let template = [paragraph(&["{:#grid g}"]), paragraph(&["{.}"]), paragraph(&["{:/grid}"])].concat();
    let _ = fill(&template, json!({ "g": { "size": [9223372036854775808_u64, 2], "data": ["1"] } }));
}

#[test]
fn bounds_the_columns_a_huge_grid_or_colspan_asks_for() {
    let template = [paragraph(&["{:#grid g}"]), paragraph(&["{.}"]), paragraph(&["{:/grid}"])].concat();
    let grid = fill(&template, json!({ "g": { "size": [1, 1_000_000_000_000_u64], "data": ["1"] } }));
    let table = fill(&paragraph(&["{:table t}"]), json!({ "t": { "data": [[{ "text": "a", "colspan": 1_000_000_000_000_u64 }]] } }));

    assert!(grid.len() < 100_000 && table.len() < 100_000);
}

#[test]
fn survives_column_widths_whose_sum_overflows() {
    let body = fill(&paragraph(&["{:table t}"]), json!({ "t": { "data": [[{ "text": "a", "colspan": 2 }]], "widths": [1e300, 1e300] } }));

    assert!(body.contains("<w:tblGrid>"));
}
