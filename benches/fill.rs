//! Fill benchmarks for tracking regressions. See BENCHMARKS.md.
//!
//! ```sh
//! cargo bench --all-features --bench fill                                 # print timings
//! cargo bench --all-features --bench fill -- --save benches/baseline.json # record a baseline
//! cargo bench --all-features --bench fill -- --compare benches/baseline.json
//! ```

use std::collections::BTreeMap;
use std::hint::black_box;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use docx_fill::{FillOptions, PackageTemplate, Template};
use serde_json::{Map, Value, json};

const WARMUP_RUNS: usize = 2;
const MIN_RUNS: usize = 5;
const MAX_RUNS: usize = 50;
const TIME_PER_CASE: Duration = Duration::from_secs(2);
/// A case slower than its baseline median by more than this fails `--compare`.
const DEFAULT_THRESHOLD_PERCENT: f64 = 15.0;

const NAMESPACES: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;

struct Case {
    name: &'static str,
    /// Parses and fills once, returning the output size so the work is not optimised away.
    run: Box<dyn Fn() -> usize>,
}

fn document(body: &str) -> String {
    format!("<w:document {NAMESPACES}><w:body>{body}<w:sectPr><w:pgSz w:w=\"12240\" w:h=\"15840\"/><w:pgMar w:top=\"1440\" w:right=\"1440\" w:bottom=\"1440\" w:left=\"1440\"/></w:sectPr></w:body></w:document>")
}

fn paragraph(text: &str) -> String {
    format!("<w:p><w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r></w:p>")
}

/// A part case: parse `body` as a document and fill it with `data`.
fn part_case(name: &'static str, body: String, data: Value, options: FillOptions) -> Case {
    let xml = document(&body);
    Case {
        name,
        run: Box::new(move || {
            let template = Template::parse(&xml, &options).expect("template parses");
            template.render(&data).len()
        }),
    }
}

/// 1,000 tags, each split across three runs as Word often saves them.
fn split_tags() -> Case {
    let body: String = (0..1000).map(|i| format!("<w:p><w:r><w:t>Value {{</w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>v{i}</w:t></w:r><w:r><w:t>}} end</w:t></w:r></w:p>")).collect();
    let data: Map<String, Value> = (0..1000).map(|i| (format!("v{i}"), json!(format!("text {i}")))).collect();
    part_case("tags/split-1k", body, Value::Object(data), FillOptions::default())
}

/// A table row loop over 20,000 items of five cells.
fn table_rows() -> Case {
    let cell = |text: &str| format!("<w:tc><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:tc>");
    let cells = [cell("{#rows}{a}"), cell("{b}"), cell("{c}"), cell("{d}"), cell("{e}{/rows}")].concat();
    let body = format!("<w:tbl><w:tblGrid>{}</w:tblGrid><w:tr>{cells}</w:tr></w:tbl>", "<w:gridCol w:w=\"1800\"/>".repeat(5));
    let rows: Vec<Value> = (0..20_000).map(|i| json!({ "a": i, "b": "bee", "c": "sea", "d": i * 2, "e": "end" })).collect();
    part_case("loop/table-rows-20k", body, json!({ "rows": rows }), FillOptions::default())
}

/// Paragraph loops nested 200 by 100.
fn nested_loops() -> Case {
    let body = [paragraph("{#outer}"), paragraph("Group {name}"), paragraph("{#inner}"), paragraph("{label}: {value}"), paragraph("{/inner}"), paragraph("{/outer}")].concat();
    let inner: Vec<Value> = (0..100).map(|i| json!({ "label": format!("item {i}"), "value": i })).collect();
    let outer: Vec<Value> = (0..200).map(|i| json!({ "name": format!("group {i}"), "inner": inner })).collect();
    part_case("loop/nested-200x100", body, json!({ "outer": outer }), FillOptions::default())
}

/// 5,000 inline sections, half shown.
fn conditions() -> Case {
    let body: String = (0..5000).map(|i| paragraph(&format!("{{#f{i}}}shown {i}{{/f{i}}}{{^f{i}}}hidden{{/f{i}}}"))).collect();
    let data: Map<String, Value> = (0..5000).map(|i| (format!("f{i}"), json!(i % 2 == 0))).collect();
    part_case("conditions/5k", body, Value::Object(data), FillOptions::default())
}

/// A 5,000 item loop of arithmetic, comparisons, paths and `$index`.
fn expressions() -> Case {
    let body = [paragraph("{#items}"), paragraph("{$index + 1}. {item.name}: {price * qty} {qty > 2 ? \"bulk\" : \"single\"}"), paragraph("{/items}")].concat();
    let items: Vec<Value> = (0..5000).map(|i| json!({ "item": { "name": format!("n{i}") }, "price": 1.5, "qty": i % 5 })).collect();
    // Core tags only: with extensions on, `$` starts a chart tag.
    let options = FillOptions { expressions: true, ..FillOptions::core() };
    part_case("expressions/loop-5k", body, json!({ "items": items }), options)
}

/// About 300 KB of HTML: formatted paragraphs, lists and a 200 row table.
fn html_blocks() -> Case {
    let mut html = String::new();
    for i in 0..1500 {
        html.push_str(&format!("<p>Paragraph {i} with <b>bold</b>, <i>italic</i> and <a href=\"https://example.com/{i}\">a link</a>.</p>"));
    }
    html.push_str("<ul>");
    for i in 0..500 {
        html.push_str(&format!("<li>Item {i}<ol><li>Sub {i}</li></ol></li>"));
    }
    html.push_str("</ul><table>");
    for i in 0..200 {
        html.push_str(&format!("<tr><td>{i}</td><td style=\"background:#eeeeee\">cell</td><td>more</td></tr>"));
    }
    html.push_str("</table>");
    part_case("html/blocks-300k", paragraph("{~~body}"), json!({ "body": html }), FillOptions::default())
}

/// 3,000 elements matched against a 60 rule style sheet.
fn html_style_sheet() -> Case {
    let mut sheet = String::new();
    for i in 0..20 {
        sheet.push_str(&format!(".c{i} {{ color: #00{i:02}00; }} div.c{i} span {{ font-weight: bold; }} #id{i} > p + p {{ margin-left: {i}px; }}\n"));
    }
    let mut html = String::new();
    for i in 0..1000 {
        html.push_str(&format!("<div class=\"c{} wrap\" id=\"id{}\"><p>first <span>{i}</span></p><p class=\"c{}\">second</p></div>", i % 20, i % 20, (i + 1) % 20));
    }
    let options = FillOptions { html_style_sheet: sheet, ..FillOptions::default() };
    part_case("html/style-sheet-3k", paragraph("{~~body}"), json!({ "body": html }), options)
}

/// 2,000 template errors collected in one parse.
fn template_errors() -> Case {
    let xml = document(&(0..2000).map(|i| paragraph(&format!("bad {{v{i}}}}} tag"))).collect::<String>());
    let options = FillOptions::default();
    Case {
        name: "errors/2k",
        run: Box::new(move || Template::parse(&xml, &options).expect_err("template has errors").to_string().len()),
    }
}

/// A package with a 2,000 row body loop, rich text, a header, a footer and document properties.
fn package() -> Case {
    let content_types = r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/header1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml"/><Override PartName="/word/footer1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/></Types>"#;
    let body = [paragraph("{title}"), paragraph("{~~summary}"), paragraph("{#rows}"), paragraph("{name}: {status}"), paragraph("{/rows}")].concat();
    let parts: Vec<(String, Vec<u8>)> = vec![
        ("[Content_Types].xml".into(), content_types.into()),
        ("word/document.xml".into(), document(&body).into_bytes()),
        ("word/header1.xml".into(), format!("<w:hdr {NAMESPACES}>{}</w:hdr>", paragraph("{title} header")).into_bytes()),
        ("word/footer1.xml".into(), format!("<w:ftr {NAMESPACES}>{}</w:ftr>", paragraph("{title} footer")).into_bytes()),
        ("docProps/core.xml".into(), br#"<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>{title}</dc:title></cp:coreProperties>"#.to_vec()),
    ];
    let rows: Vec<Value> = (0..2000).map(|i| json!({ "name": format!("row {i}"), "status": if i % 3 == 0 { "open" } else { "closed" } })).collect();
    let summary = "<p>Summary with <b>bold</b> text.</p>".repeat(100);
    let data = json!({ "title": "Report", "summary": summary, "rows": rows });
    let options = FillOptions::default();
    Case {
        name: "package/report-2k",
        run: Box::new(move || {
            let template = PackageTemplate::parse(parts.clone(), &options).expect("package parses");
            template.render(&data).iter().map(|(_, bytes)| bytes.len()).sum()
        }),
    }
}

fn cases() -> Vec<Case> {
    vec![split_tags(), table_rows(), nested_loops(), conditions(), expressions(), html_blocks(), html_style_sheet(), template_errors(), package()]
}

/// Median and fastest run of one case, in milliseconds.
fn measure(case: &Case) -> (f64, f64) {
    for _ in 0..WARMUP_RUNS {
        black_box((case.run)());
    }
    let started = Instant::now();
    let mut times = Vec::new();
    while times.len() < MIN_RUNS || (times.len() < MAX_RUNS && started.elapsed() < TIME_PER_CASE) {
        let run = Instant::now();
        black_box((case.run)());
        times.push(run.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    (times[times.len() / 2], times[0])
}

fn read_baseline(path: &str) -> BTreeMap<String, f64> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{path}: {error}"));
    let value: Value = serde_json::from_str(&text).unwrap_or_else(|error| panic!("{path}: {error}"));
    value["median_ms"].as_object().into_iter().flatten().filter_map(|(name, ms)| Some((name.clone(), ms.as_f64()?))).collect()
}

fn main() -> ExitCode {
    // `cargo bench` passes `--bench`; a bare word filters cases by name.
    let mut save = None;
    let mut baseline = None;
    let mut threshold = DEFAULT_THRESHOLD_PERCENT;
    let mut filter = None;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--bench" => {}
            "--save" => save = arguments.next(),
            "--compare" => baseline = arguments.next().map(|path| read_baseline(&path)),
            "--threshold" => threshold = arguments.next().and_then(|value| value.parse().ok()).expect("--threshold takes a percentage"),
            _ => filter = Some(argument),
        }
    }

    let mut medians = BTreeMap::new();
    let mut regressions = Vec::new();
    println!("{:<24} {:>11} {:>11} {:>9}", "case", "median ms", "fastest ms", "change");
    for case in cases() {
        if filter.as_deref().is_some_and(|filter| !case.name.contains(filter)) {
            continue;
        }
        let (median, fastest) = measure(&case);
        let change = baseline.as_ref().and_then(|baseline| baseline.get(case.name)).map(|before| (median / before - 1.0) * 100.0);
        let shown = change.map_or(String::new(), |change| format!("{change:+.1}%"));
        println!("{:<24} {median:>11.2} {fastest:>11.2} {shown:>9}", case.name);
        if change.is_some_and(|change| change > threshold) {
            regressions.push(case.name);
        }
        medians.insert(case.name.to_string(), (median * 100.0).round() / 100.0);
    }

    if let Some(path) = save {
        let text = serde_json::to_string_pretty(&json!({ "median_ms": medians })).unwrap();
        std::fs::write(&path, text + "\n").unwrap_or_else(|error| panic!("{path}: {error}"));
        println!("Saved {path}");
    }
    if regressions.is_empty() {
        return ExitCode::SUCCESS;
    }
    eprintln!("Slower than the baseline by more than {threshold}%: {}", regressions.join(", "));
    ExitCode::FAILURE
}
