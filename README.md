# docx-fill

A Rust library for filling Word documents from JSON data, builds for Webassembly.  
Templates use `docxtemplater` tag syntax. 

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

## Excel templates

`WorkbookTemplate` fills `.xlsx` and `.xltx` packages through the same parts-based boundary as DOCX:

```rust
use docx_fill::{FillOptions, WorkbookTemplate};
use serde_json::json;

let template = WorkbookTemplate::parse(parts, &FillOptions::default())?;
let filled = template.render(&json!({"name": "Alice", "amount": 12.5}))?;
```

`WorkbookTemplate::render` and `render_with` return `Result`. Unsupported structural edits return errors. `sheets()` lists worksheet names and part paths in workbook order. Chart sheets and unrelated package parts pass through. The core API does not add a ZIP dependency.

Run the complete file example with:

```sh
cargo run --example fill_xlsx -- template.xlsx data.json output.xlsx
```

Cell tags work in shared strings and inline strings, including tags split across rich-text runs. A whole-cell `{amount}` writes a JSON number as an Excel number and a boolean as an Excel boolean. A string stays text, including strings beginning with `=`. Missing or null values leave blank cells unless `FillHooks::missing_value` supplies text. Mixed text such as `Total: {amount}` stays text. Existing cell styles remain attached. Generated string text uses the first text run's formatting; finer rich-text formatting is follow-up work.

Explicit formulas use `{"type":"formula","value":"A1+B1","result":42}` as the tag value. `result` is optional. Formula text omits the leading `=`. The library does not calculate formulas. Changed input cells cause existing cached formula results and the calculation chain to be cleared; the workbook requests recalculation when opened.

Place `{#items}` in the first cell and `{/items}` in the last cell of a row to repeat that row. Markers may also span several rows. Empty arrays remove their rows. `{^items}` supplies the inverted case. Paired tags within one cell repeat text within that cell. Nested loops, parent data lookup, custom delimiters and the existing expression/filter hooks are supported. Expression assignments are not supported in Excel cells yet.

Row expansion updates cell addresses, supported A1 formula references and worksheet ranges. The initial version rejects structural edits involving cross-sheet formulas, defined names, shared/array formulas, drawings, tables, pivots, hyperlinks, validation or conditional-formatting formulas, manual page breaks, or worksheet extensions. Horizontal loops, sheet cloning, date descriptors, dynamic styles, images and HTML are tracked in the [Excel roadmap](https://github.com/finwarman/docx-fill/issues/168). Existing date cells, styles and other untouched content are not interpreted. Only standard XLSX/XLTX workbook types are accepted; macro-enabled, Strict OOXML, encrypted and signed workbooks are not supported.

The parts API limits inputs to 10,000 parts and 256 MiB total, with 16 MiB per parsed XML part. Rendering limits worksheet rows, columns, output cells, nesting and cell text. The file example also limits ZIP size to 64 MiB and checks entry sizes.

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
