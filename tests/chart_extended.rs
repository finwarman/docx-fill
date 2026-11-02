mod common;

use std::collections::BTreeMap;
use common::{paragraph, export_chart_fixture};
use docx_fill::{FillOptions, PackageTemplate};
use serde_json::{Value, json};

const CAT: &str = "<cx:strDim type=\"cat\"><cx:lvl ptCount=\"2\"><cx:pt idx=\"0\">France</cx:pt><cx:pt idx=\"1\">Spain</cx:pt></cx:lvl></cx:strDim>";
const GEO: &str = "<cx:geography cultureLanguage=\"en\" cultureRegion=\"US\" attribution=\"Example\"><cx:geoCache provider=\"test-provider\"><cx:binary>AAEC</cx:binary></cx:geoCache></cx:geography>";

fn drawing() -> &'static str {
    r#"<w:p><w:r><w:drawing xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:cx="http://schemas.microsoft.com/office/drawing/2014/chartex" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><wp:inline><wp:extent cx="5486400" cy="3200400"/><wp:docPr id="1" name="Extended chart"/><a:graphic><a:graphicData uri="http://schemas.microsoft.com/office/drawing/2014/chartex"><cx:chart r:id="rIdChart"/></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#
}

fn chart(layout: &str, numeric_type: &str) -> String {
    let axes = if layout == "waterfall" { "<cx:axis id=\"0\"><cx:catScaling gapWidth=\"1\"/><cx:tickLabels/></cx:axis><cx:axis id=\"1\"><cx:valScaling/><cx:majorGridlines/><cx:tickLabels/><cx:numFmt formatCode=\"0.00\" sourceLinked=\"0\"/></cx:axis>" } else { "" };
    let links = if layout == "waterfall" { "<cx:axisId>0</cx:axisId><cx:axisId>1</cx:axisId>" } else { "" };
    let geo = if layout == "regionMap" { GEO } else { "" };
    format!(r#"<cx:chartSpace xmlns:cx="http://schemas.microsoft.com/office/drawing/2014/chartex" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><cx:chartData><cx:data id="7">{CAT}<cx:strDim type="entityId"><cx:lvl ptCount="2"><cx:pt idx="0">FR</cx:pt><cx:pt idx="1">ES</cx:pt></cx:lvl></cx:strDim><cx:numDim type="{numeric_type}"><cx:lvl ptCount="2" formatCode="0.00"><cx:pt idx="0">1</cx:pt><cx:pt idx="1">2</cx:pt></cx:lvl></cx:numDim></cx:data></cx:chartData><cx:chart><cx:title pos="t" align="ctr"><cx:tx><cx:rich><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Revenue</a:t></a:r></a:p></cx:rich></cx:tx></cx:title><cx:plotArea><cx:plotAreaRegion><cx:series layoutId="{layout}" uniqueId="{{D5C87836-4BFB-4617-B29A-BE9E77F7D26B}}"><cx:tx><cx:txData><cx:v>Original</cx:v></cx:txData></cx:tx><cx:spPr><a:solidFill><a:srgbClr val="006699"/></a:solidFill></cx:spPr><cx:dataLabels pos="ctr"><cx:visibility seriesName="0" categoryName="1" value="0"/></cx:dataLabels><cx:dataId val="7"/><cx:layoutPr>{geo}</cx:layoutPr>{links}</cx:series></cx:plotAreaRegion>{axes}</cx:plotArea></cx:chart></cx:chartSpace>"#)
}

fn render(body: &str, chart: &str, data: Value) -> BTreeMap<String, Vec<u8>> {
    let document = format!(r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}<w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440"/></w:sectPr></w:body></w:document>"#);
    let parts = [
        ("[Content_Types].xml".into(), br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/extendedCharts/chart1.xml" ContentType="application/vnd.ms-office.chartex+xml"/></Types>"#.to_vec()),
        ("_rels/.rels".into(), br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#.to_vec()),
        ("word/document.xml".into(), document.into_bytes()),
        ("word/extendedCharts/chart1.xml".into(), chart.as_bytes().to_vec()),
        ("word/_rels/document.xml.rels".into(), br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdChart" Type="http://schemas.microsoft.com/office/2014/relationships/chartEx" Target="extendedCharts/chart1.xml"/></Relationships>"#.to_vec()),
    ];
    PackageTemplate::parse(parts, &FillOptions::default()).unwrap().render(&data).into_iter().map(|(name, bytes)| (name, bytes.into_owned())).collect()
}

fn output_chart(parts: &BTreeMap<String, Vec<u8>>) -> String {
    String::from_utf8(parts.iter().find(|(name, _)| name.contains("/docxFillChart") && name.ends_with(".xml")).expect("filled chart").1.clone()).unwrap()
}


#[test]
fn fills_waterfall_dimensions() {
    let source = chart("waterfall", "val");
    let body = drawing().to_string() + &paragraph(&["{$sales}"]);
    let parts = render(&body, &source, json!({"sales":{"categories":["First","Loss","Gain","Last"],"data":[4,-2,3,5]}}));
    let chart = output_chart(&parts);
    assert!(chart.contains("layoutId=\"waterfall\"") && chart.contains("<cx:axis id=\"1\">"));
    assert!(chart.contains("<cx:numDim type=\"val\"><cx:lvl ptCount=\"4\" formatCode=\"0.00\"><cx:pt idx=\"0\">4</cx:pt><cx:pt idx=\"1\">-2</cx:pt>"));
    assert!(chart.contains("<cx:pt idx=\"3\">Last</cx:pt>") && chart.contains("006699"));
    assert!(String::from_utf8_lossy(&parts["word/_rels/document.xml.rels"]).contains("Type=\"http://schemas.microsoft.com/office/2014/relationships/chartEx\" Target=\"extendedCharts/docxFillChart1.xml\""));
    assert!(String::from_utf8_lossy(&parts["[Content_Types].xml"]).contains("PartName=\"/word/extendedCharts/docxFillChart1.xml\" ContentType=\"application/vnd.ms-office.chartex+xml\""));
    export_chart_fixture("waterfall", &parts);
}

#[test]
fn fills_sunburst_hierarchy() {
    let source = chart("sunburst", "size");
    let body = drawing().to_string() + &paragraph(&["{$sales}"]);
    let parts = render(&body, &source, json!({"sales":{"categories":[{"name":"Drinks","categories":[{"name":"Tea","value":3},{"name":"Coffee","value":5}]},{"name":"Food","categories":[{"name":"Fruit","categories":[{"name":"Apple","value":2}]}]}]}}));
    let chart = output_chart(&parts);
    assert!(chart.contains("layoutId=\"sunburst\"") && chart.contains("<cx:numDim type=\"size\">"));
    assert!(chart.contains("<cx:lvl ptCount=\"3\"><cx:pt idx=\"0\"></cx:pt><cx:pt idx=\"1\"></cx:pt><cx:pt idx=\"2\">Apple</cx:pt></cx:lvl>"));
    assert!(chart.contains("<cx:lvl ptCount=\"3\"><cx:pt idx=\"0\">Drinks</cx:pt><cx:pt idx=\"1\">Drinks</cx:pt><cx:pt idx=\"2\">Food</cx:pt></cx:lvl>"));
    export_chart_fixture("sunburst", &parts);
}

#[test]
fn preserves_known_map_geometry() {
    let source = chart("regionMap", "colorVal");
    let body = drawing().to_string() + &paragraph(&["{$sales}"]);
    let parts = render(&body, &source, json!({"sales":{"categories":["France","Spain"],"series":[{"data":[7,8]}]}}));
    let chart = output_chart(&parts);
    assert!(chart.contains(GEO));
    assert!(chart.contains("<cx:pt idx=\"0\">FR</cx:pt>") && chart.contains("<cx:pt idx=\"1\">ES</cx:pt>"));
    assert!(chart.contains("<cx:numDim type=\"colorVal\"><cx:lvl ptCount=\"2\" formatCode=\"0.00\"><cx:pt idx=\"0\">7</cx:pt><cx:pt idx=\"1\">8</cx:pt>"));
}

#[test]
fn keeps_unknown_map_regions() {
    let source = chart("regionMap", "colorVal");
    let body = drawing().to_string() + &paragraph(&["{$sales}"]);
    let parts = render(&body, &source, json!({"sales":{"categories":["France","Germany"],"series":[{"data":[7,8]}]}}));
    assert!(!parts.keys().any(|name| name.contains("/docxFillChart")));
    assert_eq!(parts["word/extendedCharts/chart1.xml"], source.as_bytes());
}

#[test]
fn reads_extended_title_tags() {
    let source = chart("waterfall", "val").replace("Revenue", "Revenue {$sales}");
    let parts = render(drawing(), &source, json!({"sales":{"categories":["France","Spain"],"series":[{"title":"Changed","data":[7,null]}]}}));
    let chart = output_chart(&parts);
    assert!(!chart.contains("{$sales}"));
    assert!(chart.contains("<cx:tx><cx:txData><cx:v>Changed</cx:v></cx:txData></cx:tx>"));
    assert!(chart.contains("ptCount=\"2\" formatCode=\"0.00\"><cx:pt idx=\"0\">7</cx:pt></cx:lvl>"));
}

#[test]
fn repeats_extended_chart_values() {
    let source = chart("waterfall", "val");
    let body = paragraph(&["{#rows}"]) + drawing() + &paragraph(&["{$sales}"]) + &paragraph(&["{/rows}"]);
    let parts = render(&body, &source, json!({"rows":[{"sales":{"categories":["A"],"data":[1]}},{"sales":{"categories":["B"],"data":[2]}}]}));
    let charts = parts.iter().filter(|(name, _)| name.contains("/docxFillChart")).map(|(_, bytes)| String::from_utf8_lossy(bytes)).collect::<Vec<_>>();
    assert_eq!(charts.len(), 2);
    assert!(charts[0].contains(">A</cx:pt>") && charts[1].contains(">B</cx:pt>"));
}
