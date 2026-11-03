//! cargo run --example fill_xlsx -- template.xlsx data.json output.xlsx

use std::error::Error;
use std::io::{Cursor, Read, Write};
use std::process::ExitCode;

use docx_fill::{FillOptions, WorkbookTemplate};
use serde_json::Value;
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

type Outcome<T> = Result<T, Box<dyn Error>>;
const MAX_COMPRESSED: usize = 64 * 1024 * 1024;
const MAX_EXPANDED: u64 = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;

/// Read a bounded ZIP, fill its package parts, and return a new XLSX file.
pub fn fill_xlsx(bytes: &[u8], data: &Value) -> Outcome<Vec<u8>> {
    if bytes.len() > MAX_COMPRESSED { return Err("Input XLSX exceeds 64 MiB".into()); }
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    if archive.len() > MAX_ENTRIES || archive.has_duplicate_names() { return Err("Too many or duplicate ZIP entries".into()); }
    let mut parts = Vec::new();
    let mut total = 0u64;
    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        if entry.is_dir() { continue; }
        total = total.checked_add(entry.size()).ok_or("ZIP size overflow")?;
        if total > MAX_EXPANDED { return Err("Expanded XLSX exceeds 256 MiB".into()); }
        let name = entry.name()?.into_owned();
        let declared = entry.size();
        let mut content = Vec::new();
        entry.take(declared + 1).read_to_end(&mut content)?;
        if content.len() as u64 != declared { return Err("ZIP entry size differs from its declaration".into()); }
        parts.push((name, content));
    }
    let template = WorkbookTemplate::parse(parts, &FillOptions::default())?;
    let mut output = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, content) in template.render(data)? {
        output.start_file(name, options)?;
        output.write_all(&content)?;
    }
    let bytes = output.finish()?.into_inner();
    if bytes.len() > MAX_COMPRESSED { return Err("Output XLSX exceeds 64 MiB".into()); }
    Ok(bytes)
}

fn run(input: &str, data: &str, output: &str) -> Outcome<()> {
    // Check metadata before allocating the input buffer.
    if std::fs::metadata(input)?.len() > MAX_COMPRESSED as u64 { return Err("Input XLSX exceeds 64 MiB".into()); }
    let bytes = std::fs::read(input)?;
    let data: Value = serde_json::from_slice(&std::fs::read(data)?)?;
    let filled = fill_xlsx(&bytes, &data)?;
    std::fs::write(output, filled)?;
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let [input, data, output] = args.as_slice() else {
        eprintln!("Usage: fill_xlsx <template.xlsx> <data.json> <output.xlsx>");
        return ExitCode::from(2);
    };
    match run(input, data, output) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => { eprintln!("{error}"); ExitCode::FAILURE }
    }
}
