mod common;

use std::borrow::Cow;

use common::{document, elements, fill, paragraph_texts, read_rows};
use docx_fill::{Extensions, FillOptions, PackageTemplate, fill_part};
use serde_json::json;

const TAG: &str = r#"<w:p><w:r><w:t>{~~html}</w:t></w:r></w:p>"#;

fn html(value: &str) -> String {
    fill(TAG, json!({ "html": value }))
}

fn run_with<'a>(body: &'a str, text: &str) -> &'a str {
    elements(body, "w:r").into_iter().find(|run| run.contains(&format!(">{text}</w:t>"))).unwrap_or_else(|| panic!("no run holds {text}"))
}

#[test]
fn marks_strike_sup_sub_mark_and_code() {
    let body = html("<p><s>a</s><del>b</del><sup>c</sup><sub>d</sub><mark>e</mark><code>f</code><ins>g</ins></p>");

    assert!(run_with(&body, "a").contains("<w:strike/>"));
    assert!(run_with(&body, "b").contains("<w:strike/>"));
    assert!(run_with(&body, "c").contains(r#"<w:vertAlign w:val="superscript"/>"#));
    assert!(run_with(&body, "d").contains(r#"<w:vertAlign w:val="subscript"/>"#));
    assert!(run_with(&body, "e").contains(r#"<w:highlight w:val="yellow"/>"#));
    assert!(run_with(&body, "f").contains(r#"<w:rFonts w:ascii="Courier New" w:hAnsi="Courier New" w:cs="Courier New"/>"#));
    assert!(run_with(&body, "f").contains(r#"<w:highlight w:val="lightGray"/>"#));
    assert!(run_with(&body, "g").contains(r#"<w:u w:val="single"/>"#));
}

#[test]
fn reads_inline_styles_in_schema_order() {
    let body = html(r#"<p><span style="color: #f00; font-size: 16px; font-family: 'Times New Roman', serif; font-weight: 700; font-style: italic; text-decoration: underline line-through; background-color: rgb(0, 128, 255)">x</span></p>"#);

    assert!(run_with(&body, "x").contains(
        r#"<w:rPr><w:rFonts w:ascii="Times New Roman" w:hAnsi="Times New Roman" w:cs="Times New Roman"/><w:b/><w:bCs/><w:i/><w:iCs/><w:strike/><w:color w:val="FF0000"/><w:sz w:val="24"/><w:szCs w:val="24"/><w:u w:val="single"/><w:shd w:val="clear" w:color="auto" w:fill="0080FF"/></w:rPr>"#
    ));
}

#[test]
fn lets_an_inner_style_undo_an_outer_one() {
    let body = html(r#"<p><b>bold <span style="font-weight: normal">plain</span> again</b></p>"#);

    assert!(run_with(&body, "bold ").contains("<w:b/>"));
    assert!(run_with(&body, "plain").contains(r#"<w:b w:val="0"/>"#));
    assert!(run_with(&body, " again").contains("<w:b/>"));
}

#[test]
fn aligns_paragraphs_and_centres_center() {
    let body = html(r#"<p style="text-align: justify">j</p><center>c</center><div style="text-align:right"><p>r</p></div>"#);

    let paragraphs = elements(&body, "w:p");
    assert!(paragraphs[0].contains(r#"<w:jc w:val="both"/>"#));
    assert!(paragraphs[1].contains(r#"<w:jc w:val="center"/>"#));
    assert!(paragraphs[2].contains(r#"<w:jc w:val="right"/>"#));
}

#[test]
fn indents_quotes_and_draws_a_rule() {
    let body = html("<blockquote><p>q</p></blockquote><hr><p>after</p>");

    let paragraphs = elements(&body, "w:p");
    assert!(paragraphs[0].contains(r#"<w:ind w:left="720"/>"#));
    assert!(paragraphs[1].contains(r#"<w:pBdr><w:bottom w:val="single" w:sz="6" w:space="1" w:color="auto"/></w:pBdr>"#));
    assert_eq!(paragraph_texts(&body), ["q", "", "after"]);
}

#[test]
fn keeps_preformatted_lines_in_a_fixed_font() {
    let body = html("<pre>a  b\nc</pre>");

    assert!(body.contains(r#"<w:t xml:space="preserve">a  b</w:t>"#));
    assert!(body.contains("<w:br/>"));
    assert!(run_with(&body, "c").contains("Courier New"));
}

#[test]
fn numbers_lists_from_start_and_value_in_their_style() {
    let body = html(r#"<ol start="3"><li>c</li><li value="10">j</li><li>k</li></ol><ol type="A"><li>a</li></ol><ul style="list-style-type: lower-roman"><li>i</li></ul>"#);

    assert_eq!(paragraph_texts(&body), ["3.c", "10.j", "11.k", "A.a", "i.i"]);
}

#[test]
fn links_to_addresses_by_relationship_and_to_bookmarks_by_anchor() {
    let body = html(r##"<p>See <a href="https://example.com/?a=1&amp;b=2">site</a> or <a href="#notes">notes</a>.</p>"##);

    assert!(body.contains(r#"<w:hyperlink r:id="rIdDocxFillLink1" w:history="1"><w:r><w:rPr><w:color w:val="0563C1"/><w:u w:val="single"/></w:rPr><w:t xml:space="preserve">site</w:t></w:r></w:hyperlink>"#));
    assert!(body.contains(r#"<w:hyperlink w:anchor="notes" w:history="1">"#));
}

#[test]
fn adds_a_relationship_per_address_to_the_parts_rels() {
    let parts = vec![("word/document.xml".to_string(), document(TAG).into_bytes())];
    let template = PackageTemplate::parse(parts, &FillOptions::default()).unwrap();
    let filled = template.render(&json!({ "html": r#"<a href="https://a.test">1</a><a href="https://b.test">2</a><a href="https://a.test">3</a>"# }));

    let rels = filled.iter().find(|(name, _)| name == "word/_rels/document.xml.rels").map(|(_, bytes): &(String, Cow<[u8]>)| String::from_utf8_lossy(bytes).into_owned()).expect("rels added");
    assert!(rels.contains(r#"<Relationship Id="rIdDocxFillLink1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://a.test" TargetMode="External"/>"#));
    assert!(rels.contains(r#"Id="rIdDocxFillLink2""#));
    assert!(!rels.contains("rIdDocxFillLink3"));
}

#[test]
fn builds_a_bordered_table_with_bold_header_cells_and_spans() {
    let body = html(r#"<p>before</p><table><thead><tr><th>H1</th><th>H2</th></tr></thead><tbody><tr><td colspan="2">wide</td></tr><tr><td><p>a</p><p>b</p></td></tr></tbody></table>"#);

    assert!(body.contains(r#"<w:tblW w:w="5000" w:type="pct"/><w:tblBorders>"#));
    assert_eq!(read_rows(&body), vec![vec!["H1", "H2"], vec!["wide"], vec!["ab", ""]]);
    assert!(run_with(&body, "H1").contains("<w:b/>"));
    assert!(body.contains(r#"<w:gridSpan w:val="2"/>"#));
    assert!(body.ends_with("</w:tbl><w:p></w:p>"));
}

#[test]
fn flattens_a_table_beside_other_text() {
    let body = fill(r#"<w:p><w:r><w:t>T: {~html}</w:t></w:r></w:p>"#, json!({ "html": "<table><tr><td>a</td><td>b</td></tr><tr><td>c</td></tr></table>" }));

    assert_eq!(paragraph_texts(&body), ["T: abc"]);
    assert!(body.contains("<w:tab/>") && body.contains("<w:br/>"));
}

#[test]
fn prints_checkboxes_and_drops_comments() {
    let body = html(r#"<p><input type="checkbox" checked> done <input type="checkbox"> todo<!-- hidden --></p>"#);

    assert_eq!(paragraph_texts(&body), ["\u{2612} done \u{2610} todo"]);
}

#[test]
fn reads_only_the_core_html_without_extras() {
    let options = FillOptions { extensions: Extensions { html_extras: false, ..Extensions::ALL }, ..FillOptions::default() };
    let xml = fill_part(&document(TAG), &json!({ "html": "<p><s>x</s></p>" }), &options).unwrap();

    assert!(!xml.contains("strike"));
    assert!(xml.contains(">x</w:t>"));
}

#[test]
fn keeps_the_text_of_cells_outside_a_table() {
    for value in ["<p><td>x</td></p>", "<td>x</td>", "<tr><td>x</td></tr>", "<ul><td>x</td></ul>", "<ul><li><td>x</td></li></ul>", "<p>a</p><th>x</th>"] {
        let texts = paragraph_texts(&html(value));
        let expected = if value.contains("<li>") { "\u{2022}x" } else { "x" };
        assert_eq!(texts.last().map(String::as_str), Some(expected), "{value}");
    }
}

#[test]
fn fills_any_html_without_panicking() {
    const PIECES: [&str; 24] = [
        "<table>", "</table>", "<tr>", "</tr>", "<td>", "</td>", "<th colspan=\"3\">", "</th>", "<p>", "</p>", "<ul>", "</ul>", "<ol>", "</ol>", "<li>", "</li>",
        "<b>", "</b>", "<pre>", "</pre>", "<br>", "x", "<style>", "&amp;",
    ];
    // xorshift, seeded so a failure repeats.
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    for _ in 0..3000 {
        let mut value = String::new();
        for _ in 0..12 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            value.push_str(PIECES[(state % PIECES.len() as u64) as usize]);
        }
        let filled = fill_part(&document(TAG), &json!({ "html": value }), &FillOptions::default());
        assert!(filled.is_ok(), "{value}");
    }
}

#[test]
fn prints_nothing_inside_style_script_and_template() {
    let body = html("<style>p{color:red}</style><p>x</p><SCRIPT>run()</SCRIPT><template><p>t</p></template><p>y</p>");
    assert_eq!(paragraph_texts(&body), ["x", "y"]);
}

#[test]
fn separates_adjacent_tables() {
    let body = html("<table><tr><td>a</td></tr></table><table><tr><td>b</td></tr></table>");
    assert_eq!(elements(&body, "w:tbl").len(), 2);
    assert!(!body.contains("</w:tbl><w:tbl>"));
}

#[test]
fn separates_adjacent_tables_in_a_cell() {
    let body = html("<table><tr><td><table><tr><td>a</td></tr></table><table><tr><td>b</td></tr></table></td></tr></table>");
    assert!(!body.contains("</w:tbl><w:tbl>"));
}

#[test]
fn limits_a_cell_span_to_words_column_count() {
    let body = html(r#"<table><td colspan="999999">x</td></table>"#);
    assert!(body.contains(r#"<w:gridSpan w:val="63"/>"#));
    assert!(body.len() < 10_000, "{} bytes", body.len());
}
