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
    render_with_relations(body, chart, r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdChart" Type="http://schemas.microsoft.com/office/2014/relationships/chartEx" Target="extendedCharts/chart1.xml"/></Relationships>"#, data)
}

fn render_with_relations(body: &str, chart: &str, relations: &str, data: Value) -> BTreeMap<String, Vec<u8>> {
    let document = format!(r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}<w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440"/></w:sectPr></w:body></w:document>"#);
    let parts = [
        ("[Content_Types].xml".into(), br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/extendedCharts/chart1.xml" ContentType="application/vnd.ms-office.chartex+xml"/></Types>"#.to_vec()),
        ("_rels/.rels".into(), br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#.to_vec()),
        ("word/document.xml".into(), document.into_bytes()),
        ("word/extendedCharts/chart1.xml".into(), chart.as_bytes().to_vec()),
        ("word/_rels/document.xml.rels".into(), relations.as_bytes().to_vec()),
        ("word/charts/fallback.xml".into(), br#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart><c:plotArea/></c:chart></c:chartSpace>"#.to_vec()),
        ("word/media/fallback.png".into(), b"opaque fallback image".to_vec()),
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
fn fills_treemap_flat_values() {
    let source = chart("treemap", "size");
    let body = drawing().to_string() + &paragraph(&["{$sales}"]);
    let parts = render(&body, &source, json!({"sales":{"categories":["A","B"],"series":[{"data":[7,8]}]}}));
    let chart = output_chart(&parts);
    assert!(chart.contains("layoutId=\"treemap\"") && chart.contains("<cx:numDim type=\"size\">"));
    assert!(chart.contains("<cx:pt idx=\"0\">7</cx:pt><cx:pt idx=\"1\">8</cx:pt>"));
}

#[test]
fn preserves_waterfall_subtotals() {
    let source = chart("waterfall", "val").replace("<cx:layoutPr></cx:layoutPr>", "<cx:layoutPr><cx:subtotals><cx:idx val=\"1\"/></cx:subtotals></cx:layoutPr>");
    let body = drawing().to_string() + &paragraph(&["{$sales}"]);
    let parts = render(&body, &source, json!({"sales":{"categories":["A","Total"],"data":[7,7]}}));
    assert!(output_chart(&parts).contains("<cx:subtotals><cx:idx val=\"1\"/></cx:subtotals>"));
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

#[test]
fn accepts_chart_namespace_aliases() {
    let source = chart("waterfall", "val").replace("cx:", "ex:").replace("xmlns:cx=", "xmlns:ex=").replace("a:", "draw:").replace("xmlns:a=", "xmlns:draw=");
    let body = drawing().replace("cx:", "ex:").replace("xmlns:cx=", "xmlns:ex=").replace("r:id", "links:id").replace("xmlns:r=", "xmlns:links=") + &paragraph(&["{$sales}"]);
    let relations = r#"<p:Relationships xmlns:p="http://schemas.openxmlformats.org/package/2006/relationships"><p:Relationship Id="rIdChart" Type="http://schemas.microsoft.com/office/2014/relationships/chartEx" Target="extendedCharts/chart1.xml"/></p:Relationships>"#;
    let parts = render_with_relations(&body, &source, relations, json!({"sales":{"categories":["Aliased"],"data":[8]}}));
    assert!(output_chart(&parts).contains(">Aliased</cx:pt>"));
    assert!(String::from_utf8_lossy(&parts["word/_rels/document.xml.rels"]).contains("Target=\"extendedCharts/docxFillChart1.xml\""));
}

#[test]
fn accepts_default_chart_namespace() {
    let source = chart("waterfall", "val").replace("xmlns:cx=", "xmlns=").replace("cx:", "");
    let body = drawing().replace("<cx:chart", "<chart xmlns=\"http://schemas.microsoft.com/office/drawing/2014/chartex\"") + &paragraph(&["{$sales}"]);
    let parts = render(&body, &source, json!({"sales":{"categories":["Default"],"data":[8]}}));
    assert!(output_chart(&parts).contains(">Default</cx:pt>"));
}

#[test]
fn preserves_foreign_namespaces() {
    let extension = r#"<ex:extLst><ex:ext uri="test"><cx:series marker="untouched"><cx:data>foreign</cx:data></cx:series><other:series xmlns:other="urn:other">unchanged</other:series></ex:ext></ex:extLst>"#;
    let source = chart("waterfall", "val").replace("cx:", "ex:").replace("xmlns:cx=", "xmlns:ex=").replace("<ex:chartSpace ", "<ex:chartSpace xmlns:cx=\"urn:foreign\" xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\" mc:Ignorable=\"cx\" ").replace("</ex:chartSpace>", &format!("{extension}</ex:chartSpace>"));
    let foreign_ref = r#"<other:chart xmlns:other="urn:other" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" r:id="unrelated"/>"#;
    let body = drawing().replace("</w:p>", &format!("{foreign_ref}</w:p>")) + &paragraph(&["{$sales}"]);
    let parts = render(&body, &source, json!({"sales":{"categories":["Bound"],"data":[8]}}));
    let chart = output_chart(&parts);
    assert!(chart.contains("marker=\"untouched\"") && chart.contains("urn:foreign") && chart.contains(">foreign</docxFillNs"));
    assert!(chart.contains("mc:Ignorable=\"docxFillNs"));
    assert!(chart.contains(r#"<other:series xmlns:other="urn:other">unchanged</other:series>"#));
    assert!(chart.contains(">Bound</cx:pt>"));
    assert!(String::from_utf8_lossy(&parts["word/document.xml"]).contains(foreign_ref));
}

#[test]
fn binds_supported_chart_choice() {
    let selected = drawing().trim_start_matches("<w:p><w:r>").trim_end_matches("</w:r></w:p>").to_string();
    let fallback = selected.replace("xmlns:cx=\"http://schemas.microsoft.com/office/drawing/2014/chartex\"", "xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\"").replace("<cx:chart", "<c:chart").replace("rIdChart", "rIdFallback");
    let alternate = format!(r#"<alt:AlternateContent xmlns:alt="http://schemas.openxmlformats.org/markup-compatibility/2006"><alt:Choice xmlns:extended="http://schemas.microsoft.com/office/drawing/2014/chartex" Requires="extended">{selected}</alt:Choice><alt:Fallback>{fallback}</alt:Fallback></alt:AlternateContent>"#);
    let relations = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdChart" Type="http://schemas.microsoft.com/office/2014/relationships/chartEx" Target="extendedCharts/chart1.xml"/><Relationship Id="rIdFallback" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart" Target="charts/fallback.xml"/></Relationships>"#;
    for body in [format!("<w:p><w:r>{alternate}</w:r></w:p>{}", paragraph(&["{$sales}"])), format!("<w:p><w:r>{alternate}</w:r><w:r><w:t>{{$sales}}</w:t></w:r></w:p>")] {
        let parts = render_with_relations(&body, &chart("waterfall", "val"), relations, json!({"sales":{"categories":["Chosen"],"data":[8]}}));
        assert!(output_chart(&parts).contains(">Chosen</cx:pt>"));
        let document = String::from_utf8_lossy(&parts["word/document.xml"]);
        assert!(document.contains("<c:chart r:id=\"rIdFallback\"/>") && document.contains("<alt:Fallback>"));
        assert_eq!(parts["word/media/fallback.png"], b"opaque fallback image");
        assert!(String::from_utf8_lossy(&parts["word/charts/fallback.xml"]).contains("<c:plotArea/>"));
    }
}

#[test]
fn keeps_chart_fallback_picture() {
    let selected = drawing().trim_start_matches("<w:p><w:r>").trim_end_matches("</w:r></w:p>").to_string();
    let picture = r#"<a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:nvPicPr><pic:cNvPr id="2" name="Fallback"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="rIdPreview"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="5486400" cy="3200400"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData>"#;
    let start = selected.find("<a:graphicData").unwrap();
    let end = selected.find("</a:graphicData>").unwrap() + "</a:graphicData>".len();
    let mut fallback = selected.clone();
    fallback.replace_range(start..end, picture);
    let alternate = format!(r#"<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice xmlns:cx="http://schemas.microsoft.com/office/drawing/2014/chartex" Requires="cx">{selected}</mc:Choice><mc:Fallback>{fallback}</mc:Fallback></mc:AlternateContent>"#);
    let body = format!("<w:p><w:r>{alternate}</w:r></w:p>{}", paragraph(&["{$sales}"]));
    let relations = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdChart" Type="http://schemas.microsoft.com/office/2014/relationships/chartEx" Target="extendedCharts/chart1.xml"/><Relationship Id="rIdPreview" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/fallback.png"/></Relationships>"#;
    let parts = render_with_relations(&body, &chart("waterfall", "val"), relations, json!({"sales":{"categories":["Picture"],"data":[8]}}));
    assert!(output_chart(&parts).contains(">Picture</cx:pt>"));
    assert!(String::from_utf8_lossy(&parts["word/document.xml"]).contains(picture));
    assert_eq!(parts["word/media/fallback.png"], b"opaque fallback image");
}
