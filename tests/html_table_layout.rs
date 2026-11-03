mod common;

use common::{NAMESPACES, document, elements, fill};
use docx_fill::{FillOptions, PackageTemplate};
use serde_json::json;

const TAG: &str = "<w:p><w:r><w:t>{~~html}</w:t></w:r></w:p>";
const SECTION: &str = "<w:sectPr><w:pgSz w:w=\"12000\" w:h=\"16000\"/><w:pgMar w:left=\"1000\" w:right=\"2000\"/></w:sectPr>";

fn render(html: &str) -> String {
    fill(&format!("{TAG}{SECTION}"), json!({"html": html}))
}

#[test]
fn fits_the_section_text_width() {
    let xml = render("<table><tr><td>A</td><td>B</td></tr></table>");

    assert!(xml.contains("<w:tblW w:w=\"8640\" w:type=\"dxa\"/>"), "{xml}");
    assert_eq!(xml.matches("<w:gridCol w:w=\"4320\"/>").count(), 2);
    assert_eq!(xml.matches("<w:tcW w:w=\"4320\" w:type=\"dxa\"/>").count(), 2);
    assert!(xml.contains("<w:tblCellMar><w:top w:w=\"55\" w:type=\"dxa\"/>"));
    assert!(xml.contains("<w:top w:val=\"single\" w:sz=\"2\""));
}

#[test]
fn lets_full_width_exceed_default() {
    let xml = render("<table style='width:100%'><tr><td>A</td></tr></table>");

    assert!(xml.contains("<w:tblW w:w=\"9000\" w:type=\"dxa\"/>"), "{xml}");
}

#[test]
fn keeps_explicit_widths() {
    let xml = render("<table style='width:240px;table-layout:fixed'><tr><td style='width:25%'>A</td><td>B</td></tr></table>");

    assert!(xml.contains("<w:tblW w:w=\"3600\" w:type=\"dxa\"/>"), "{xml}");
    assert!(xml.contains("<w:tblLayout w:type=\"fixed\"/>"));
    assert!(xml.contains("<w:gridCol w:w=\"900\"/><w:gridCol w:w=\"2700\"/>"));
}

#[test]
fn sizes_nested_tables() {
    let xml = render("<table><tr><td><table><tr><td>inner</td></tr></table></td><td>outer</td></tr></table>");

    assert!(xml.contains("<w:tblW w:w=\"8640\" w:type=\"dxa\"/>"));
    assert!(xml.contains("<w:tblW w:w=\"4220\" w:type=\"dxa\"/>"), "{xml}");
}

#[test]
fn uses_template_cell_width() {
    let body = format!("<w:tbl><w:tr><w:tc><w:tcPr><w:tcW w:w=\"3000\" w:type=\"dxa\"/><w:tcMar><w:left w:w=\"90\" w:type=\"dxa\"/><w:right w:w=\"150\" w:type=\"dxa\"/></w:tcMar></w:tcPr>{TAG}</w:tc></w:tr></w:tbl>{SECTION}");
    let xml = fill(&body, json!({"html": "<table><tr><td>A</td></tr></table>"}));

    assert!(xml.contains("<w:tblW w:w=\"2760\" w:type=\"dxa\"/>"), "{xml}");
}

#[test]
fn keeps_padding_and_borders() {
    let xml = render("<table cellpadding='6' style='border:none'><tr><td style='padding:0 4px;border-bottom:2px dashed red'>A</td></tr></table>");

    assert!(xml.contains("<w:tblCellMar><w:top w:w=\"90\" w:type=\"dxa\"/>"), "{xml}");
    assert!(xml.contains("<w:tcMar><w:top w:w=\"0\" w:type=\"dxa\"/><w:left w:w=\"60\" w:type=\"dxa\"/><w:bottom w:w=\"0\" w:type=\"dxa\"/><w:right w:w=\"60\" w:type=\"dxa\"/></w:tcMar>"));
    assert!(xml.contains("<w:top w:val=\"nil\"/>"));
    assert!(xml.contains("<w:bottom w:val=\"dashed\" w:sz=\"12\" w:space=\"0\" w:color=\"FF0000\"/>"));
}

#[test]
fn resolves_percent_width() {
    let xml = render("<table style='width:50%;margin-left:20px'><tr><td>A</td></tr></table>");

    assert!(xml.contains("<w:tblW w:w=\"4500\" w:type=\"dxa\"/>"), "{xml}");
    assert!(xml.contains("<w:tblInd w:w=\"345\" w:type=\"dxa\"/>"));
}

#[test]
fn reads_each_section_width() {
    let second = "<w:sectPr><w:pgSz w:w=\"9000\" w:h=\"16000\"/><w:pgMar w:left=\"1000\" w:right=\"2000\"/></w:sectPr>";
    let body = format!("{TAG}<w:p><w:pPr>{SECTION}</w:pPr></w:p>{TAG}{second}");
    let xml = fill(&body, json!({"html": "<table><tr><td>A</td></tr></table>"}));
    let tables = elements(&xml, "w:tbl");

    assert!(tables[0].contains("<w:tblW w:w=\"8640\" w:type=\"dxa\"/>"), "{xml}");
    assert!(tables[1].contains("<w:tblW w:w=\"6000\" w:type=\"dxa\"/>"));
}

#[test]
fn reads_html_width_attributes() {
    let xml = render("<table width='200'><tr><td width='60'>A</td><td>B</td></tr></table>");

    assert!(xml.contains("<w:tblW w:w=\"3000\" w:type=\"dxa\"/>"), "{xml}");
    assert!(xml.contains("<w:gridCol w:w=\"900\"/><w:gridCol w:w=\"2100\"/>"));
}

#[test]
fn uses_the_breaks_own_section() {
    let body = format!("<w:p><w:pPr>{SECTION}</w:pPr><w:r><w:t>{{~~html}}</w:t></w:r></w:p><w:sectPr><w:pgSz w:w=\"6000\" w:h=\"16000\"/></w:sectPr>");
    let xml = fill(&body, json!({"html": "<table><tr><td>A</td></tr></table>"}));

    assert!(xml.contains("<w:tblW w:w=\"8640\" w:type=\"dxa\"/>"), "{xml}");
}

#[test]
fn uses_page_column_width() {
    let body = format!("{TAG}<w:sectPr><w:pgSz w:w=\"12000\" w:h=\"16000\"/><w:pgMar w:left=\"1000\" w:right=\"2000\"/><w:cols w:num=\"2\" w:space=\"600\"/></w:sectPr>");
    let xml = fill(&body, json!({"html": "<table><tr><td>A</td></tr></table>"}));

    assert!(xml.contains("<w:tblW w:w=\"4200\" w:type=\"dxa\"/>"), "{xml}");
}

#[test]
fn keeps_column_sizes_and_units() {
    let xml = render("<table style='width:2in;table-layout:fixed'><col width='25%'><col width='75%'><tr><td>A</td><td>B</td></tr></table>");

    assert!(xml.contains("<w:tblW w:w=\"2880\" w:type=\"dxa\"/>"));
    assert!(xml.contains("<w:gridCol w:w=\"720\"/><w:gridCol w:w=\"2160\"/>"), "{xml}");
}

#[test]
fn keeps_positive_narrow_columns() {
    let xml = render("<table style='width:1px'><tr><td style='width:100000px'>A</td><td>B</td></tr></table>");

    assert!(!xml.contains("<w:gridCol w:w=\"0\"/>"), "{xml}");
    assert!(xml.contains("<w:tblW w:w=\"15\" w:type=\"dxa\"/>"));
}

#[test]
fn sums_explicit_cell_widths() {
    let xml = render("<table><tr><td width='60'>A</td><td width='140'>B</td></tr></table>");

    assert!(xml.contains("<w:tblW w:w=\"3000\" w:type=\"dxa\"/>"), "{xml}");
    assert!(xml.contains("<w:gridCol w:w=\"900\"/><w:gridCol w:w=\"2100\"/>"));
}

#[test]
fn ignores_old_section_widths() {
    let section = "<w:sectPr><w:pgSz w:w=\"12000\" w:h=\"16000\"/><w:pgMar w:left=\"1000\" w:right=\"2000\"/><w:sectPrChange w:id=\"1\"><w:sectPr><w:pgSz w:w=\"9000\" w:h=\"16000\"/><w:pgMar w:left=\"1000\" w:right=\"2000\"/></w:sectPr></w:sectPrChange></w:sectPr>";
    let xml = fill(&format!("{TAG}{section}"), json!({"html": "<table><tr><td>A</td></tr></table>"}));

    assert!(xml.contains("<w:tblW w:w=\"8640\" w:type=\"dxa\"/>"), "{xml}");
    assert!(xml.contains(section));
}

#[test]
fn reads_false_equal_width() {
    let section = "<w:sectPr><w:pgSz w:w=\"9000\" w:h=\"16000\"/><w:cols w:equalWidth=\"false\"><w:col w:w=\"2000\"/><w:col w:w=\"7000\"/></w:cols></w:sectPr>";
    let xml = fill(&format!("{TAG}{section}"), json!({"html": "<table><tr><td>A</td></tr></table>"}));

    assert!(xml.contains("<w:tblW w:w=\"2000\" w:type=\"dxa\"/>"), "{xml}");
}

#[test]
fn splits_fallback_page_width() {
    let body = format!("{TAG}<w:sectPr><w:cols w:num=\"2\" w:space=\"720\"/></w:sectPr>");
    let xml = fill(&body, json!({"html": "<table><tr><td>A</td></tr></table>"}));

    assert!(xml.contains("<w:tblW w:w=\"4875\" w:type=\"dxa\"/>"), "{xml}");
}

#[test]
fn applies_padding_in_order() {
    for (style, left) in [("padding-left:20px;padding:0", 0), ("padding:0;padding-left:20px", 300)] {
        let xml = render(&format!("<table><tr><td style='{style}'>A</td></tr></table>"));
        let margins = elements(&xml, "w:tcMar");

        assert!(margins[0].contains(&format!("<w:left w:w=\"{left}\"")), "{style}: {xml}");
    }
}

#[test]
fn applies_border_in_order() {
    for (style, border) in [("border-left:2px solid red;border:none", "<w:left w:val=\"nil\"/>"), ("border:none;border-left:2px solid red", "<w:left w:val=\"single\" w:sz=\"12\"")] {
        let xml = render(&format!("<table><tr><td style='{style}'>A</td></tr></table>"));
        let borders = elements(&xml, "w:tcBorders");

        assert!(borders[0].contains(border), "{style}: {xml}");
    }
}

#[test]
fn keeps_important_table_styles() {
    let xml = render("<style>td {padding:0 !important; border-left:2px solid red !important}</style><table><tr><td style='padding:20px;border:none'>A</td></tr></table>");
    let margins = elements(&xml, "w:tcMar");
    let borders = elements(&xml, "w:tcBorders");

    assert!(margins[0].contains("<w:left w:w=\"0\""), "{xml}");
    assert!(borders[0].contains("<w:left w:val=\"single\" w:sz=\"12\""));
}

#[test]
fn keeps_spaced_rgb_borders() {
    let xml = render("<style>td {border-bottom:2px solid rgb(10, 20, 30)}</style><table style='border:1px solid rgb(255, 0, 0)'><tr><td>A</td></tr></table>");
    let table = elements(&xml, "w:tblBorders");
    let cell = elements(&xml, "w:tcBorders");

    assert!(table[0].contains("w:color=\"FF0000\""), "{xml}");
    assert!(cell[0].contains("<w:bottom w:val=\"single\" w:sz=\"12\" w:space=\"0\" w:color=\"0A141E\"/>"), "{xml}");
}

#[test]
fn uses_percentage_cell_width() {
    for (kind, width, expected) in [("dxa", 6000, 2760), ("pct", 2500, 2010)] {
        let body = format!("<w:tbl><w:tblPr><w:tblW w:w=\"{width}\" w:type=\"{kind}\"/><w:tblPrChange w:id=\"1\"><w:tblPr><w:tblW w:w=\"1000\" w:type=\"dxa\"/></w:tblPr></w:tblPrChange></w:tblPr><w:tr><w:tc><w:tcPr><w:tcW w:w=\"2500\" w:type=\"pct\"/><w:tcMar><w:left w:w=\"90\" w:type=\"dxa\"/><w:right w:w=\"150\" w:type=\"dxa\"/></w:tcMar></w:tcPr>{TAG}</w:tc></w:tr></w:tbl>{SECTION}");
        let xml = fill(&body, json!({"html": "<table><tr><td>A</td></tr></table>"}));

        assert!(xml.contains(&format!("<w:tblW w:w=\"{expected}\" w:type=\"dxa\"/>")), "{kind}: {xml}");
    }
}

#[test]
fn resolves_nested_native_width() {
    let body = format!("<w:tbl><w:tr><w:tc><w:tcPr><w:tcW w:w=\"8000\" w:type=\"dxa\"/><w:tcMar><w:left w:w=\"100\" w:type=\"dxa\"/><w:right w:w=\"100\" w:type=\"dxa\"/></w:tcMar></w:tcPr><w:tbl><w:tblPr><w:tblW w:w=\"2500\" w:type=\"pct\"/></w:tblPr><w:tr><w:tc><w:tcPr><w:tcW w:w=\"2500\" w:type=\"pct\"/><w:tcMar><w:left w:w=\"50\" w:type=\"dxa\"/><w:right w:w=\"50\" w:type=\"dxa\"/></w:tcMar></w:tcPr>{TAG}</w:tc></w:tr></w:tbl><w:p/></w:tc></w:tr></w:tbl>{SECTION}");
    let xml = fill(&body, json!({"html": "<table><tr><td>A</td></tr></table>"}));

    assert!(xml.contains("<w:tblW w:w=\"1850\" w:type=\"dxa\"/>"), "{xml}");
}

#[test]
fn sizes_tables_in_header_footer() {
    let table = format!("<w:tbl><w:tblPr><w:tblW w:w=\"5000\" w:type=\"pct\"/></w:tblPr><w:tr><w:tc><w:tcPr><w:tcW w:w=\"2500\" w:type=\"pct\"/></w:tcPr>{TAG}</w:tc></w:tr></w:tbl>");
    let template = PackageTemplate::parse([
        ("word/header1.xml".to_string(), format!("<w:hdr {NAMESPACES}>{table}</w:hdr>").into_bytes()),
        ("word/footer1.xml".to_string(), format!("<w:ftr {NAMESPACES}>{table}</w:ftr>").into_bytes()),
        ("word/document.xml".to_string(), document(SECTION).into_bytes()),
    ], &FillOptions::default()).unwrap();
    let parts = template.render(&json!({"html": "<table><tr><td>A</td></tr></table>"}));

    for name in ["word/header1.xml", "word/footer1.xml"] {
        let xml = String::from_utf8_lossy(&parts.iter().find(|(part, _)| part == name).unwrap().1);

        assert!(xml.contains("<w:tblW w:w=\"4284\" w:type=\"dxa\"/>"), "{name}: {xml}");
    }
}

#[test]
fn applies_table_margin_in_order() {
    for (style, indent) in [("margin-left:20px;margin:0", 45), ("margin:0;margin-left:20px", 345)] {
        let xml = render(&format!("<table style='{style}'><tr><td>A</td></tr></table>"));

        assert!(xml.contains(&format!("<w:tblInd w:w=\"{indent}\"")), "{style}: {xml}");
    }
}

#[test]
fn fits_auto_width_template_cells() {
    for (width, expected) in [("<w:tcW w:w=\"0\" w:type=\"auto\"/>", 2784), ("<w:tcW w:w=\"1667\" w:type=\"pct\"/>", 784)] {
        let body = format!("<w:tbl><w:tblPr><w:tblW w:w=\"3000\" w:type=\"dxa\"/></w:tblPr><w:tblGrid><w:gridCol w:w=\"3000\"/></w:tblGrid><w:tr><w:tc><w:tcPr>{width}</w:tcPr>{TAG}</w:tc></w:tr></w:tbl>{SECTION}");
        let xml = fill(&body, json!({"html": "<table><tr><td>A</td></tr></table>"}));

        assert!(xml.contains(&format!("<w:tblW w:w=\"{expected}\" w:type=\"dxa\"/>")), "{width}: {xml}");
    }
}

#[test]
fn sums_spanned_grid_columns() {
    let body = format!("<w:tbl><w:tblGrid><w:gridCol w:w=\"1000\"/><w:gridCol w:w=\"2000\"/><w:gridCol w:w=\"3000\"/></w:tblGrid><w:tr><w:trPr><w:gridBefore w:val=\"1\"/></w:trPr><w:tc><w:tcPr><w:tcW w:w=\"0\" w:type=\"auto\"/><w:gridSpan w:val=\"2\"/></w:tcPr>{TAG}</w:tc></w:tr></w:tbl>{SECTION}");
    let xml = fill(&body, json!({"html": "<table><tr><td>A</td></tr></table>"}));

    assert!(xml.contains("<w:tblW w:w=\"4784\" w:type=\"dxa\"/>"), "{xml}");
}

#[test]
fn keeps_full_width_in_unknown_containers() {
    let cell = format!("<w:tbl><w:tr><w:tc><w:tcPr><w:tcW w:w=\"0\" w:type=\"auto\"/></w:tcPr>{TAG}</w:tc></w:tr></w:tbl>");
    let text_box = format!("<w:p><w:r><w:drawing><wps:txbx><w:txbxContent>{TAG}</w:txbxContent></wps:txbx></w:drawing></w:r></w:p>");
    for body in [cell, text_box] {
        let xml = fill(&format!("{body}{SECTION}"), json!({"html": "<table style='width:50%'><tr><td>A</td><td>B</td></tr></table>"}));

        assert!(xml.contains("<w:tblW w:w=\"2500\" w:type=\"pct\"/>"), "{xml}");
        assert_eq!(xml.matches("<w:tcW w:w=\"2500\" w:type=\"pct\"/>").count(), 2, "{xml}");
    }
}

#[test]
fn subtracts_default_cell_margins() {
    let body = format!("<w:tbl><w:tblGrid><w:gridCol w:w=\"3000\"/></w:tblGrid><w:tr><w:tc><w:tcPr><w:tcW w:w=\"3000\" w:type=\"dxa\"/></w:tcPr>{TAG}</w:tc></w:tr></w:tbl>{SECTION}");
    let xml = fill(&body, json!({"html": "<table><tr><td>A</td></tr></table>"}));

    assert!(xml.contains("<w:tblW w:w=\"2784\" w:type=\"dxa\"/>"), "{xml}");
}

#[test]
fn reads_default_table_style_margins() {
    let styles = format!("<w:styles {NAMESPACES}><w:style w:type=\"table\" w:default=\"1\" w:styleId=\"TableNormal\"><w:tblPr><w:tblCellMar><w:left w:w=\"50\" w:type=\"dxa\"/><w:right w:w=\"150\" w:type=\"dxa\"/></w:tblCellMar></w:tblPr></w:style></w:styles>");
    let body = format!("<w:tbl><w:tr><w:tc><w:tcPr><w:tcW w:w=\"3000\" w:type=\"dxa\"/></w:tcPr>{TAG}</w:tc></w:tr></w:tbl>{SECTION}");
    let template = PackageTemplate::parse([
        ("word/styles.xml".to_string(), styles.into_bytes()),
        ("word/document.xml".to_string(), document(&body).into_bytes()),
    ], &FillOptions::default()).unwrap();
    let parts = template.render(&json!({"html": "<table><tr><td>A</td></tr></table>"}));
    let xml = String::from_utf8_lossy(&parts.iter().find(|(part, _)| part == "word/document.xml").unwrap().1);

    assert!(xml.contains("<w:tblW w:w=\"2800\" w:type=\"dxa\"/>"), "{xml}");
}

#[test]
fn keeps_rowspan_borders() {
    let xml = render("<table><tr><td rowspan='3' style='border-bottom:2px solid red;border-left:2px solid blue'>A</td><td>B</td></tr><tr><td>C</td></tr><tr><td>D</td></tr></table>");
    let cells = elements(&xml, "w:tc");
    let continued: Vec<_> = cells.iter().filter(|cell| cell.contains("<w:vMerge/>")).collect();

    assert!(!cells[0].contains("w:color=\"FF0000\""), "{xml}");
    assert!(cells[0].contains("<w:left w:val=\"single\" w:sz=\"12\" w:space=\"0\" w:color=\"0000FF\"/>"));
    assert!(continued.iter().all(|cell| cell.contains("w:color=\"0000FF\"")), "{xml}");
    assert!(!continued[0].contains("w:color=\"FF0000\""));
    assert!(continued[1].contains("<w:bottom w:val=\"single\" w:sz=\"12\" w:space=\"0\" w:color=\"FF0000\"/>"));
}
