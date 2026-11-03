#![allow(dead_code)]
pub const NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
pub const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
pub fn inline(reference: &str, text: &str) -> String {
    format!(r#"<c r="{reference}" t="inlineStr"><is><t>{text}</t></is></c>"#)
}
pub fn package(rows: &str) -> Vec<(String, Vec<u8>)> {
    vec![
        ("[Content_Types].xml".into(), br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#.to_vec()),
        ("_rels/.rels".into(), format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="office" Type="{REL}/officeDocument" Target="xl/workbook.xml"/></Relationships>"#).into_bytes()),
        ("xl/workbook.xml".into(), format!(r#"<workbook xmlns="{NS}" xmlns:r="{REL}"><sheets><sheet name="Data" sheetId="1" r:id="s1"/></sheets></workbook>"#).into_bytes()),
        ("xl/_rels/workbook.xml.rels".into(), format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="s1" Type="{REL}/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#).into_bytes()),
        ("xl/worksheets/sheet1.xml".into(), format!(r#"<worksheet xmlns="{NS}"><sheetData>{rows}</sheetData></worksheet>"#).into_bytes()),
        ("xl/media/unchanged.bin".into(), vec![0, 255, 42, 1]),
    ]
}
pub fn xml<'a>(parts: &'a [(String, std::borrow::Cow<'_, [u8]>)], name: &str) -> &'a str {
    std::str::from_utf8(&parts.iter().find(|(path, _)| path == name).unwrap().1).unwrap()
}
pub fn replace(parts: &mut [(String, Vec<u8>)], name: &str, xml: String) {
    parts.iter_mut().find(|(path, _)| path == name).unwrap().1 = xml.into_bytes();
}
