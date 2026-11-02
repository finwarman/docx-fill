mod common;

use std::collections::BTreeMap;

use common::{cell, elements, fill, row, table};
use docx_fill::{FillOptions, PackageTemplate};
use serde_json::json;

#[test]
fn styles_the_tags_paragraph_and_its_runs() {
    let body = fill(
        r#"<w:p><w:pPr><w:spacing w:after="0"/></w:pPr><w:r><w:t>{:stylepar s}Title</w:t></w:r></w:p><w:p><w:r><w:t>plain</w:t></w:r></w:p>"#,
        json!({ "s": { "textAlign": "justify", "pStyle": "Heading1", "textColor": "#f00", "fontSize": 28, "bold": true, "cellBackground": "#00ff00" } }),
    );

    let [styled, plain] = elements(&body, "w:p")[..] else { panic!("two paragraphs") };
    assert!(styled.starts_with(
        r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/><w:shd w:val="clear" w:color="auto" w:fill="00FF00"/><w:spacing w:after="0"/><w:jc w:val="both"/><w:rPr><w:b/><w:bCs/><w:color w:val="FF0000"/><w:sz w:val="28"/><w:szCs w:val="28"/></w:rPr></w:pPr>"#
    ));
    assert!(styled.contains(r#"<w:r><w:rPr><w:b/><w:bCs/><w:color w:val="FF0000"/>"#));
    assert!(!plain.contains("w:rPr"));
}

#[test]
fn replaces_properties_the_run_already_has() {
    let body = fill(
        r#"<w:p><w:r><w:rPr><w:b/><w:color w:val="111111"/><w:u w:val="double"/></w:rPr><w:t>{:stylerun s}text</w:t></w:r></w:p>"#,
        json!({ "s": { "bold": false, "textColor": "#222222", "underline": true, "italic": true, "fontFamily": "Arial" } }),
    );

    assert!(body.contains(
        r#"<w:rPr><w:rFonts w:ascii="Arial" w:hAnsi="Arial" w:cs="Arial" w:eastAsia="Arial"/><w:b w:val="0"/><w:bCs w:val="0"/><w:i/><w:iCs/><w:color w:val="222222"/><w:u w:val="single"/></w:rPr>"#
    ));
}

#[test]
fn styles_only_the_run_holding_the_tag() {
    let body = fill(r#"<w:p><w:r><w:t>before </w:t></w:r><w:r><w:t>{:stylerun s}styled</w:t></w:r></w:p>"#, json!({ "s": { "italic": true } }));

    let runs = elements(&body, "w:r");
    assert!(!runs[0].contains("<w:i/>"));
    assert!(runs[1].contains("<w:i/>"));
}

#[test]
fn styles_every_cell_of_the_tags_row() {
    let body = fill(
        &table(&[row(&[cell(&["{:stylerow s}a"]), cell(&["b"])]), row(&[cell(&["c"]), cell(&["d"])])]),
        json!({ "s": { "cellBackground": "#123456", "textColor": "#abcdef", "borders": { "bottom": { "val": "single", "sz": 4, "color": "#000000" } } } }),
    );

    let rows = elements(&body, "w:tr");
    for styled in elements(rows[0], "w:tc") {
        assert!(styled.contains(r#"<w:tcBorders><w:bottom w:val="single" w:sz="4" w:color="000000"/></w:tcBorders><w:shd w:val="clear" w:color="auto" w:fill="123456"/>"#));
        assert!(styled.contains(r#"<w:color w:val="ABCDEF"/>"#));
    }
    assert!(!rows[1].contains("w:shd"));
}

#[test]
fn styles_a_cells_text_as_well_as_its_fill() {
    let body = fill(&table(&[row(&[cell(&["{:stylecell s}a"])])]), json!({ "s": { "cellBackground": "#fff", "textAlign": "center" } }));

    assert!(body.contains(r#"w:fill="FFFFFF""#));
    assert!(body.contains(r#"<w:jc w:val="center"/>"#));
}

#[test]
fn ignores_a_style_that_is_not_an_object() {
    let template = r#"<w:p><w:r><w:t>{:stylepar s}x</w:t></w:r></w:p>"#;

    for value in [json!("bold"), json!(null), json!([1])] {
        assert_eq!(fill(template, json!({ "s": value })), r#"<w:p><w:r><w:t xml:space="preserve">x</w:t></w:r></w:p>"#);
    }
}

#[test]
fn colours_the_bullet_of_the_tags_paragraph() {
    let body = fill(
        r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>{:stylebullets s}item</w:t></w:r></w:p>"#,
        json!({ "s": { "bulletColor": "#ff0000" } }),
    );

    assert!(body.contains(r#"<w:rPr><w:color w:val="FF0000"/></w:rPr></w:pPr>"#));
    assert!(!body.contains(r#"<w:r><w:rPr>"#));
}

#[test]
fn colours_a_drawingml_shape() {
    let body = fill(
        r#"<w:p><w:r><w:drawing><wps:wsp><wps:spPr><a:prstGeom prst="rect"/><a:noFill/><a:ln w="9525"><a:noFill/></a:ln></wps:spPr><wps:txbx><w:txbxContent><w:p><w:r><w:t>{:styleshape s}box</w:t></w:r></w:p></w:txbxContent></wps:txbx></wps:wsp></w:drawing></w:r></w:p>"#,
        json!({ "s": { "fillColor": "#00ff00", "strokeColor": "#0000ff" } }),
    );

    assert!(body.contains(
        r#"<wps:spPr><a:prstGeom prst="rect"/><a:solidFill><a:srgbClr val="00FF00"/></a:solidFill><a:ln w="9525"><a:solidFill><a:srgbClr val="0000FF"/></a:solidFill></a:ln></wps:spPr>"#
    ));
}

#[test]
fn colours_a_vml_shape() {
    let body = fill(
        r#"<w:p><w:r><w:pict><v:rect fillcolor="white"><v:textbox><w:txbxContent><w:p><w:r><w:t>{:styleshape s}box</w:t></w:r></w:p></w:txbxContent></v:textbox></v:rect></w:pict></w:r></w:p>"#,
        json!({ "s": { "fillColor": "#00ff00", "strokeColor": "#0000ff" } }),
    );

    assert!(body.contains(r##"<v:rect fillcolor="#00FF00" filled="t" strokecolor="#0000FF" stroked="t">"##));
}

#[test]
fn styles_the_runs_between_start_and_end_tags() {
    let body = fill(
        r#"<w:p><w:r><w:t>a{:stylestart s}b</w:t></w:r><w:r><w:rPr><w:i/></w:rPr><w:t>c{:/style}d</w:t></w:r></w:p>"#,
        json!({ "s": { "bold": true } }),
    );

    let runs = elements(&body, "w:r");
    assert_eq!(runs.len(), 4);
    assert!(!runs[0].contains("<w:b/>") && runs[0].contains(">a<"));
    assert!(runs[1].contains("<w:b/>") && runs[1].contains(">b<"));
    assert!(runs[2].contains("<w:b/>") && runs[2].contains("<w:i/>") && runs[2].contains(">c<"));
    assert!(!runs[3].contains("<w:b/>") && runs[3].contains(">d<"));
    assert!(!body.contains(['\u{E002}', '\u{E003}', '\u{E004}']));
}

#[test]
fn nests_ranges_across_paragraphs() {
    let body = fill(
        r#"<w:p><w:r><w:t>{:stylestart outer}a{:stylestart inner}b</w:t></w:r></w:p><w:p><w:r><w:t>c{:/style}d{:/style}e</w:t></w:r></w:p>"#,
        json!({ "outer": { "italic": true }, "inner": { "bold": true } }),
    );

    let runs = elements(&body, "w:r");
    let styled = |text: &str| runs.iter().find(|run| run.contains(&format!(">{text}<"))).unwrap().to_string();
    assert!(styled("a").contains("<w:i/>") && !styled("a").contains("<w:b/>"));
    assert!(styled("b").contains("<w:i/>") && styled("b").contains("<w:b/>"));
    assert!(styled("c").contains("<w:i/>") && styled("c").contains("<w:b/>"));
    assert!(styled("d").contains("<w:i/>") && !styled("d").contains("<w:b/>"));
    assert!(!styled("e").contains("<w:i/>"));
}

#[test]
fn style_tag_prints_escaped_text_from_the_style_object() {
    let body = fill(
        r#"<w:p><w:r><w:t>{:stylerun s}</w:t></w:r></w:p>"#,
        json!({ "s": { "text": "has <text> & more", "bold": true } }),
    );

    assert!(body.contains("<w:b/>"));
    assert!(body.contains("has &lt;text&gt; &amp; more"));
}

#[test]
fn package_theme_colors_replace_existing_and_system_colors() {
    let theme = r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:themeElements><a:clrScheme><a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1><a:accent1><a:srgbClr val="FFFFFF"/></a:accent1></a:clrScheme></a:themeElements></a:theme>"#;
    let mut package = PackageTemplate::parse(
        [("word/theme/theme1.xml".to_string(), theme.as_bytes().to_vec())],
        &FillOptions::default(),
    )
    .unwrap();
    package
        .set_theme_colors(BTreeMap::from([
            ("a:dk1".to_string(), "123456".to_string()),
            ("a:accent1".to_string(), "#abc".to_string()),
        ]))
        .unwrap();

    let parts = package.render(&json!({}));
    let theme = String::from_utf8(
        parts.into_iter().find(|(name, _)| name == "word/theme/theme1.xml").unwrap().1.into_owned(),
    )
    .unwrap();
    assert!(theme.contains(r#"<a:dk1><a:srgbClr val="123456"/></a:dk1>"#));
    assert!(theme.contains(r#"<a:accent1><a:srgbClr val="AABBCC"/></a:accent1>"#));
}

#[test]
fn package_theme_colors_reject_unknown_slots_and_invalid_hex() {
    let mut package = PackageTemplate::parse([], &FillOptions::default()).unwrap();
    assert!(
        package
            .set_theme_colors(BTreeMap::from([("a:other".to_string(), "123456".to_string())]))
            .is_err()
    );
    assert!(
        package
            .set_theme_colors(BTreeMap::from([("a:dk1".to_string(), "red".to_string())]))
            .is_err()
    );
}

#[test]
fn combines_two_styles_on_one_paragraph() {
    let body = fill(r#"<w:p><w:r><w:t>{:stylepar a}{:stylepar b}x</w:t></w:r></w:p>"#, json!({ "a": { "bold": true, "textColor": "#111111" }, "b": { "textColor": "#222222" } }));

    assert!(body.contains(r#"<w:b/><w:bCs/><w:color w:val="222222"/>"#));
}
