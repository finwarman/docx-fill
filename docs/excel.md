# Excel templates

`WorkbookTemplate` fills SpreadsheetML packages. It uses the same `(String, Vec<u8>)` input as `PackageTemplate`; `render` and `render_with` return `Result<PackageParts, FillError>`. ZIP handling remains outside the default library dependencies. The [file example](../examples/fill_xlsx.rs) reads an archive, fills it, and writes a new archive.

See the [README](../README.md) for the DOCX tags these templates share.

```rust
use docx_fill::{FillOptions, WorkbookTemplate};
use serde_json::json;

let template = WorkbookTemplate::parse(parts, &FillOptions::default())?;
let filled = template.render(&json!({"name": "Alice", "amount": 12.5}))?;
```

Run the complete file example with:

```sh
cargo run --example fill_xlsx -- template.xlsx data.json output.xlsx
```

## Cell values and types

Tags work in inline strings and shared strings, including tags split across rich-text runs. A whole-cell tag writes JSON numbers and booleans using native Excel cell types. Strings remain text even when they begin with `=`. Missing values leave blank cells unless the missing-value hook supplies text. Existing cell styles remain attached. Simple text replacement retains surrounding rich runs.

Explicit descriptors use a `type` and a `value`:

```json
{
  "amount": {"type": "currency", "value": 12.5, "fmt": "$0.00"},
  "percentage": {"type": "percent", "value": 0.25},
  "day": {"type": "date", "value": "2024-02-29"},
  "time": {"type": "time", "value": "12:30:00"},
  "instant": {"type": "datetime", "value": "2024-02-29T12:30:00Z"},
  "total": {"type": "formula", "value": "SUM(B2:B4)", "result": 42}
}
```

`string`, `float`, `int`, `number`, `boolean` and `cell` descriptors are also available. Formula text omits the leading `=`; its cached `result` is optional. The library updates formula references and requests recalculation, but does not calculate formulas. Existing cached results are cleared when their source data changes. Numeric values are limited to 15 significant decimal digits so Excel does not silently round larger values; use a string when the exact digits must be retained.

Dates use the workbook's 1900 or 1904 date system. The fictional date 1900-02-29 is rejected; dates from 1900-03-01 account for Excel's reserved serial 60. Time is a fraction of one day. Date/time input uses ISO calendar dates and clock times; offsets other than a trailing UTC `Z` are not accepted.

## Styles, links and notes

A `cell` descriptor can attach formatting and annotations:

```json
{
  "type": "cell",
  "value": "Documentation",
  "numberFormat": "@",
  "font": {"bold": true, "color": "0066CC"},
  "alignment": {"horizontal": "left", "wrapText": true},
  "hyperlink": "https://example.com",
  "comment": {"text": "Review this item", "author": "Reviewer"}
}
```

`style` can group the number format, font, fill, border and alignment properties. New formats and style records are deduplicated. Unspecified properties retain the cell's base style. Internal links begin with `#`, for example `#Summary!A1`. External links allow `http`, `https` and `mailto` schemes. Notes use standard comments and VML anchors. Existing workbook content is kept unless the requested edit replaces or expands it.

## Repeated cells, rows and sheets

`{#items}` and `{/items}` in different cells repeat their row range. Empty arrays remove the range. `{^items}` supplies the inverted case. Matching loop markers within a single cell repeat text in that cell. Loops retain parent data lookup and expression/filter hooks.

`{>items}` expands a cell horizontally. A range can use `{>items}` and `{/>items}` in separate cells. Later cells shift to make room. Horizontal expansion operates on the affected row; it does not insert entire worksheet columns. Row and cell maps update supported formula references, merged ranges, validations and table ranges. Explicit row heights and wrapping stay with copied rows. `set_row_height_mode(RowHeightMode::Estimate)` opts into approximate row sizing from newlines, wrapping and column widths; `Preserve` is the default. Estimates never shrink an explicit height and stop at Excel's 409.5-point limit.

Row-local expansion keeps worksheet column-width definitions global. Shared formulas are expanded into ordinary formulas when structural loops require it. Array formulas can move when their complete range remains contiguous; edits that split, repeat or partly delete an array range return errors. Three-dimensional sheet references require consistent transforms. The current check conservatively compares all worksheet maps and rejects a reference when any transform differs. External-workbook references stay unchanged. Pivot-table/cache dependencies, threaded comments and ambiguous structural edits return errors.

Worksheet names can contain value tags. Names are checked for Excel's length, character and uniqueness rules. `repeat_sheet` adds copies with independent data:

```rust
use docx_fill::SheetInstance;
use serde_json::json;

template.repeat_sheet("Template", vec![
    SheetInstance { name: "North".into(), data: json!({"region": "North"}) },
    SheetInstance { name: "South".into(), data: json!({"region": "South"}) },
])?;
```

The source worksheet remains present. `retain_sheets` selects source worksheets before rendering. Copies preserve supported worksheet relationships, including drawings, charts, tables and notes, with separate package parts and IDs. Root data remains visible to copied sheet data. Cycles reuse the cloned parts. Unsupported dependencies and graphs deeper than 128 levels return errors.

`sheets()` lists source worksheet names and part paths. `tags()` lists cell-addressed tags with nested loop children, including images, charts and rich text. Errors include part paths; cell errors include the affected cell where available.

## Rich text and other text surfaces

`{~html}` inserts the supported HTML subset as SpreadsheetML rich-text runs. Basic emphasis, line breaks and inline font properties map to native run formatting. HTML uses a separate spreadsheet renderer; Word XML is not inserted into worksheets. Unsupported elements or invalid rich text return errors.

A `richText` descriptor can supply `runs`, each with `text` and optional `bold`, `italic`, `underline`, `strike`, `font`, `size` or `color`. Alternatively, `raw` accepts validated SpreadsheetML rich-text runs. Raw markup is checked for balanced XML and allowed elements and attributes.

Worksheet headers and footers, and text in drawing shapes, use value tags and inline conditions. Header/footer control sequences such as `&P` remain intact. Ampersands inserted from data are escaped for Excel's header syntax. Shape text retains surrounding run properties and geometry.

## Images and charts

`{%image}` uses `FillHooks::image`, `image_size`, `image_properties` and `svg_fallback`. PNG, JPEG, GIF and SVG media use worksheet drawing relationships and native anchors. Repeated images share identical binary data but retain separate drawing objects. Image hooks supply bytes; the library does not fetch remote resources.

`{$sales}` fills an existing chart on the worksheet using the same categories/series data shape as the Word chart API. If a sheet contains several charts, include `target` with its relationship ID or chart part name. Standard chart caches are tied to a generated hidden data worksheet so the chart references and editable cell data agree. Chart formatting is preserved. Unsupported chart layouts return errors.

## Formats, limits and validation

Standard XLSX/XLTX and macro-enabled XLSM/XLTM workbook types are supported. Existing VBA bytes are preserved. Signed packages, signed VBA projects, encrypted containers and legacy binary XLS files are rejected. Worksheet protection is preserved. Locked workbook structure blocks worksheet renaming, selection and copying. Strict OOXML input is accepted. Modified Strict packages are normalized to Transitional OOXML so newly created parts use a consistent namespace set. Unchanged packages retain their original bytes.

`WorkbookLimits` configures lower output row, cell, byte and loop-depth limits through `set_limits`. Defaults are Excel's 1,048,576 rows and 16,384 columns, one million rendered cells, 256 MiB of package data and 64 loop levels. XML input parts are limited to 16 MiB. Cell text is limited to 32,767 UTF-16 code units. The file example also limits compressed archives to 64 MiB and rejects duplicate and unsafe archive members before filling.

Run the normal Rust tests and the optional independent checks:

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --target wasm32-unknown-unknown
cargo bench --bench spreadsheet
python differential/check_xlsx.py  # requires openpyxl
```

The benchmark reports elapsed time, output bytes and peak tracked allocations for 10,000 and 100,000 rows. Interoperability fixtures include unmodified Excel and LibreOffice files with their provenance and licenses under `tests/fixtures/spreadsheet`.
