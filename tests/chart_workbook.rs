use docx_fill::{ChartDataMode, FillOptions};
#[cfg(not(feature = "chart-workbook"))]
use docx_fill::{FillErrorId, PackageTemplate};

#[test]
fn parses_workbook_option() {
    let options = FillOptions::default().read_json(&serde_json::json!({"chartData":"workbook"})).unwrap();
    assert_eq!(options.chart_data, ChartDataMode::Workbook);
    assert_eq!(FillOptions::default().chart_data, ChartDataMode::Literal);
    assert!(FillOptions::default().read_json(&serde_json::json!({"chartData":"other"})).is_err());
}

#[cfg(not(feature = "chart-workbook"))]
#[test]
fn requires_workbook_feature() {
    let options = FillOptions { chart_data: ChartDataMode::Workbook, ..FillOptions::default() };
    let error = PackageTemplate::parse(Vec::new(), &options).unwrap_err();
    assert_eq!(error.id, FillErrorId::ChartWorkbook);
}

#[cfg(feature = "chart-workbook")]
mod enabled {
    use std::collections::BTreeMap;
    use std::io::{Cursor, Read, Write};
    use docx_fill::{ChartDataMode, FillErrorId, FillOptions, PackageTemplate};
    use serde_json::{Value, json};
    use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

    const SHEET_NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
    const REL_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
    const OFFICE_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    const TYPES_NS: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
    const ORIGINAL_CELL: &str = "<c r=\"B2\" s=\"1\"><f>SUM(1,2)</f><v>3</v></c>";
    const STYLES: &str = "<styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><numFmts count=\"1\"><numFmt numFmtId=\"164\" formatCode=\"0.00\"/></numFmts><fonts count=\"1\"><font/></fonts><fills count=\"1\"><fill/></fills><borders count=\"1\"><border/></borders><cellStyleXfs count=\"1\"><xf/></cellStyleXfs><cellXfs count=\"2\"><xf/><xf numFmtId=\"164\" applyNumberFormat=\"1\"/></cellXfs></styleSheet>";
    const NOTES: &str = "<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData><row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>Unrelated notes</t></is></c></row></sheetData></worksheet>";
    const DRAWING: &str = "<w:p><w:r><w:drawing><c:chart xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" r:id=\"chart\"/></w:drawing></w:r></w:p>";

    fn workbook(first_name: &str) -> Vec<u8> {
        let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
        let files = [
            ("_rels/.rels", format!("<Relationships xmlns=\"{REL_NS}\"><Relationship Id=\"r1\" Type=\"{OFFICE_REL}/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>")),
            ("[Content_Types].xml", format!("<Types xmlns=\"{TYPES_NS}\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/><Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/><Override PartName=\"/xl/worksheets/sheet2.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/></Types>")),
            ("xl/workbook.xml", format!("<workbook xmlns=\"{SHEET_NS}\" xmlns:r=\"{OFFICE_REL}\"><bookViews><workbookView activeTab=\"0\"/></bookViews><sheets><sheet name=\"{first_name}\" sheetId=\"1\" r:id=\"s1\"/><sheet name=\"Notes\" sheetId=\"2\" r:id=\"s2\"/></sheets></workbook>")),
            ("xl/_rels/workbook.xml.rels", format!("<Relationships xmlns=\"{REL_NS}\"><Relationship Id=\"s1\" Type=\"{OFFICE_REL}/worksheet\" Target=\"worksheets/sheet1.xml\"/><Relationship Id=\"s2\" Type=\"{OFFICE_REL}/worksheet\" Target=\"worksheets/sheet2.xml\"/></Relationships>")),
            ("xl/worksheets/sheet1.xml", format!("<worksheet xmlns=\"{SHEET_NS}\"><sheetViews><sheetView workbookViewId=\"0\" tabSelected=\"1\"/></sheetViews><sheetData><row r=\"2\">{ORIGINAL_CELL}</row></sheetData></worksheet>")),
            ("xl/worksheets/sheet2.xml", NOTES.into()),
            ("xl/styles.xml", STYLES.into()),
            ("customXml/item1.xml", "<custom>Keep me</custom>".into()),
        ];
        archive.set_comment("source archive").unwrap();
        for (name, bytes) in files {
            archive.start_file(name, SimpleFileOptions::default()).unwrap();
            archive.write_all(bytes.as_bytes()).unwrap();
        }
        archive.finish().unwrap().into_inner()
    }

    fn chart() -> String {
        format!("<c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\" xmlns:r=\"{OFFICE_REL}\"><c:chart><c:plotArea><c:barChart><c:barDir val=\"col\"/><c:ser><c:idx val=\"0\"/><c:order val=\"0\"/><c:tx><c:strRef><c:f>Template!$B$1</c:f><c:strCache><c:ptCount val=\"1\"/><c:pt idx=\"0\"><c:v>Old</c:v></c:pt></c:strCache></c:strRef></c:tx><c:cat><c:strRef><c:f>Template!$A$2:$A$3</c:f></c:strRef></c:cat><c:val><c:numRef><c:f>Template!$B$2:$B$3</c:f><c:numCache><c:formatCode>0.00</c:formatCode></c:numCache></c:numRef></c:val></c:ser></c:barChart></c:plotArea></c:chart><c:externalData r:id=\"workbook\"><c:autoUpdate val=\"1\"/></c:externalData></c:chartSpace>")
    }

    fn parts(body: &str, workbook: Vec<u8>, rel: &str) -> Vec<(String, Vec<u8>)> {
        vec![
            ("word/document.xml".into(), format!("<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>{body}</w:body></w:document>").into_bytes()),
            ("word/charts/chart1.xml".into(), chart().into_bytes()),
            ("word/charts/_rels/chart1.xml.rels".into(), format!("<Relationships xmlns=\"{REL_NS}\">{rel}</Relationships>").into_bytes()),
            ("word/embeddings/data.xlsx".into(), workbook),
            ("word/_rels/document.xml.rels".into(), format!("<Relationships xmlns=\"{REL_NS}\"><Relationship Id=\"chart\" Type=\"{OFFICE_REL}/chart\" Target=\"charts/chart1.xml\"/></Relationships>").into_bytes()),
            ("[Content_Types].xml".into(), format!("<Types xmlns=\"{TYPES_NS}\"><Default Extension=\"xml\" ContentType=\"application/xml\"/></Types>").into_bytes()),
        ]
    }

    fn relation() -> String {
        format!("<Relationship Id=\"workbook\" Type=\"{OFFICE_REL}/package\" Target=\"../embeddings/data.xlsx\"/>")
    }

    fn options() -> FillOptions { FillOptions { chart_data: ChartDataMode::Workbook, ..FillOptions::default() } }

    fn data() -> Value { json!({"sales":{"categories":["A & B","C"],"series":[{"title":"Revenue","data":[1.5,null]},{"title":"Costs","data":[2,3]}]}}) }

    fn unzip(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
        let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
        assert_eq!(archive.comment(), b"source archive");
        let mut parts = BTreeMap::new();
        for index in 0..archive.len() {
            let mut file = archive.by_index(index).unwrap();
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).unwrap();
            parts.insert(file.name().unwrap().into_owned(), bytes);
        }
        parts
    }

    fn text(parts: &BTreeMap<String, Vec<u8>>, path: &str) -> String { String::from_utf8(parts[path].clone()).unwrap() }

    #[test]
    fn links_cloned_workbook_data() {
        let original = workbook("Template");
        let body = format!("{DRAWING}<w:p><w:r><w:t>{{$sales}}</w:t></w:r></w:p>");
        let template = PackageTemplate::parse(parts(&body, original.clone(), &relation()), &options()).unwrap();
        let rendered: BTreeMap<_, _> = template.try_render(&data()).unwrap().into_iter().map(|(name, bytes)| (name, bytes.into_owned())).collect();
        assert_eq!(rendered["word/embeddings/data.xlsx"], original);
        let chart = text(&rendered, "word/charts/docxFillChart1.xml");
        assert!(chart.contains("<c:f>'DocxFillData'!$A$2:$A$3</c:f>"));
        assert!(chart.contains("<c:f>'DocxFillData'!$B$1</c:f>"));
        assert!(chart.contains("<c:numCache><c:formatCode>0.00</c:formatCode>"));
        assert!(chart.contains("<c:externalData r:id=\"workbook\">"));
        assert!(!chart.contains("<c:numLit>") && !chart.contains("<c:strLit>"));
        assert!(text(&rendered, "word/charts/_rels/docxFillChart1.xml.rels").contains("../embeddings/docxFillWorkbook1.xlsx"));
        assert!(text(&rendered, "[Content_Types].xml").contains("/word/embeddings/docxFillWorkbook1.xlsx"));
        let workbook = unzip(&rendered["word/embeddings/docxFillWorkbook1.xlsx"]);
        let worksheet = text(&workbook, "xl/worksheets/docxFillData1.xml");
        assert!(worksheet.contains("<c r=\"A2\" t=\"inlineStr\"><is><t xml:space=\"preserve\">A &amp; B</t>"));
        assert!(worksheet.contains("<c r=\"B2\"><v>1.5</v></c>") && !worksheet.contains("r=\"B3\""));
        assert!(worksheet.contains("<c r=\"C3\"><v>3</v></c>"));
        assert!(text(&workbook, "xl/workbook.xml").contains("name=\"DocxFillData\" sheetId=\"3\""));
        assert!(text(&workbook, "xl/workbook.xml").contains("activeTab=\"2\""));
        assert!(text(&workbook, "xl/_rels/workbook.xml.rels").contains("worksheets/docxFillData1.xml"));
        assert!(text(&workbook, "[Content_Types].xml").contains("/xl/worksheets/docxFillData1.xml"));
        assert!(text(&workbook, "xl/worksheets/sheet1.xml").contains(ORIGINAL_CELL));
        assert!(text(&workbook, "xl/worksheets/sheet1.xml").contains("tabSelected=\"0\""));
        assert_eq!(text(&workbook, "xl/worksheets/sheet2.xml"), NOTES);
        assert_eq!(text(&workbook, "xl/styles.xml"), STYLES);
        assert_eq!(text(&workbook, "customXml/item1.xml"), "<custom>Keep me</custom>");
    }

    #[test]
    fn gives_loop_copies_own_data() {
        let body = format!("<w:p><w:r><w:t>{{#rows}}</w:t></w:r></w:p>{DRAWING}<w:p><w:r><w:t>{{$sales}}</w:t></w:r></w:p><w:p><w:r><w:t>{{/rows}}</w:t></w:r></w:p>");
        let template = PackageTemplate::parse(parts(&body, workbook("Template"), &relation()), &options()).unwrap();
        let data = json!({"rows":[{"sales":{"categories":["A"],"series":[{"data":[10]}]}},{"sales":{"categories":["B"],"series":[{"data":[20]}]}}]});
        let rendered: BTreeMap<_, _> = template.try_render(&data).unwrap().into_iter().map(|(name, bytes)| (name, bytes.into_owned())).collect();
        for (index, value) in [(1, 10), (2, 20)] {
            let workbook = unzip(&rendered[&format!("word/embeddings/docxFillWorkbook{index}.xlsx")]);
            assert!(text(&workbook, "xl/worksheets/docxFillData1.xml").contains(&format!("<v>{value}</v>")));
            assert!(text(&rendered, &format!("word/charts/_rels/docxFillChart{index}.xml.rels")).contains(&format!("docxFillWorkbook{index}.xlsx")));
        }
    }

    #[test]
    fn avoids_existing_sheet_names() {
        let body = format!("{DRAWING}<w:p><w:r><w:t>{{$sales}}</w:t></w:r></w:p>");
        let template = PackageTemplate::parse(parts(&body, workbook("docxfilldata"), &relation()), &options()).unwrap();
        let rendered: BTreeMap<_, _> = template.try_render(&data()).unwrap().into_iter().map(|(name, bytes)| (name, bytes.into_owned())).collect();
        let workbook = unzip(&rendered["word/embeddings/docxFillWorkbook1.xlsx"]);
        assert!(text(&workbook, "xl/workbook.xml").contains("name=\"DocxFillData2\""));
        assert!(workbook.contains_key("xl/worksheets/docxFillData2.xml"));
        assert!(text(&rendered, "word/charts/docxFillChart1.xml").contains("'DocxFillData2'!"));
    }

    #[test]
    fn rejects_bad_workbooks() {
        let body = format!("{DRAWING}<w:p><w:r><w:t>{{$sales}}</w:t></w:r></w:p>");
        let external = relation().replace("Target=", "TargetMode=\"External\" Target=");
        for (workbook, relation) in [(b"invalid zip".to_vec(), relation()), (workbook("Template"), external)] {
            let original = parts(&body, workbook, &relation);
            let template = PackageTemplate::parse(original.clone(), &options()).unwrap();
            let error = template.try_render(&data()).unwrap_err();
            assert_eq!(error.id, FillErrorId::ChartWorkbook);
            assert_eq!(error.part.as_deref(), Some("word/charts/chart1.xml"));
            let rendered: BTreeMap<_, _> = template.render(&data()).into_iter().map(|(name, bytes)| (name, bytes.into_owned())).collect();
            assert!(!rendered.keys().any(|name| name.contains("docxFillChart") || name.contains("docxFillWorkbook")));
            for (name, bytes) in original.iter().filter(|(name, _)| name != "word/document.xml") { assert_eq!(&rendered[name], bytes); }
        }
    }

    #[test]
    fn rejects_empty_workbook_ranges() {
        let body = format!("{DRAWING}<w:p><w:r><w:t>{{$sales}}</w:t></w:r></w:p>");
        let template = PackageTemplate::parse(parts(&body, workbook("Template"), &relation()), &options()).unwrap();
        let error = template.try_render(&json!({"sales":{"categories":[],"series":[{"data":[]}]}})).unwrap_err();
        assert_eq!(error.id, FillErrorId::ChartWorkbook);
    }
    fn change_part(bytes: Vec<u8>, path: &str, change: impl FnOnce(String) -> String) -> Vec<u8> {
        let mut parts = unzip(&bytes);
        let xml = String::from_utf8(parts.remove(path).unwrap()).unwrap();
        parts.insert(path.to_string(), change(xml).into_bytes());
        let mut output = ZipWriter::new(Cursor::new(Vec::new()));
        output.set_comment("source archive").unwrap();
        for (name, bytes) in parts {
            output.start_file(name, SimpleFileOptions::default()).unwrap();
            output.write_all(&bytes).unwrap();
        }
        output.finish().unwrap().into_inner()
    }

    #[test]
    fn links_xy_and_bubble_ranges() {
        let body = format!("{DRAWING}<w:p><w:r><w:t>{{$sales}}</w:t></w:r></w:p>");
        for kind in ["scatterChart", "bubbleChart"] {
            let mut source = parts(&body, workbook("Template"), &relation());
            let chart = chart().replace("barChart", kind).replace("<c:barDir val=\"col\"/>", "").replace("c:cat", "c:xVal").replace("c:val", "c:yVal");
            source.iter_mut().find(|(name, _)| name == "word/charts/chart1.xml").unwrap().1 = chart.into_bytes();
            let template = PackageTemplate::parse(source, &options()).unwrap();
            let mut values = json!({"sales":{"series":[{"title":"XY","data":[{"x":1,"y":2,"size":3,"name":"P1"},{"x":4,"y":5,"size":6,"name":"P2"}]}]}});
            if kind == "scatterChart" {
                for point in values["sales"]["series"][0]["data"].as_array_mut().unwrap() { point.as_object_mut().unwrap().remove("size"); }
            }
            let rendered: BTreeMap<_, _> = template.try_render(&values).unwrap().into_iter().map(|(name, bytes)| (name, bytes.into_owned())).collect();
            let chart = text(&rendered, "word/charts/docxFillChart1.xml");
            assert!(chart.contains("<c:xVal><c:numRef><c:f>'DocxFillData'!$A$2:$A$3</c:f>"));
            assert!(chart.contains("<c:yVal><c:numRef><c:f>'DocxFillData'!$B$2:$B$3</c:f>"));
            assert!(chart.contains("P1") && chart.contains("P2"));
            let workbook = unzip(&rendered["word/embeddings/docxFillWorkbook1.xlsx"]);
            let worksheet = text(&workbook, "xl/worksheets/docxFillData1.xml");
            assert!(worksheet.contains("<c r=\"A2\"><v>1</v></c>") && worksheet.contains("<c r=\"B3\"><v>5</v></c>"));
            if kind == "bubbleChart" {
                assert!(chart.contains("<c:bubbleSize><c:numRef><c:f>'DocxFillData'!$C$2:$C$3</c:f>"));
                assert!(worksheet.contains("<c r=\"C3\"><v>6</v></c>"));
            }
        }
    }

    #[test]
    fn keeps_point_labels_separate() {
        let body = format!("{DRAWING}<w:p><w:r><w:t>{{$sales}}</w:t></w:r></w:p>");
        let mut source = parts(&body, workbook("Template"), &relation());
        let chart = chart().replace("barChart", "scatterChart").replace("<c:barDir val=\"col\"/>", "").replace("c:cat", "c:xVal").replace("c:val", "c:yVal");
        let first = chart.find("<c:tx>").unwrap();
        let end = chart.find("</c:tx>").unwrap() + "</c:tx>".len();
        let chart = format!("{}{}", &chart[..first], &chart[end..]);
        source.iter_mut().find(|(name, _)| name == "word/charts/chart1.xml").unwrap().1 = chart.into_bytes();
        let template = PackageTemplate::parse(source, &options()).unwrap();
        let rendered: BTreeMap<_, _> = template.try_render(&json!({"sales":{"series":[{"data":[{"x":1,"y":2,"name":"Point name"}]}]}})).unwrap().into_iter().map(|(name, bytes)| (name, bytes.into_owned())).collect();
        let chart = text(&rendered, "word/charts/docxFillChart1.xml");
        assert!(chart.contains("Point name"));
        let workbook = unzip(&rendered["word/embeddings/docxFillWorkbook1.xlsx"]);
        assert!(text(&workbook, "xl/worksheets/docxFillData1.xml").contains("Series 1"));
        assert!(!text(&workbook, "xl/worksheets/docxFillData1.xml").contains("Point name"));
    }

    #[test]
    fn accepts_prefixed_workbooks() {
        let workbook = change_part(workbook("Template"), "xl/workbook.xml", |mut xml| {
            for name in ["workbook", "bookViews", "workbookView", "sheets", "sheet"] {
                xml = xml.replace(&format!("<{name}"), &format!("<s:{name}")).replace(&format!("</{name}"), &format!("</s:{name}"));
            }
            xml.replace(&format!("xmlns=\"{SHEET_NS}\""), &format!("xmlns:s=\"{SHEET_NS}\"")).replace('"', "'")
        });
        let body = format!("{DRAWING}<w:p><w:r><w:t>{{$sales}}</w:t></w:r></w:p>");
        let template = PackageTemplate::parse(parts(&body, workbook, &relation().replace('"', "'")), &options()).unwrap();
        let rendered: BTreeMap<_, _> = template.try_render(&data()).unwrap().into_iter().map(|(name, bytes)| (name, bytes.into_owned())).collect();
        let workbook = unzip(&rendered["word/embeddings/docxFillWorkbook1.xlsx"]);
        let xml = text(&workbook, "xl/workbook.xml");
        assert!(xml.contains("<s:sheet name=\"DocxFillData\""));
        assert!(xml.contains("activeTab='2'"));
    }

    #[test]
    fn refuses_protected_structure() {
        let workbook = change_part(workbook("Template"), "xl/workbook.xml", |xml| xml.replace("<bookViews>", "<workbookProtection lockStructure=\"1\"/><bookViews>"));
        let body = format!("{DRAWING}<w:p><w:r><w:t>{{$sales}}</w:t></w:r></w:p>");
        let template = PackageTemplate::parse(parts(&body, workbook, &relation()), &options()).unwrap();
        let error = template.try_render(&data()).unwrap_err();
        assert_eq!(error.id, FillErrorId::ChartWorkbook);
        assert!(error.message.contains("structure-protected"));
    }

    #[test]
    fn accepts_xml_declarations() {
        let mut workbook = workbook("Template");
        for path in ["xl/workbook.xml", "xl/_rels/workbook.xml.rels", "_rels/.rels", "[Content_Types].xml"] {
            workbook = change_part(workbook, path, |xml| format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!-- workbook metadata -->\n{xml}"));
        }
        let body = format!("{DRAWING}<w:p><w:r><w:t>{{$sales}}</w:t></w:r></w:p>");
        let template = PackageTemplate::parse(parts(&body, workbook, &relation()), &options()).unwrap();
        let rendered: BTreeMap<_, _> = template.try_render(&data()).unwrap().into_iter().map(|(name, bytes)| (name, bytes.into_owned())).collect();
        let workbook = unzip(&rendered["word/embeddings/docxFillWorkbook1.xlsx"]);
        assert!(text(&workbook, "xl/workbook.xml").starts_with("<?xml"));
        assert!(text(&workbook, "xl/workbook.xml").contains("<!-- workbook metadata -->"));
        assert!(text(&workbook, "xl/worksheets/docxFillData1.xml").contains("Revenue"));
    }


    #[test]
    fn clears_unfilled_title_tag() {
        let mut source = parts(DRAWING, workbook("Template"), &relation());
        let chart = chart().replace("<c:chart><c:plotArea>", "<c:chart><c:title><c:tx><c:rich><a:bodyPr xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"/><a:p xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><a:r><a:t>Sales {$sales}</a:t></a:r></a:p></c:rich></c:tx></c:title><c:plotArea>");
        source.iter_mut().find(|(name, _)| name == "word/charts/chart1.xml").unwrap().1 = chart.into_bytes();
        let template = PackageTemplate::parse(source, &options()).unwrap();
        let rendered: BTreeMap<_, _> = template.render(&json!({})).into_iter().map(|(name, bytes)| (name, bytes.into_owned())).collect();
        assert!(!text(&rendered, "word/charts/chart1.xml").contains("{$sales}"));
    }

    #[test]
    fn accepts_filtered_series() {
        let body = format!("{DRAWING}<w:p><w:r><w:t>{{$sales}}</w:t></w:r></w:p>");
        let mut source = parts(&body, workbook("Template"), &relation());
        let filtered = "<c:extLst><c:ext uri=\"{02D57815-91ED-43cb-92C2-25804820EDAC}\" xmlns:c15=\"http://schemas.microsoft.com/office/drawing/2012/chart\"><c15:filteredBarSeries><c15:ser><c:idx val=\"5\"/><c:order val=\"5\"/><c:val><c:numRef><c:f>Template!$C$2:$C$3</c:f></c:numRef></c:val></c15:ser></c15:filteredBarSeries></c:ext></c:extLst>";
        let chart = chart().replace("</c:barChart>", &format!("{filtered}</c:barChart>"));
        source.iter_mut().find(|(name, _)| name == "word/charts/chart1.xml").unwrap().1 = chart.into_bytes();
        let template = PackageTemplate::parse(source, &options()).unwrap();
        let result = template.try_render(&data());
        assert!(result.is_ok(), "{:?}", result.err());
    }
}
