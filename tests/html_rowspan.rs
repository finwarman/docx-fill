mod common;

use common::{elements, fill, paragraph_texts};
use serde_json::json;

fn render(html: &str) -> String {
    fill("<w:p><w:r><w:t>{~~html}</w:t></w:r></w:p>", json!({"html":html}))
}

#[test]
fn keeps_spanned_columns_aligned() {
    let xml = render("<table><tr><td rowspan='2'>A</td><td>B</td></tr><tr><td>C</td></tr></table>");
    let rows = elements(&xml, "w:tr");

    assert_eq!(rows.len(), 2);
    assert!(rows[0].contains("<w:vMerge w:val=\"restart\"/>"));
    let cells = elements(rows[1], "w:tc");
    assert_eq!(cells.len(), 2);
    assert!(cells[0].contains("<w:vMerge/>") && !cells[1].contains("w:vMerge"));
    assert_eq!(paragraph_texts(cells[1]), ["C"]);
}

#[test]
fn combines_row_and_column_spans() {
    let xml = render("<table><tr><td rowspan='3' colspan='2'>A</td><td>B</td></tr><tr><td>C</td></tr><tr><td>D</td></tr></table>");
    let rows = elements(&xml, "w:tr");

    for row in rows {
        let cells = elements(row, "w:tc");
        assert_eq!(cells.len(), 2);
        assert!(cells[0].contains("<w:gridSpan w:val=\"2\"/>"));
    }
    assert_eq!(xml.matches("<w:vMerge/>").count(), 2);
    assert_eq!(xml.matches("<w:gridCol ").count(), 3);
}

#[test]
fn bounds_spans_to_existing_rows() {
    for span in ["0", "999999999"] {
        let xml = render(&format!("<table><tr><td rowspan='{span}'>A</td></tr><tr></tr></table>"));

        assert_eq!(xml.matches("<w:tr>").count(), 2);
        assert_eq!(xml.matches("<w:vMerge/>").count(), 1);
    }
}

#[test]
fn tracks_independent_spans() {
    let xml = render("<table><tr><td rowspan='2'>A</td><td rowspan='3'>B</td></tr><tr></tr><tr><td>C</td></tr></table>");
    let rows = elements(&xml, "w:tr");
    let cells = elements(rows[2], "w:tc");

    assert_eq!(paragraph_texts(cells[0]), ["C"]);
    assert!(cells[1].contains("<w:vMerge/>"));
}
