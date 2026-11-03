#[path = "common/spreadsheet.rs"]
mod common;
use common::*;
use docx_fill::{FillHooks, FillOptions, Image, ImageProperties, WorkbookTemplate};
use serde_json::{Value, json};
use std::sync::Arc;
struct Images;
impl FillHooks for Images {
    fn image(&mut self, _part: &str, value: &Value) -> Option<Image> {
        value.as_str().map(|name| Image {
            key: name.into(),
            bytes: Arc::from(include_bytes!("fixtures/spreadsheet/pixel.png").as_slice()),
            extension: "png".into(),
            width: 60.0,
            height: 30.0,
        })
    }
    fn image_properties(&mut self, _part: &str, _value: &Value, _image: &Image) -> ImageProperties {
        ImageProperties::default()
            .alt("Spreadsheet picture")
            .rotate(30.0)
            .unwrap()
    }
}
#[test]
fn inserts_native_spreadsheet_images_and_reuses_binary_media() {
    let rows = format!(
        r#"<row r="1">{}{}</row>"#,
        inline("A1", "{#items}{%image}"),
        inline("B1", "{/items}")
    );
    let template = WorkbookTemplate::parse(package(&rows), &FillOptions::default()).unwrap();
    let filled = template
        .render_with(
            &json!({"items":[{"image":"logo"},{"image":"logo"}]}),
            &mut Images,
        )
        .unwrap();
    let drawings: Vec<_> = filled
        .iter()
        .filter(|(path, _)| path.starts_with("xl/drawings/") && path.ends_with(".xml"))
        .collect();
    assert_eq!(drawings.len(), 1);
    let drawing = std::str::from_utf8(&drawings[0].1).unwrap();
    assert_eq!(drawing.matches("<xdr:oneCellAnchor ").count(), 2);
    assert!(drawing.contains("<xdr:row>0</xdr:row>"));
    assert!(drawing.contains("<xdr:row>1</xdr:row>"));
    assert!(drawing.contains("Spreadsheet picture"));
    assert_eq!(
        filled
            .iter()
            .filter(|(p, _)| p.starts_with("xl/media/") && p.ends_with(".png"))
            .count(),
        1
    );
}
#[test]
fn fills_headers_and_drawing_text_without_changing_geometry() {
    let mut parts = package(&format!(r#"<row r="1">{}</row>"#, inline("A1", "Value")));
    let sheet=String::from_utf8(parts[4].1.clone()).unwrap().replace("</worksheet>",r#"<headerFooter><oddHeader>&amp;C{name} &amp;P</oddHeader></headerFooter><drawing xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" r:id="draw"/></worksheet>"#);
    replace(&mut parts, "xl/worksheets/sheet1.xml", sheet);
    parts.push(("xl/worksheets/_rels/sheet1.xml.rels".into(),format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="draw" Type="{REL}/drawing" Target="../drawings/shape.xml"/></Relationships>"#).into_bytes()));
    parts.push(("xl/drawings/shape.xml".into(),r#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><xdr:oneCellAnchor><xdr:from><xdr:col>2</xdr:col><xdr:row>4</xdr:row></xdr:from><xdr:sp><xdr:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr b="1"/><a:t>Hello </a:t></a:r><a:r><a:rPr i="1"/><a:t>{na</a:t></a:r><a:r><a:t>me}</a:t></a:r><a:r><a:t>!</a:t></a:r></a:p></xdr:txBody></xdr:sp><xdr:clientData/></xdr:oneCellAnchor></xdr:wsDr>"#.as_bytes().to_vec()));
    let template = WorkbookTemplate::parse(parts, &FillOptions::default()).unwrap();
    let filled = template.render(&json!({"name":"A & B"})).unwrap();
    assert!(xml(&filled, "xl/worksheets/sheet1.xml").contains("&amp;CA &amp;&amp; B &amp;P"));
    let drawing = xml(&filled, "xl/drawings/shape.xml");
    assert!(drawing.contains("A &amp; B"));
    assert!(drawing.contains("b=\"1\""));
    assert!(drawing.contains("<xdr:row>4</xdr:row>"));
    assert!(!drawing.contains("{na"));
}
#[test]
fn chart_caches_point_at_generated_worksheet_values() {
    let mut parts = package(&format!(r#"<row r="1">{}</row>"#, inline("A1", "{$sales}")));
    let sheet=String::from_utf8(parts[4].1.clone()).unwrap().replace("</worksheet>",r#"<drawing xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" r:id="draw"/></worksheet>"#);
    replace(&mut parts, "xl/worksheets/sheet1.xml", sheet);
    parts.push(("xl/worksheets/_rels/sheet1.xml.rels".into(),format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="draw" Type="{REL}/drawing" Target="../drawings/chart.xml"/></Relationships>"#).into_bytes()));
    parts.push(("xl/drawings/chart.xml".into(),format!(r#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:r="{REL}"><xdr:oneCellAnchor><xdr:from><xdr:col>0</xdr:col><xdr:row>2</xdr:row></xdr:from><xdr:graphicFrame><c:chart r:id="chart"/></xdr:graphicFrame></xdr:oneCellAnchor></xdr:wsDr>"#).into_bytes()));
    parts.push(("xl/drawings/_rels/chart.xml.rels".into(),format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="chart" Type="{REL}/chart" Target="../charts/chart1.xml"/></Relationships>"#).into_bytes()));
    parts.push(("xl/charts/chart1.xml".into(),r#"<chartSpace xmlns="http://schemas.openxmlformats.org/drawingml/2006/chart"><chart><plotArea><barChart><barDir val="col"/><ser><idx val="0"/><order val="0"/><tx><v>Old</v></tx><cat><strLit><ptCount val="1"/><pt idx="0"><v>Old</v></pt></strLit></cat><val><numLit><formatCode>0.00</formatCode><ptCount val="1"/><pt idx="0"><v>0</v></pt></numLit></val></ser></barChart></plotArea></chart></chartSpace>"#.as_bytes().to_vec()));
    let template = WorkbookTemplate::parse(parts, &FillOptions::default()).unwrap();
    let filled = template
        .render(
            &json!({"sales":{"categories":["A","B"],"series":[{"title":"Sales","data":[2,4]}]}}),
        )
        .unwrap();
    let chart = xml(&filled, "xl/charts/chart1.xml");
    assert!(chart.contains("strRef"));
    assert!(chart.contains("numCache"));
    assert!(chart.contains("DocxFillChart"));
    let workbook = xml(&filled, "xl/workbook.xml");
    assert!(workbook.contains("state=\"hidden\""));
    let data = filled
        .iter()
        .find(|(p, _)| p.contains("/docxFillChart") && p.ends_with(".xml"))
        .unwrap();
    let data = std::str::from_utf8(&data.1).unwrap();
    assert!(data.contains("<v>4</v>"));
}

#[test]
fn svg_images_keep_their_png_fallback_and_source() {
    struct Svg;
    impl FillHooks for Svg {
        fn image(&mut self, _: &str, _: &Value) -> Option<Image> {
            Some(Image{key:"vector".into(),bytes:Arc::from(br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10"/></svg>"#.as_slice()),extension:"svg".into(),width:10.0,height:10.0})
        }
        fn svg_fallback(&mut self, _: &str, _: &Value, _: &Image) -> Option<Arc<[u8]>> {
            Some(Arc::from(
                include_bytes!("fixtures/spreadsheet/pixel.png").as_slice(),
            ))
        }
    }
    let p = package(&format!(
        r#"<row r="1">{}</row>"#,
        inline("A1", "{%vector}")
    ));
    let t = WorkbookTemplate::parse(p, &FillOptions::default()).unwrap();
    let out = t.render_with(&json!({"vector":true}), &mut Svg).unwrap();
    assert!(out.iter().any(|(p, _)| p.ends_with(".svg")));
    assert!(out.iter().any(|(p, _)| p.ends_with(".png")));
    let drawing = out
        .iter()
        .find(|(p, _)| p.starts_with("xl/drawings/") && p.ends_with(".xml"))
        .unwrap();
    assert!(
        std::str::from_utf8(&drawing.1)
            .unwrap()
            .contains("asvg:svgBlip")
    );
}

fn chart_package_for_copy_test() -> Vec<(String, Vec<u8>)> {
    let mut parts = package(&format!(r#"<row r="1">{}</row>"#, inline("A1", "{$sales}")));
    let sheet=String::from_utf8(parts[4].1.clone()).unwrap().replace("</worksheet>",r#"<drawing xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" r:id="draw"/></worksheet>"#);
    replace(&mut parts, "xl/worksheets/sheet1.xml", sheet);
    parts.push(("xl/worksheets/_rels/sheet1.xml.rels".into(),format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="draw" Type="{REL}/drawing" Target="../drawings/chart.xml"/></Relationships>"#).into_bytes()));
    parts.push(("xl/drawings/chart.xml".into(),format!(r#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:r="{REL}"><xdr:oneCellAnchor><xdr:from><xdr:col>0</xdr:col><xdr:row>2</xdr:row></xdr:from><xdr:graphicFrame><c:chart r:id="chart"/></xdr:graphicFrame></xdr:oneCellAnchor></xdr:wsDr>"#).into_bytes()));
    parts.push(("xl/drawings/_rels/chart.xml.rels".into(),format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="chart" Type="{REL}/chart" Target="../charts/chart1.xml"/></Relationships>"#).into_bytes()));
    parts.push(("xl/charts/chart1.xml".into(),r#"<chartSpace xmlns="http://schemas.openxmlformats.org/drawingml/2006/chart"><chart><plotArea><barChart><barDir val="col"/><ser><idx val="0"/><order val="0"/><tx><v>Old</v></tx><cat><strLit><ptCount val="1"/><pt idx="0"><v>Old</v></pt></strLit></cat><val><numLit><formatCode>0.00</formatCode><ptCount val="1"/><pt idx="0"><v>0</v></pt></numLit></val></ser></barChart></plotArea></chart></chartSpace>"#.as_bytes().to_vec()));

    parts
}

#[test]
fn repeated_chart_sheets_have_independent_chart_data() {
    let mut t =
        WorkbookTemplate::parse(chart_package_for_copy_test(), &FillOptions::default()).unwrap();
    t.repeat_sheet(
        "Data",
        vec![
            docx_fill::SheetInstance {
                name: "Second".into(),
                data: json!({"sales":{"categories":["B"],"series":[{"data":[20]}]}}),
            },
            docx_fill::SheetInstance {
                name: "Third".into(),
                data: json!({"sales":{"categories":["C"],"series":[{"data":[30]}]}}),
            },
        ],
    )
    .unwrap();
    let out = t
        .render(&json!({"sales":{"categories":["A"],"series":[{"data":[10]}]}}))
        .unwrap();
    let charts: Vec<_> = out
        .iter()
        .filter(|(p, _)| p.starts_with("xl/charts/") && p.ends_with(".xml"))
        .map(|(_, b)| std::str::from_utf8(b).unwrap())
        .collect();
    assert_eq!(charts.len(), 3);
    for n in [10, 20, 30] {
        assert!(
            charts
                .iter()
                .any(|xml| xml.contains(&format!("<c:v>{n}</c:v>"))),
            "Missing {n} in independent charts"
        );
    }
}
