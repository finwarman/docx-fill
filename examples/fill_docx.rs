//! Fills a `.docx` file with values from a JSON file:
//!
//! ```sh
//! cargo run --example fill_docx -- in.docx data.json out.docx
//! ```
//!
//! The crate works on a package's `(name, bytes)` parts, so all the zip handling a caller needs is here.

use std::error::Error;
use std::io::{Cursor, Read, Write};
use std::process::ExitCode;

use docx_fill::{FillOptions, PackageTemplate};
use serde_json::Value;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

type Outcome<T> = Result<T, Box<dyn Error>>;

/// Fills the zipped package in `docx` and zips it again.
pub fn fill_docx(docx: &[u8], data: &Value) -> Outcome<Vec<u8>> {
    let template = PackageTemplate::parse(read_parts(docx)?, &FillOptions::default())?;
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, bytes) in template.render(data) {
        zip.start_file(name, options)?;
        zip.write_all(&bytes)?;
    }
    Ok(zip.finish()?.into_inner())
}

/// The package's parts, in the order the zip holds them.
fn read_parts(docx: &[u8]) -> Outcome<Vec<(String, Vec<u8>)>> {
    let mut zip = ZipArchive::new(Cursor::new(docx))?;
    let mut parts = Vec::with_capacity(zip.len());
    for index in 0..zip.len() {
        let mut part = zip.by_index(index)?;
        if part.is_dir() {
            continue;
        }
        let mut bytes = Vec::new();
        part.read_to_end(&mut bytes)?;
        parts.push((part.name()?.into_owned(), bytes));
    }
    Ok(parts)
}

fn run(input: &str, data: &str, output: &str) -> Outcome<()> {
    let docx = std::fs::read(input).map_err(|error| format!("{input}: {error}"))?;
    let values = std::fs::read(data).map_err(|error| format!("{data}: {error}"))?;
    let values = serde_json::from_slice(&values).map_err(|error| format!("{data}: {error}"))?;
    let filled = fill_docx(&docx, &values).map_err(|error| format!("{input}: {error}"))?;
    std::fs::write(output, filled).map_err(|error| format!("{output}: {error}"))?;
    Ok(())
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [input, data, output] = arguments.as_slice() else {
        eprintln!("Usage: fill_docx <in.docx> <data.json> <out.docx>");
        return ExitCode::from(2);
    };
    match run(input, data, output) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
