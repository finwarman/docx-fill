//! Fills each case in a JSON file with core options and prints one JSON line per case, for the
//! differential tests in `differential/`:
//!
//! ```sh
//! cargo run --release --example differential_fill -- cases.json
//! ```
//!
//! A case is `{ "body": "<w:p>…</w:p>", "data": {…}, "options": {…} }`, with options as
//! `FillOptions::read_json` reads them. Each line is `{ "out": "<xml>" }` or `{ "error": "…" }`.

use std::process::ExitCode;

use docx_fill::{FillOptions, fill_part};
use serde_json::{Value, json};

const DOCUMENT_START: &str = r#"<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>"#;
const DOCUMENT_END: &str = "</w:body></w:document>";

fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("Usage: differential_fill <cases.json>");
        return ExitCode::from(2);
    };
    let cases: Vec<Value> = match std::fs::read(&path).map_err(|error| error.to_string()).and_then(|bytes| serde_json::from_slice(&bytes).map_err(|error| error.to_string())) {
        Ok(cases) => cases,
        Err(error) => {
            eprintln!("{path}: {error}");
            return ExitCode::FAILURE;
        }
    };
    for case in &cases {
        println!("{}", fill_case(case));
    }
    ExitCode::SUCCESS
}

fn fill_case(case: &Value) -> Value {
    let options = match FillOptions::core().read_json(&case["options"]) {
        Ok(options) => options,
        Err(error) => return json!({ "error": format!("options: {error}") }),
    };
    let xml = format!("{DOCUMENT_START}{}{DOCUMENT_END}", case["body"].as_str().unwrap_or_default());
    match std::panic::catch_unwind(|| fill_part(&xml, &case["data"], &options)) {
        Ok(Ok(out)) => json!({ "out": out }),
        Ok(Err(error)) => json!({ "error": error.message }),
        Err(_) => json!({ "error": "panic" }),
    }
}
