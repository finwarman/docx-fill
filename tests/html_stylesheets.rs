mod common;

use common::{elements, fill_with};
use docx_fill::FillOptions;
use serde_json::json;

fn render(sheet: &str, html: &str) -> String {
    let options = FillOptions::default().read_json(&json!({"htmlStyleSheet":sheet})).unwrap();
    fill_with("<w:p><w:r><w:t>{~~html}</w:t></w:r></w:p>", json!({"html":html}), &options)
}

#[test]
fn inline_styles_win() {
    let xml = render("p { color: red; font-size: 20px }", "<p style='color:blue'>A</p>");

    assert!(xml.contains("w:color w:val=\"0000FF\""));
    assert!(xml.contains("w:sz w:val=\"30\""));
    assert!(!xml.contains("w:color w:val=\"FF0000\""));
}

#[test]
fn embedded_rules_apply_globally() {
    let xml = render("p { color:red }", "<p>A</p><style>p {color:green}</style><p>B</p>");

    assert_eq!(xml.matches("w:color w:val=\"008000\"").count(), 2);
    assert!(!xml.contains("{color"));
}

#[test]
fn matches_declared_selectors() {
    let sheet = "* {font-size:10px} p, span {font-style:italic} div p {text-decoration:underline} .wrap > p.hit[data-kind^='pre'] {color:red} #first + p {font-weight:bold}";
    let html = "<div class='wrap'><p class='hit' id='first' data-kind='prefix'>A</p><p>B</p></div><p>C</p>";
    let xml = render(sheet, html);
    let paragraphs = elements(&xml, "w:p");

    assert!(paragraphs[0].contains("w:color w:val=\"FF0000\""));
    assert!(paragraphs[0].contains("<w:i/>") && paragraphs[0].contains("w:u w:val=\"single\""));
    assert!(paragraphs[1].contains("<w:b/>"));
    assert!(!paragraphs[2].contains("<w:b/>") && !paragraphs[2].contains("w:u "));
}

#[test]
fn obeys_specificity_and_important() {
    let xml = render("#x {color:green} .item {color:red} p {color:blue!important}", "<p id='x' class='item' style='color:black'>A</p>");
    assert!(xml.contains("w:color w:val=\"0000FF\""));
    let xml = render("#x {color:green} .item {color:red}", "<p id='x' class='item'>A</p>");
    assert!(xml.contains("w:color w:val=\"008000\""));
}

#[test]
fn ignores_style_text_in_scripts() {
    let xml = render("", "<script>'<style>p {color:red}</style>'</script><p>A</p>");
    assert!(!xml.contains("w:color") && !xml.contains("style>"));
}

#[test]
fn rejects_invalid_sheet_options() {
    for sheet in ["p { color:red", "p > { color:red }", "p { color }", "p:hover {color:red}"] {
        assert!(FillOptions::default().read_json(&json!({"htmlStyleSheet":sheet})).is_err(), "{sheet}");
    }
}

#[test]
fn hidden_nodes_break_adjacency() {
    let xml = render("p + p {color:red}", "<p>A</p><script>ignored</script><p>B</p><style>span {color:blue}</style><p>C</p>");

    assert!(!xml.contains("w:color") && !xml.contains("ignored"));
}

#[test]
fn matches_ancestors_in_deep_html() {
    const DEPTH: usize = 20_000;
    let html = format!("<div class='root'>{}deep{}</div><p>after</p>", "<span>".repeat(DEPTH), "</span>".repeat(DEPTH));
    let xml = render(".missing span {color:red} .root span {font-style:italic}", &html);
    let runs = elements(&xml, "w:r");

    assert!(runs[0].contains("<w:i/>") && runs[0].contains("deep"));
    assert!(!xml.contains("w:color"));
    assert!(runs.last().unwrap().contains("after") && !runs.last().unwrap().contains("<w:i/>"));
}

#[test]
fn keeps_combinator_matches_in_their_branch() {
    let sheet = ".root > p + section span {color:red} .root > p + section > span {font-weight:bold}";
    let html = "<div class='root'><p>A</p><section><span>B</span><div><span>C</span></div></section><section><span>D</span></section></div><span>E</span>";
    let xml = render(sheet, html);
    let runs = elements(&xml, "w:r");

    assert!(runs[1].contains("w:color w:val=\"FF0000\"") && runs[1].contains("<w:b/>"));
    assert!(runs[2].contains("w:color w:val=\"FF0000\"") && !runs[2].contains("<w:b/>"));
    assert!(!runs[3].contains("w:color") && !runs[4].contains("w:color"));
}

#[test]
fn matches_the_full_selector_depth() {
    let sheet = format!("{} {{color:red}}", std::iter::repeat_n("span", 64).collect::<Vec<_>>().join(" > "));
    let html = format!("{}leaf{}", "<span>".repeat(64), "</span>".repeat(64));
    assert!(render(&sheet, &html).contains("w:color w:val=\"FF0000\""));
    let shallower = format!("{}leaf{}", "<span>".repeat(63), "</span>".repeat(63));
    assert!(!render(&sheet, &shallower).contains("w:color"));
}
