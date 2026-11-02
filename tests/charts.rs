mod common;

use std::collections::BTreeMap;
use common::{document, paragraph};
use docx_fill::{FillOptions, PackageTemplate, TagType};
use serde_json::{Value, json};

const CHART: &str = r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><c:chart><c:title><c:tx><c:rich><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Sales</a:t></a:r></a:p></c:rich></c:tx></c:title><c:plotArea><c:layout/><c:barChart><c:barDir val="col"/><c:grouping val="clustered"/><c:ser><c:idx val="0"/><c:order val="0"/><c:tx><c:strRef><c:f>Sheet1!B1</c:f><c:strCache><c:ptCount val="1"/><c:pt idx="0"><c:v>Original</c:v></c:pt></c:strCache></c:strRef></c:tx><c:spPr><a:solidFill><a:srgbClr val="00AA00"/></a:solidFill></c:spPr><c:cat><c:strRef><c:f>Sheet1!A2:A3</c:f><c:strCache><c:ptCount val="2"/><c:pt idx="0"><c:v>Old</c:v></c:pt></c:strCache></c:strRef></c:cat><c:val><c:numRef><c:f>Sheet1!B2:B3</c:f><c:numCache><c:formatCode>0.00</c:formatCode><c:ptCount val="2"/><c:pt idx="0"><c:v>9</c:v></c:pt></c:numCache></c:numRef></c:val></c:ser><c:gapWidth val="150"/><c:axId val="1"/><c:axId val="2"/></c:barChart><c:catAx><c:axId val="1"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:axPos val="b"/><c:crossAx val="2"/></c:catAx><c:valAx><c:axId val="2"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:axPos val="l"/><c:crossAx val="1"/></c:valAx></c:plotArea></c:chart><c:externalData r:id="rIdWorkbook"><c:autoUpdate val="1"/></c:externalData></c:chartSpace>"#;

fn drawing() -> &'static str {
    r#"<w:p><w:r><w:drawing xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><wp:inline><wp:extent cx="914400" cy="914400"/><wp:docPr id="1" name="Chart"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart r:id="rIdChart"/></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#
}

fn template(body: &str, chart: &str, options: &Value) -> PackageTemplate {
    let parts = [
        ("word/document.xml".into(), document(body).into_bytes()),
        ("word/charts/chart1.xml".into(), chart.as_bytes().to_vec()),
        ("word/charts/_rels/chart1.xml.rels".into(), br#"<Relationships><Relationship Id="rIdWorkbook" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/package" Target="../embeddings/data.xlsx"/></Relationships>"#.to_vec()),
        ("word/embeddings/data.xlsx".into(), b"original workbook".to_vec()),
        ("word/_rels/document.xml.rels".into(), br#"<Relationships><Relationship Id="rIdChart" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart" Target="charts/chart1.xml"/></Relationships>"#.to_vec()),
        ("[Content_Types].xml".into(), br#"<Types><Override PartName="/word/charts/chart1.xml" ContentType="application/vnd.openxmlformats-officedocument.drawingml.chart+xml"/></Types>"#.to_vec()),
    ];
    let options = FillOptions::default().read_json(options).unwrap();
    PackageTemplate::parse(parts, &options).unwrap()
}

fn render(template: &PackageTemplate, data: Value) -> BTreeMap<String, Vec<u8>> {
    template.render(&data).into_iter().map(|(name, bytes)| (name, bytes.into_owned())).collect()
}

fn chart_xml(parts: &BTreeMap<String, Vec<u8>>) -> Vec<String> {
    parts.iter().filter(|(path, _)| path.contains("/docxFillChart") && path.ends_with(".xml")).map(|(_, bytes)| String::from_utf8(bytes.clone()).unwrap()).collect()
}

fn category_chart(name: &str, properties: &str) -> String {
    let start = CHART.find("<c:ser>").unwrap();
    let end = CHART.find("</c:ser>").unwrap() + "</c:ser>".len();
    let mut tail = String::new();
    let mut axes = String::new();
    if name == "c:doughnutChart" { tail.push_str("<c:holeSize val=\"65\"/>"); }
    if !matches!(name, "c:pieChart" | "c:pie3DChart" | "c:ofPieChart" | "c:doughnutChart") {
        tail.push_str("<c:axId val=\"1\"/><c:axId val=\"2\"/>");
        let start = CHART.find("<c:catAx>").unwrap();
        let end = CHART.find("</c:valAx>").unwrap() + "</c:valAx>".len();
        axes.push_str(&CHART[start..end]);
    }
    if matches!(name, "c:surfaceChart" | "c:surface3DChart") {
        tail.push_str("<c:axId val=\"3\"/>");
        axes.push_str("<c:serAx><c:axId val=\"3\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:axPos val=\"b\"/><c:crossAx val=\"1\"/></c:serAx>");
    }
    format!(r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><c:chart><c:view3D><c:rotX val="15"/><c:rotY val="20"/></c:view3D><c:plotArea><{name}>{properties}{}{tail}</{name}>{axes}</c:plotArea></c:chart></c:chartSpace>"#, &CHART[start..end])
}

#[test]
fn fills_category_chart_types() {
    let cases = [
        ("c:bar3DChart", "<c:barDir val=\"col\"/><c:grouping val=\"clustered\"/>"),
        ("c:lineChart", "<c:grouping val=\"standard\"/>"),
        ("c:line3DChart", "<c:grouping val=\"standard\"/>"),
        ("c:areaChart", "<c:grouping val=\"standard\"/>"),
        ("c:area3DChart", "<c:grouping val=\"standard\"/>"),
        ("c:radarChart", "<c:radarStyle val=\"standard\"/>"),
        ("c:pieChart", "<c:varyColors val=\"1\"/>"),
        ("c:pie3DChart", "<c:varyColors val=\"1\"/>"),
        ("c:ofPieChart", "<c:ofPieType val=\"pie\"/><c:varyColors val=\"1\"/>"),
        ("c:doughnutChart", "<c:varyColors val=\"1\"/>"),
        ("c:surfaceChart", "<c:wireframe val=\"0\"/>"),
        ("c:surface3DChart", "<c:wireframe val=\"0\"/>"),
    ];
    for (name, properties) in cases {
        let source = category_chart(name, properties);
        let body = drawing().to_string() + &paragraph(&["{$sales}"]);
        let parts = render(&template(&body, &source, &json!({})), json!({"sales":{"categories":["One","Two"],"series":[{"title":"Updated","data":[3,null]}]}}));
        let charts = chart_xml(&parts);
        assert_eq!(charts.len(), 1, "{name}");
        let chart = &charts[0];
        assert!(chart.contains(properties) && chart.contains("<c:rotY val=\"20\"/>"), "{name}");
        assert!(chart.contains("<c:tx><c:v>Updated</c:v></c:tx>") && chart.contains(">Two</c:v>"), "{name}");
        assert!(chart.contains("<c:ptCount val=\"2\"/>") && chart.contains("<c:pt idx=\"0\"><c:v>3</c:v></c:pt>"), "{name}");
        assert!(chart.contains("00AA00") && chart.contains("<c:formatCode>0.00</c:formatCode>"), "{name}");
        if name == "c:doughnutChart" { assert!(chart.contains("<c:holeSize val=\"65\"/>")); }
    }
}

#[test]
fn preserves_line_marker_options() {
    let source = category_chart("c:lineChart", "<c:grouping val=\"standard\"/>")
        .replace("</c:spPr>", "</c:spPr><c:marker><c:symbol val=\"diamond\"/><c:size val=\"8\"/></c:marker>")
        .replace("</c:val>", "</c:val><c:smooth val=\"1\"/>");
    let body = drawing().to_string() + &paragraph(&["{$sales}"]);
    let parts = render(&template(&body, &source, &json!({})), json!({"sales":{"categories":["A"],"series":[{"data":[7]}]}}));
    let chart = &chart_xml(&parts)[0];
    assert!(chart.contains("<c:symbol val=\"diamond\"/>") && chart.contains("<c:smooth val=\"1\"/>"));
    assert!(chart.find("<c:marker>").unwrap() < chart.find("<c:cat>").unwrap());
    assert!(chart.find("</c:val>").unwrap() < chart.find("<c:smooth").unwrap());
}

#[test]
fn pie_requires_one_series() {
    let source = category_chart("c:pieChart", "<c:varyColors val=\"1\"/>");
    let body = drawing().to_string() + &paragraph(&["{$sales}"]);
    let parts = render(&template(&body, &source, &json!({})), json!({"sales":{"categories":["A"],"series":[{"data":[7]},{"data":[8]}]}}));
    assert!(chart_xml(&parts).is_empty());
}

fn two_charts(body: &str) -> Result<PackageTemplate, docx_fill::FillError> {
    let parts = [
        ("word/document.xml".into(), document(body).into_bytes()),
        ("word/charts/chart1.xml".into(), CHART.replace("Sales", "First source").into_bytes()),
        ("word/charts/chart2.xml".into(), CHART.replace("Sales", "Second source").into_bytes()),
        ("word/_rels/document.xml.rels".into(), br#"<Relationships><Relationship Id="rIdChart" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart" Target="charts/chart1.xml"/><Relationship Id="rIdSecond" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart" Target="charts/chart2.xml"/></Relationships>"#.to_vec()),
    ];
    PackageTemplate::parse(parts, &FillOptions::default())
}

#[test]
fn rejects_ambiguous_chart() {
    let body = drawing().trim_end_matches("</w:p>").to_string()
        + drawing().replace("rIdChart", "rIdSecond").trim_start_matches("<w:p>")
        + &paragraph(&["{$sales}"]);

    let error = two_charts(&body).expect_err("two charts cannot share a following tag");
    assert!(error.message.contains("ambiguous"), "{}", error.message);
}

#[test]
fn binds_tag_between_drawings() {
    let body = drawing().trim_end_matches("</w:p>").to_string()
        + "<w:r><w:t>{$sales}</w:t></w:r>"
        + drawing().replace("rIdChart", "rIdSecond").trim_start_matches("<w:p>");
    let parts = render(&two_charts(&body).unwrap(), json!({"sales":{"categories":["Updated"],"series":[{"data":[7]}]}}));
    let charts = chart_xml(&parts);

    assert_eq!(charts.len(), 1);
    assert!(charts[0].contains("First source") && charts[0].contains(">Updated</c:v>"));
    let body = String::from_utf8_lossy(&parts["word/document.xml"]);
    assert!(body.contains("r:id=\"rIdDocxFillChart1\"") && body.contains("r:id=\"rIdSecond\""));
    assert!(!body.contains("r:id=\"rIdChart\""));
}

#[test]
fn rejects_tag_before_a_chart() {
    let body = drawing().to_string()
        + "<w:p><w:r><w:t>{$sales}</w:t></w:r>"
        + drawing().replace("rIdChart", "rIdSecond").trim_start_matches("<w:p>");

    assert!(two_charts(&body).is_err(), "a tag before its drawing must not edit the previous paragraph's chart");
}

#[test]
fn accepts_paired_chart_ref() {
    let body = drawing().replace("<c:chart r:id=\"rIdChart\"/>", "<c:chart r:id=\"rIdChart\"></c:chart>") + &paragraph(&["{$sales}"]);
    let parts = render(&template(&body, CHART, &json!({})), json!({"sales":{"categories":["A"],"series":[{"data":[7]}]}}));

    assert_eq!(chart_xml(&parts).len(), 1);
}

#[test]
fn fills_categories_and_series() {
    let body = drawing().to_string() + &paragraph(&["{$sales}"]);
    let template = template(&body, CHART, &json!({}));
    assert_eq!(template.tags()[0].1[0].kind, TagType::Chart);
    let parts = render(&template, json!({"sales":{"categories":["A & B","Two","Three"],"series":[{"title":"New <title>","data":[1,null,-2.5]},{"data":[4,5,6]}]}}));
    let charts = chart_xml(&parts);
    let chart = &charts[0];

    assert_eq!(chart.matches("<c:ser>").count(), 2);
    assert!(chart.contains("<c:tx><c:v>New &lt;title&gt;</c:v></c:tx>"));
    assert!(chart.contains("<c:tx><c:v>Original</c:v></c:tx>"));
    assert!(chart.contains("<c:pt idx=\"0\"><c:v>A &amp; B</c:v></c:pt>"));
    assert!(chart.contains("<c:pt idx=\"2\"><c:v>-2.5</c:v></c:pt>"));
    assert!(chart.contains("<c:idx val=\"1\"/>") && chart.contains("<c:ptCount val=\"3\"/>"));
    assert!(chart.contains("val=\"00AA00\"") && chart.contains("<c:gapWidth val=\"150\"/>") && chart.contains("<c:catAx>"));
    assert!(chart.contains("<c:formatCode>0.00</c:formatCode>"));
    assert!(!chart.contains("<c:f>") && !chart.contains("<c:externalData"));
    assert_eq!(parts["word/charts/chart1.xml"], CHART.as_bytes());
    assert_eq!(parts["word/embeddings/data.xlsx"], b"original workbook");
    assert!(String::from_utf8_lossy(&parts["word/document.xml"]).contains("r:id=\"rIdDocxFillChart1\""));
    assert!(!String::from_utf8_lossy(&parts["word/document.xml"]).contains("<w:t></w:t>"));
    assert!(String::from_utf8_lossy(&parts["[Content_Types].xml"]).contains("/word/charts/docxFillChart1.xml"));
}

#[test]
fn clones_charts_for_loop_scopes() {
    let body = paragraph(&["{#rows}"]) + drawing() + &paragraph(&["{$sales}"]) + &paragraph(&["{/rows}"]);
    let template = template(&body, CHART, &json!({}));
    let parts = render(&template, json!({"rows":[{"sales":{"categories":["First"],"series":[{"data":[1]}]}},{"sales":{"categories":["Second"],"series":[{"data":[2]}]}}]}));
    let charts = chart_xml(&parts);

    assert_eq!(charts.len(), 2);
    assert!(charts[0].contains(">First</c:v>") && charts[1].contains(">Second</c:v>"));
    let body = String::from_utf8_lossy(&parts["word/document.xml"]);
    assert!(body.contains("rIdDocxFillChart1") && body.contains("rIdDocxFillChart2"));
    let rels = String::from_utf8_lossy(&parts["word/_rels/document.xml.rels"]);
    assert!(rels.contains("charts/docxFillChart1.xml") && rels.contains("charts/docxFillChart2.xml"));
}

#[test]
fn reads_split_title_tags() {
    let chart = CHART.replace("Sales", "Sales [[ $sa</a:t></a:r><a:r><a:t>les ]]");
    let template = template(drawing(), &chart, &json!({"delimiters":{"start":"[[","end":"]]"}}));
    let parts = render(&template, json!({"sales":{"categories":["Title tag"],"series":[{"data":[7]}]}}));
    let chart = &chart_xml(&parts)[0];

    assert!(chart.contains(">Title tag</c:v>"));
    assert!(chart.contains("<a:t>Sales </a:t>") && !chart.contains("[["));
    assert!(!String::from_utf8_lossy(&parts["word/document.xml"]).contains("[["));
}

#[test]
fn reads_inline_chart_tag() {
    let body = drawing().replace("</w:p>", "<w:r><w:t>{$sales} units</w:t></w:r></w:p>");
    let parts = render(&template(&body, CHART, &json!({})), json!({"sales":{"categories":["Inline"],"series":[{"data":[7]}]}}));
    assert_eq!(chart_xml(&parts).len(), 1);
    assert!(String::from_utf8_lossy(&parts["word/document.xml"]).contains(" units"));
}

#[test]
fn keeps_invalid_or_missing_data() {
    let body = drawing().to_string() + &paragraph(&["{$sales}"]);
    let template = template(&body, CHART, &json!({}));
    for data in [json!({}), json!({"sales":{"categories":["A"],"series":[{"data":[1,2]}]}}), json!({"sales":{"categories":["A"],"series":[{"data":["invalid"]}]}})] {
        let parts = render(&template, data);
        assert!(chart_xml(&parts).is_empty());
        assert!(String::from_utf8_lossy(&parts["word/document.xml"]).contains("r:id=\"rIdChart\""));
        assert_eq!(parts["word/charts/chart1.xml"], CHART.as_bytes());
    }
}

#[test]
fn keeps_unknown_chart_types() {
    let chart = CHART.replace("barChart", "stockChart");
    let body = drawing().to_string() + &paragraph(&["{$sales}"]);
    let parts = render(&template(&body, &chart, &json!({})), json!({"sales":{"categories":["A"],"series":[{"data":[1]}]}}));
    assert!(chart_xml(&parts).is_empty());
    assert_eq!(parts["word/charts/chart1.xml"], chart.as_bytes());
}

#[test]
fn requires_a_nearby_chart() {
    let parts = [("word/document.xml".into(), document(&paragraph(&["{$sales}"])).into_bytes())];
    assert!(PackageTemplate::parse(parts, &FillOptions::default()).is_err());
}

#[test]
fn respects_extension_flag() {
    let body = drawing().to_string() + &paragraph(&["{$sales}"]);
    let parts = render(&template(&body, CHART, &json!({"extensions":{"charts":false}})), json!({"sales":{"categories":["A"],"series":[{"data":[1]}]}}));
    assert!(chart_xml(&parts).is_empty());
}

#[test]
fn avoids_relationship_overlap() {
    let body = drawing().to_string() + &paragraph(&["{$sales}"]);
    let parts = [
        ("word/document.xml".into(), document(&body).into_bytes()),
        ("word/charts/chart1.xml".into(), CHART.as_bytes().to_vec()),
        ("word/_rels/document.xml.rels".into(), br#"<Relationships><Relationship Id="rIdChart" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart" Target="charts/chart1.xml"/><Relationship Id="rIdDocxFillChart1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/picture.png"/></Relationships>"#.to_vec()),
    ];
    let template = PackageTemplate::parse(parts, &FillOptions::default()).unwrap();
    let parts = render(&template, json!({"sales":{"categories":["A"],"series":[{"data":[1]}]}}));
    assert!(String::from_utf8_lossy(&parts["word/document.xml"]).contains("r:id=\"rIdDocxFillChart2\""));
    assert!(parts.contains_key("word/charts/docxFillChart2.xml"));
}

#[test]
fn clears_title_tag_when_unfilled() {
    let chart = CHART.replace("Sales", "Sales {$sales}");
    let parts = render(&template(drawing(), &chart, &json!({})), json!({}));

    assert!(chart_xml(&parts).is_empty());
    assert!(!String::from_utf8_lossy(&parts["word/charts/chart1.xml"]).contains("{$sales}"));
}

#[test]
fn reads_value_format_from_values() {
    let chart = CHART.replace("<c:cat><c:strRef><c:f>Sheet1!A2:A3</c:f><c:strCache>", "<c:cat><c:numRef><c:f>Sheet1!A2:A3</c:f><c:numCache><c:formatCode>m/d/yyyy</c:formatCode>").replace("</c:strCache></c:strRef></c:cat>", "</c:numCache></c:numRef></c:cat>");
    let body = drawing().to_string() + &paragraph(&["{$sales}"]);
    let parts = render(&template(&body, &chart, &json!({})), json!({"sales":{"categories":[null],"series":[{"title":null,"data":[1]}]}}));
    let chart = &chart_xml(&parts)[0];

    assert!(chart.contains("<c:numLit><c:formatCode>0.00</c:formatCode>"));
    assert!(chart.contains("<c:tx><c:v>Original</c:v></c:tx>"));
}

#[test]
fn gives_added_series_their_own_style() {
    let chart = CHART.replace("<c:spPr>", "<c:extLst><c:ext uri=\"u\"/></c:extLst><c:spPr>").replace("</c:val></c:ser>", "</c:val><c:extLst><c:ext uri=\"id\"/></c:extLst></c:ser>");
    let body = drawing().to_string() + &paragraph(&["{$sales}"]);
    let parts = render(&template(&body, &chart, &json!({})), json!({"sales":{"categories":["A"],"series":[{"data":[1]},{"data":[2]}]}}));
    let chart = &chart_xml(&parts)[0];

    assert_eq!(chart.matches("00AA00").count(), 1);
    assert_eq!(chart.matches("uri=\"id\"").count(), 1);
}

#[test]
fn skips_empty_series_templates() {
    let chart = CHART.replace("<c:gapWidth", "<c:ser/><c:gapWidth");
    let body = drawing().to_string() + &paragraph(&["{$sales}"]);
    let parts = render(&template(&body, &chart, &json!({})), json!({"sales":{"categories":["A"],"series":[{"data":[1]},{"data":[2]}]}}));

    assert_eq!(chart_xml(&parts)[0].matches("<c:ser>").count(), 2);
}
