mod common;

use common::{cell, elements, fill, paragraph, read_rows, row, table};
use serde_json::json;

fn rows_of(values: &[[&str; 2]]) -> String {
    values.iter().map(|[a, b]| row(&[cell(&[a]), cell(&[b])])).collect()
}

#[test]
fn merges_equal_cells_down_a_listed_column() {
    let body = fill(
        &format!(
            "{}{}",
            paragraph(&["x"]),
            table(&[row(&[cell(&["{:merge-cells-col 1}Site"]), cell(&["Item"])]), rows_of(&[["A", "1"], ["A", "2"], ["B", "3"]])])
        ),
        json!({}),
    );

    let cells: Vec<Vec<&str>> = elements(&body, "w:tr").into_iter().map(|each| elements(each, "w:tc")).collect();
    assert!(cells[1][0].contains(r#"<w:vMerge w:val="restart"/>"#));
    assert!(cells[2][0].contains("<w:vMerge/>"));
    assert!(!cells[3][0].contains("vMerge"));
    assert!(!cells[1][1].contains("vMerge"));
    assert_eq!(read_rows(&body)[2], vec!["", "2"]);
}

#[test]
fn merges_repeated_rows_a_loop_prints() {
    let template = table(&[row(&[cell(&["{:merge-cells-col 1}{#rows}{site}"]), cell(&["{item}{/rows}"])])]);
    let body = fill(&template, json!({ "rows": [{ "site": "A", "item": 1 }, { "site": "A", "item": 2 }, { "site": "A", "item": 3 }] }));

    let merges: Vec<bool> = elements(&body, "w:tr").into_iter().map(|each| each.contains("vMerge")).collect();
    assert_eq!(merges, vec![true, true, true]);
    assert_eq!(body.matches("restart").count(), 1);
}

#[test]
fn leaves_empty_cells_apart() {
    let body = fill(&table(&[row(&[cell(&["{:merge-cells-col 1}"]), cell(&["a"])]), rows_of(&[["", "b"]])]), json!({}));

    assert!(!body.contains("vMerge"));
}

#[test]
fn merges_a_cell_only_when_the_column_before_merged_with_rtl() {
    let body = fill(
        &table(&[
            row(&[cell(&["{:merge-cells-col rtl 1,2}"]), cell(&[""])]),
            rows_of(&[["A", "x"], ["B", "x"], ["B", "x"]]),
        ]),
        json!({}),
    );

    // Column 2 is x all the way down, but merges only from row 3, where column 1 merges.
    let second: Vec<&str> = elements(&body, "w:tr").into_iter().map(|each| elements(each, "w:tc")[1]).collect();
    assert!(!second[1].contains("vMerge"));
    assert!(second[2].contains(r#"<w:vMerge w:val="restart"/>"#));
    assert!(second[3].contains("<w:vMerge/>"));
}

#[test]
fn spans_equal_neighbours_across_a_row_and_sums_their_widths() {
    let wide = |text: &str| format!(r#"<w:tc><w:tcPr><w:tcW w:w="1000" w:type="dxa"/></w:tcPr>{}</w:tc>"#, paragraph(&[text]));
    let body = fill(&table(&[row(&[wide("{:merge-cells-row}A"), wide("A"), wide("B")])]), json!({}));

    let cells = elements(&body, "w:tc");
    assert_eq!(cells.len(), 2);
    assert!(cells[0].contains(r#"<w:tcW w:w="2000" w:type="dxa"/><w:gridSpan w:val="2"/>"#));
}

#[test]
fn spans_only_the_listed_rows_counting_back_from_the_end() {
    let body = fill(&table(&[row(&[cell(&["{:merge-cells-row -1}"]), cell(&[""])]), rows_of(&[["A", "A"], ["B", "B"]])]), json!({}));

    let rows = read_rows(&body);
    assert_eq!(rows[1], vec!["A", "A"]);
    assert_eq!(rows[2], vec!["B"]);
}

#[test]
fn reads_the_tag_only_with_its_extension() {
    let options = docx_fill::FillOptions::core();
    let xml = docx_fill::fill_part(&common::document(&table(&[row(&[cell(&["{:merge-cells-col 1}"])])])), &json!({}), &options).unwrap();

    assert!(!xml.contains("vMerge"));
}

#[test]
fn ignores_old_cell_spans_and_widths() {
    let history = "<w:tcPrChange w:id=\"5\" w:author=\"A\"><w:tcPr><w:tcW w:w=\"9000\" w:type=\"dxa\"/><w:gridSpan w:val=\"3\"/></w:tcPr></w:tcPrChange>";
    let first = format!("<w:tc><w:tcPr>{history}</w:tcPr>{}</w:tc>", paragraph(&["{:merge-cells-row}A"]));
    let second = format!("<w:tc><w:tcPr><w:tcW w:w=\"1000\" w:type=\"dxa\"/></w:tcPr>{}</w:tc>", paragraph(&["A"]));
    let source = table(&[row(&[first, second])]);
    let expected = fill(&source.replace(history, ""), json!({}));
    let actual = fill(&source, json!({}));
    assert!(actual.contains(history));
    assert_eq!(actual.replace(history, ""), expected);
}

#[test]
fn preserves_history_in_merged_cells() {
    let history = "<w:tcPrChange w:id=\"5\" w:author=\"A\"><w:tcPr><w:tcW w:w=\"9000\" w:type=\"dxa\"/><w:gridSpan w:val=\"3\"/></w:tcPr></w:tcPrChange>";
    let below = format!("<w:tc><w:tcPr>{history}</w:tcPr>{}</w:tc>", paragraph(&["A"]));
    let source = table(&[row(&[cell(&["{:merge-cells-col 1}A"])]), row(&[below])]);
    let expected = fill(&source.replace(history, ""), json!({}));
    let actual = fill(&source, json!({}));
    assert!(actual.contains(history));
    assert_eq!(actual.replace(history, ""), expected);
}
