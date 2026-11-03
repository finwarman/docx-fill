# Excel templates

See the [README](../README.md) for the DOCX tags these templates share.

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
