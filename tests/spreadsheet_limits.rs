#[path = "common/spreadsheet.rs"]
mod common;
use common::*;
use docx_fill::{FillOptions, WorkbookLimits, WorkbookTemplate};
use serde_json::json;
#[test]
fn configured_limits_stop_row_and_horizontal_expansion() {
    for value in ["{#items}{.}", "{>items}{.}"] {
        let last = if value.starts_with("{#") {
            inline("B1", "{/items}")
        } else {
            String::new()
        };
        let p = package(&format!(
            r#"<row r="1">{}{last}</row>"#,
            inline("A1", value)
        ));
        let mut t = WorkbookTemplate::parse(p, &FillOptions::default()).unwrap();
        t.set_limits(WorkbookLimits {
            max_output_cells: 4,
            max_output_rows: 4,
            ..WorkbookLimits::default()
        })
        .unwrap();
        let error = t.render(&json!({"items":(0..100).collect::<Vec<_>>()})).unwrap_err();
        assert!(error.diagnostics().iter().all(|diagnostic|
            diagnostic.id == docx_fill::FillErrorId::SpreadsheetLimit));
    }
}
#[test]
fn malformed_templates_and_bad_values_do_not_panic() {
    let cases = [
        "{",
        "{{",
        "}",
        "{/missing}",
        "{#a}{/b}",
        "{>a}{/>b}",
        "{#a}{#b}{/a}",
        "{~html}",
        "{value}",
    ];
    for source in cases {
        let result = std::panic::catch_unwind(|| {
            let p = package(&format!(r#"<row r="1">{}</row>"#, inline("A1", source)));
            if let Ok(t) = WorkbookTemplate::parse(p, &FillOptions::default()) {
                let _ = t.render(
                    &json!({"a":[null,1,2],"value":{"bad":true},"html":"<script>x</script>"}),
                );
            }
        });
        assert!(result.is_ok(), "panic for {source}");
    }
}
#[test]
fn lower_byte_budget_and_invalid_limit_values_are_reported() {
    let p = package(&format!(r#"<row r="1">{}</row>"#, inline("A1", "{v}")));
    let mut t = WorkbookTemplate::parse(p, &FillOptions::default()).unwrap();
    assert!(
        t.set_limits(WorkbookLimits {
            max_output_cells: 0,
            ..WorkbookLimits::default()
        })
        .is_err()
    );
    t.set_limits(WorkbookLimits {
        max_output_bytes: 1024,
        ..WorkbookLimits::default()
    })
    .unwrap();
    assert!(t.render(&json!({"v":"x".repeat(4096)})).is_err());
}
