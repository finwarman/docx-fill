use docx_fill::{FillOptions, fill_part};
use serde_json::{Value, json};

fn fill(body: &str, data: Value, options: Value) -> String {
    let xml = format!("<w:document xmlns:w=\"w\" xmlns:wps=\"wps\" xmlns:v=\"v\"><w:body>{body}</w:body></w:document>");
    fill_part(&xml, &data, &FillOptions::default().read_json(&options).unwrap()).unwrap()
}

fn shape(content: &str) -> String {
    format!("<w:p><w:r><w:drawing><wps:wsp><wps:txbx><w:txbxContent>{content}</w:txbxContent></wps:txbx></wps:wsp></w:drawing></w:r></w:p>")
}

const CONDITIONAL: &str = "<w:p><w:r><w:t>{?show}middle</w:t></w:r></w:p>";

#[test]
fn removes_the_whole_shape() {
    let body = shape(&format!("{CONDITIONAL}<w:p><w:r><w:t>other</w:t></w:r></w:p>"));
    let xml = fill(&body, json!({"show":false}), json!({}));

    assert!(!xml.contains("w:drawing") && !xml.contains("other"));
}

#[test]
fn can_remove_only_the_paragraph() {
    let body = shape(&format!("{CONDITIONAL}<w:p><w:r><w:t>other</w:t></w:r></w:p>"));
    let xml = fill(&body, json!({"show":false}), json!({"paragraphPlaceholder":{"expandToShape":false}}));

    assert!(xml.contains("w:drawing") && xml.contains("other"));
    assert!(!xml.contains("middle"));
}

#[test]
fn repeats_a_shape_per_item() {
    let body = shape("<w:p><w:r><w:t>{?items}{name}</w:t></w:r></w:p>");
    let xml = fill(&body, json!({"items":[{"name":"A"},{"name":"B"}]}), json!({}));

    assert_eq!(xml.matches("<w:drawing>").count(), 2);
    assert!(xml.contains(">A</w:t>") && xml.contains(">B</w:t>"));
}

#[test]
fn conditions_a_soft_line() {
    let paragraph = "<w:p><w:pPr><w:keepNext/></w:pPr><w:r><w:rPr><w:b/></w:rPr><w:t>first</w:t><w:br/><w:t>{?show}middle</w:t><w:br/><w:t>last</w:t></w:r></w:p>";
    let xml = fill(&shape(paragraph), json!({"show":false}), json!({"paragraphPlaceholder":{"expandSoftBreaksInShapes":true}}));

    assert!(xml.contains("first") && xml.contains("last") && !xml.contains("middle"));
    assert_eq!(xml.matches("<w:br/>").count(), 1);
    assert_eq!(xml.matches("<w:keepNext/>").count(), 1);
    assert!(!xml.contains("data-docx-fill"));
    assert!(xml.contains("w:drawing"));
}

#[test]
fn can_target_non_shape_lines() {
    let paragraph = "<w:p><w:r><w:t>first</w:t><w:br/><w:t>{?show}middle</w:t><w:br/><w:t>last</w:t></w:r></w:p>";
    let xml = fill(paragraph, json!({"show":false}), json!({"paragraphPlaceholder":{"expandSoftBreaks":true}}));

    assert!(xml.contains("first") && xml.contains("last") && !xml.contains("middle"));
    assert_eq!(xml.matches("<w:p>").count(), 1);
    assert_eq!(xml.matches("<w:br/>").count(), 1);
}

#[test]
fn repeats_soft_lines() {
    let paragraph = "<w:p><w:r><w:t>first</w:t><w:br/><w:t>{?items}{name}</w:t><w:br/><w:t>last</w:t></w:r></w:p>";
    let xml = fill(&shape(paragraph), json!({"items":[{"name":"A"},{"name":"B"}]}), json!({"paragraphPlaceholder":{"expandSoftBreaksInShapes":true}}));

    assert_eq!(xml.matches("<w:drawing>").count(), 1);
    assert_eq!(xml.matches("<w:br/>").count(), 3);
    assert!(xml.contains(">A</w:t>") && xml.contains(">B</w:t>"));
}
