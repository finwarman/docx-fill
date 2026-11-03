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

## Supported features

The library works with DOCX templates and JSON data. Extension flags in `FillOptions::extensions` are on by default; use `Extensions::NONE` for core tags only.

| Feature | Support |
|---|---|
| [HTML](docs/reference.md#html) | Paragraphs, headings, nested lists, links, tables and CSS. |
| [Images](docs/reference.md#images) | Image hooks, sizing, captions, floating pictures and SVG previews. HTML image data URLs support PNG, JPEG, GIF and SVG. |
| [Charts](docs/reference.md#charts) | Bar, line, area, radar, pie, doughnut, surface, scatter and bubble charts; ChartEx sunburst, treemap, waterfall and region maps. |
| [Tables](docs/reference.md#extension-tags) | Tables and grids from data, repeating columns and merged cells. |
| [Styling](docs/reference.md#styling-and-themes) | Cells, paragraphs, rows, runs, bullets, shapes and theme colours. |
| [Included content](docs/reference.md#extension-tags) | Subtemplates, named segments, sections, headers and footers. |
| [Footnotes and comments](docs/reference.md#extension-tags) | Inserted footnotes and comments on values. |
| [Paragraphs and XML](docs/reference.md#extension-tags) | Conditional or repeated paragraphs, raw paragraphs and raw runs. |
| [Expressions](docs/reference.md#expressions) | Property access, arithmetic, conditions and filters. |
| [Text edits](docs/reference.md#text-edits) | Search, replacement, bookmarks and comments. |

The [full extension table](docs/reference.md#extension-tags) lists flags and tag syntax. The reference also covers [options and limits](docs/reference.md).

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
