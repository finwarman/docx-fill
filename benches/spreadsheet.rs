//! cargo bench --bench spreadsheet
use docx_fill::{FillOptions, WorkbookTemplate};
use serde_json::json;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
struct Measured;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Measured {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            let n = LIVE.fetch_add(l.size(), Ordering::Relaxed) + l.size();
            PEAK.fetch_max(n, Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size(), Ordering::Relaxed);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, n) };
        if !q.is_null() {
            if n >= l.size() {
                let v = LIVE.fetch_add(n - l.size(), Ordering::Relaxed) + n - l.size();
                PEAK.fetch_max(v, Ordering::Relaxed);
            } else {
                LIVE.fetch_sub(l.size() - n, Ordering::Relaxed);
            }
        }
        q
    }
}
#[global_allocator]
static ALLOCATOR: Measured = Measured;
#[path = "../tests/common/spreadsheet.rs"]
mod common;
fn main() {
    for count in [10_000, 100_000] {
        let parts = common::package(&format!(
            r#"<row r="1">{}{}{}</row>"#,
            common::inline("A1", "{#items}{name}"),
            common::inline("B1", "{value}"),
            common::inline("C1", "{/items}")
        ));
        let data = json!({"items":(0..count).map(|i|json!({"name":format!("Item {i}"),"value":i})).collect::<Vec<_>>()});
        let start = Instant::now();
        let template = WorkbookTemplate::parse(parts, &FillOptions::default()).unwrap();
        let parse = start.elapsed();
        PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
        let start = Instant::now();
        let output = template.render(&data).unwrap();
        let render = start.elapsed();
        let bytes: usize = output.iter().map(|(_, v)| v.len()).sum();
        println!(
            "scenario=row-expansion rows={count} parse={parse:?} render={render:?} output={bytes} peak_live_bytes={}",
            PEAK.load(Ordering::Relaxed)
        );
    }

    let shared_count = 1_024;
    let shared_cells = (0..shared_count)
        .map(|index| {
            format!(
                r#"<c r="{}" t="s"><v>{index}</v></c>"#,
                cell_ref(index + 1, 1)
            )
        })
        .collect::<String>();
    let shared_strings = (0..shared_count)
        .map(|index| format!("<si><t>Entry {index}: {{value}}</t></si>"))
        .collect::<String>();
    let mut parts = common::package(&format!(r#"<row r="1">{shared_cells}</row>"#));
    common::replace(
        &mut parts,
        "xl/worksheets/sheet1.xml",
        format!(
            r#"<worksheet xmlns="{}"><sheetData><row r="1">{shared_cells}</row></sheetData></worksheet>"#,
            common::NS
        ),
    );
    parts.push((
        "xl/sharedStrings.xml".into(),
        format!(
            r#"<sst xmlns="{}" count="{shared_count}" uniqueCount="{shared_count}">{shared_strings}</sst>"#,
            common::NS
        )
        .into_bytes(),
    ));
    let rels = String::from_utf8(
        parts
            .iter()
            .find(|(path, _)| path == "xl/_rels/workbook.xml.rels")
            .unwrap()
            .1
            .clone(),
    )
    .unwrap()
    .replace(
        "</Relationships>",
        &format!(
            r#"<Relationship Id="strings" Type="{}/sharedStrings" Target="sharedStrings.xml"/></Relationships>"#,
            common::REL
        ),
    );
    common::replace(&mut parts, "xl/_rels/workbook.xml.rels", rels);
    let types = String::from_utf8(parts[0].1.clone()).unwrap().replace(
        "</Types>",
        r#"<Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/></Types>"#,
    );
    common::replace(&mut parts, "[Content_Types].xml", types);
    benchmark("shared-strings", parts, json!({"value":"filled"}), shared_count);

    let wide_count = 128;
    let wide_cells = (0..wide_count)
        .map(|index| common::inline(&cell_ref(index + 1, 1), &format!("{{v{index}}}")))
        .collect::<String>();
    let wide_data = (0..wide_count)
        .map(|index| (format!("v{index}"), json!(format!("Value {index}"))))
        .collect::<serde_json::Map<_, _>>();
    let parts = common::package(&format!(r#"<row r="1">{wide_cells}</row>"#));
    benchmark("wide-sheet", parts, serde_json::Value::Object(wide_data), wide_count);
}

fn benchmark(scenario: &str, parts: Vec<(String, Vec<u8>)>, data: serde_json::Value, cells: usize) {
    let start = Instant::now();
    let template = WorkbookTemplate::parse(parts, &FillOptions::default()).unwrap();
    let parse = start.elapsed();
    PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
    let start = Instant::now();
    let output = template.render(&data).unwrap();
    let render = start.elapsed();
    let bytes: usize = output.iter().map(|(_, value)| value.len()).sum();
    println!(
        "scenario={scenario} cells={cells} parse={parse:?} render={render:?} output={bytes} peak_live_bytes={}",
        PEAK.load(Ordering::Relaxed)
    );
}

fn cell_ref(column: usize, row: usize) -> String {
    let mut column = column;
    let mut letters = Vec::new();
    while column > 0 {
        column -= 1;
        letters.push((b'A' + (column % 26) as u8) as char);
        column /= 26;
    }
    letters.reverse();
    format!("{}{row}", letters.iter().collect::<String>())
}
