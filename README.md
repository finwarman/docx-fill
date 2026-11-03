# docx-fill

A Rust library for filling Word documents from JSON data. Templates use docxtemplater's tag syntax. It also builds for WebAssembly.

## Usage

Add the library with a pinned commit:

```toml
[dependencies]
docx-fill = { git = "https://github.com/finwarman/docx-fill", rev = "<commit>" }
serde_json = "1"
```

A template containing `Hello {name}` can be filled with `{"name": "Ada"}`:

```rust
use docx_fill::{FillOptions, PackageTemplate};
use serde_json::json;

let template = PackageTemplate::parse(parts, &FillOptions::default())?;
let filled = template.try_render(&json!({ "name": "Ada" }))?;
```

`parts` is a list of `(String, Vec<u8>)` pairs from the DOCX archive. The result contains the filled package parts, ready to zip into a DOCX file. Text retains the template's formatting.

The [file example](examples/fill_docx.rs) handles reading and writing the archive. From a checkout:

```sh
cargo run --example fill_docx -- template.docx data.json filled.docx
```

## Template tags

| Tag | Meaning |
|---|---|
| `{name}` | Inserts a value. A newline becomes a line break. |
| `{#items}` … `{/items}` | Repeats for each item, or once for a truthy value. |
| `{^items}` … `{/items}` | Includes content when the value is empty or falsy. |
| `{-w:tr rows}` … `{/rows}` | Repeats a table row. |
| `{@xml}` | Replaces the paragraph with raw Word XML. |

Missing or null values produce no text. Names are looked up as whole keys unless expressions are enabled. When loop tags each occupy their own paragraph, the paragraphs between them repeat.

## Extensions

Extensions support HTML, images, charts, table generation and merging, styling, footnotes, comments, and included documents or sections. They are enabled through `FillOptions::extensions` and are on by default. Use `Extensions::NONE` for core tags only.

The [reference](docs/reference.md) lists extension tags, options and limits:

- [HTML](docs/reference.md#html): paragraphs, lists, headings, links, tables and styles.
- [Images](docs/reference.md#images): sizing, captions, floating pictures and SVG.
- [Charts](docs/reference.md#charts): category, scatter, bubble and supported ChartEx charts.
- [Expressions](docs/reference.md#expressions): property access, arithmetic, conditions and filters.
- [Text edits](docs/reference.md#text-edits): search, replacement, bookmarks and comments.

These Cargo features are optional:

| Feature | Adds |
|---|---|
| `qr-codes` | Image tags read from QR pictures. |
| `svg-preview` | PNG previews for SVG images. |
| `chart-workbook` | Updates to embedded chart workbooks. |

## Options and errors

`FillOptions` controls delimiters, expressions, extension flags and document settings. `FillHooks` supplies images, custom values and filters. JSON callers can use `FillOptions::read_json`; keys use camelCase and unknown keys are rejected. See [options](docs/reference.md#options).

Parsing and `try_render` return `FillError`. Use `diagnostics()` to inspect errors across package parts. `render` is also available for best-effort output.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for checks and source requirements, and [BENCHMARKS.md](BENCHMARKS.md) for performance measurements.

## Licence

[MIT](LICENSE). This project is not affiliated with docxtemplater.
