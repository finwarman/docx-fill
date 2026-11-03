mod common;

use common::{document, elements, paragraph, read_rows};
use docx_fill::{FillOptions, fill_part};
use serde_json::json;

#[test]
fn drops_extra_table_tag_text() {
    let template = document(&paragraph(&["before {:table t} after"]));
    let data = json!({"t":{"data":[["cell"]]}});
    assert!(fill_part(&template, &data, &FillOptions::default()).is_err());
    let options = FillOptions::default().read_json(&json!({"table":{"paragraphValidation":"ignore"}})).unwrap();
    let xml = fill_part(&template, &data, &options).unwrap();

    assert_eq!(read_rows(&xml), [vec!["cell"]]);
    assert!(!xml.contains("before") && !xml.contains("after"));
}

#[test]
fn inserts_fixed_body_columns() {
    let template = document(&paragraph(&["{:table t}"]));
    let data = json!({"t":{"fixedColumns":[null,"Fixed",null],"data":[["A","B"],["C","D"]]}});
    let xml = fill_part(&template, &data, &FillOptions::default()).unwrap();

    assert_eq!(read_rows(&xml), [vec!["A","Fixed","B"], vec!["C","","D"]]);
    let rows = elements(&xml, "w:tr");
    assert!(elements(rows[0], "w:tc")[1].contains("<w:vMerge w:val=\"restart\"/>"));
    assert!(elements(rows[1], "w:tc")[1].contains("<w:vMerge/>"));
}

#[test]
fn restarts_fixed_columns_per_chunk() {
    let template = document(&paragraph(&["{:table t}"]));
    let data = json!({"t":{"fixedColumns":["Fixed",null],"data":[["A"],["B"],["C"]],"chunkSize":2}});
    let xml = fill_part(&template, &data, &FillOptions::default()).unwrap();

    assert_eq!(xml.matches("Fixed").count(), 2);
    assert_eq!(xml.matches("<w:vMerge w:val=\"restart\"/>").count(), 1);
    assert_eq!(xml.matches("<w:vMerge/>").count(), 1);
}

#[test]
fn keeps_vertical_column_widths() {
    let template = document("<w:tbl><w:tblPr><w:tblW w:w=\"2000\" w:type=\"dxa\"/></w:tblPr><w:tblGrid><w:gridCol w:w=\"2000\"/></w:tblGrid><w:tr><w:tc><w:tcPr><w:tcW w:w=\"2000\" w:type=\"dxa\"/></w:tcPr><w:p><w:r><w:t>{:vt#items}{name}{:vt/items}</w:t></w:r></w:p></w:tc></w:tr></w:tbl>");
    let options = FillOptions::default().read_json(&json!({"table":{"autoResizeTable":true}})).unwrap();
    let xml = fill_part(&template, &json!({"items":[{"name":"A"},{"name":"B"}]}), &options).unwrap();

    assert!(xml.contains("<w:tblW w:w=\"4000\" w:type=\"dxa\"/>"));
    assert_eq!(xml.matches("<w:gridCol w:w=\"2000\"/>").count(), 2);
}

#[test]
fn keeps_a_tracked_grid_change_when_refitting_columns() {
    let template = document("<w:tbl><w:tblPr><w:tblW w:w=\"2000\" w:type=\"dxa\"/></w:tblPr><w:tblGrid><w:gridCol w:w=\"2000\"/><w:tblGridChange w:id=\"9\"><w:tblGrid><w:gridCol w:w=\"1000\"/></w:tblGrid></w:tblGridChange></w:tblGrid><w:tr><w:tc><w:tcPr><w:tcW w:w=\"2000\" w:type=\"dxa\"/></w:tcPr><w:p><w:r><w:t>{:vt#items}{name}{:vt/items}</w:t></w:r></w:p></w:tc></w:tr></w:tbl>");
    let xml = fill_part(&template, &json!({"items":[{"name":"A"},{"name":"B"}]}), &FillOptions::default()).unwrap();

    assert_eq!(xml.matches("<w:tblGrid>").count(), xml.matches("</w:tblGrid>").count());
    assert_eq!(xml.matches("<w:tblGridChange").count(), xml.matches("</w:tblGridChange>").count());
    let grid = &xml[xml.find("<w:tblGrid>").unwrap()..xml.find("<w:tblGridChange").unwrap()];
    assert_eq!(grid.matches("<w:gridCol w:w=\"1000\"/>").count(), 2);
}

#[test]
fn refits_live_widths_without_history() {
    let table_history = "<w:tblPrChange w:id=\"3\" w:author=\"A\"><w:tblPr><w:tblW w:w=\"12000\" w:type=\"dxa\"/></w:tblPr></w:tblPrChange>";
    let cell_history = "<w:tcPrChange w:id=\"4\" w:author=\"A\"><w:tcPr><w:tcW w:w=\"9000\" w:type=\"dxa\"/></w:tcPr></w:tcPrChange>";
    let template = document(&format!("<w:tbl><w:tblPr><w:tblW w:w=\"2000\" w:type=\"dxa\"/>{table_history}</w:tblPr><w:tblGrid><w:gridCol w:w=\"2000\"/></w:tblGrid><w:tr><w:tc><w:tcPr><w:tcW w:w=\"2000\" w:type=\"dxa\"/>{cell_history}</w:tcPr><w:p><w:r><w:t>{{:vt#items}}{{name}}{{:vt/items}}</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"));
    for (resize, width) in [(false, 1000), (true, 2000)] {
        let options = FillOptions::default().read_json(&json!({"table":{"autoResizeTable":resize}})).unwrap();
        let xml = fill_part(&template, &json!({"items":[{"name":"A"},{"name":"B"}]}), &options).unwrap();
        assert!(xml.contains(table_history));
        assert_eq!(xml.matches(cell_history.split_once('>').unwrap().1).count(), 2);
        assert_eq!(xml.matches(&format!("<w:tcPr><w:tcW w:w=\"{width}\" w:type=\"dxa\"/>")).count(), 2);
        assert_eq!(xml.matches(&format!("<w:gridCol w:w=\"{width}\"/>")).count(), 2);
        assert!(xml.contains(&format!("<w:tblPr><w:tblW w:w=\"{}\" w:type=\"dxa\"/>", width * 2)));
    }
}
