# docx-fill

Fills DOCX templates written in docxtemplater's public tag syntax. Pure Rust, with `serde_json` as its only dependency, so it builds for wasm.

```toml
[dependencies]
docx-fill = { git = "https://github.com/finwarman/docx-fill", rev = "<commit>" }
```

```rust
use docx_fill::{FillOptions, PackageTemplate};
use serde_json::json;

let template = PackageTemplate::parse(parts, &FillOptions::default())?;
let filled = template.render(&json!({ "name": "Value" }));
```

`parts` is the package's `(name, bytes)` pairs. The body, headers and footers are filled; other parts pass through.

[`examples/fill_docx.rs`](examples/fill_docx.rs) shows the whole path, unzipping a `.docx`, filling it and zipping it again with the `zip` crate:

```sh
cargo run --example fill_docx -- in.docx data.json out.docx
```

## Core tags

| Tag | Does |
|---|---|
| `{name}` | Prints a value. `\n` in a value is a line break. |
| `{#list}` … `{/list}` | Repeats for each item, or once for a truthy value. |
| `{^list}` … `{/list}` | Prints for an empty or falsy value. |
| `{-w:tr rows}` … `{/rows}` | Repeats the enclosing element, here a table row. |
| `{@xml}` | Replaces its paragraph with raw XML. |

A loop whose tags sit alone in their paragraphs repeats whole paragraphs (`paragraph_loop`). Delimiters can be anything, such as `[[ ]]`, `{{ }}` or `<< >>`. Tags starting with a prefix in `module_prefixes` go to the caller's `FillHooks`. `FillHooks::missing_value` decides what a missing or null value prints, as docxtemplater's `nullGetter` does; by default it prints nothing. `FillHooks::tag_value` reads a tag's name in your own syntax, as docxtemplater's `parser` does; by default names are looked up whole.

## Extensions

Each is a flag on `FillOptions::extensions`. All are on by default; `Extensions::NONE` turns them off.

| Flag | Tags | Does | docxtemplater module |
|---|---|---|---|
| `vertical_tables` | `{:vt#cols}` … `{:vt/cols}` | Repeats columns, including ranges across rows | Vertical table |
| `cell_styling`, `styling` | `{:stylecell}`, `{:stylepar}`, `{:stylerow}`, `{:stylerun}`, `{:stylebullets}`, `{:styleshape}`, `{:stylestart}` … `{:/style}`, `{::stylepar}`, `{::stylerun}` | Styles a cell, paragraph, row, run, bullet, shape or style definition | Styling |
| `table_merge` | `{:merge-cells-col 1,2}`, `{:merge-cells-row 1,-1}` | Merges neighbouring cells with the same text | Table |
| `full_tables` | `{:table name}`, `{:#grid name}` … `{:/grid}` | Builds a table from data, or a grid of cells | Table |
| `paragraph_placeholder` | `{?name}` | Drops, repeats or keeps its paragraph | Paragraph placeholder |
| `word_run` | `{r@name}` | Replaces its run with raw `<w:r>` XML | Word run |
| `footnotes` | `{:footnote name}` | Adds a footnote | Footnotes |
| `rich_text` | `{~~html}`, `{~html}` | Prints HTML as paragraphs, or inline | HTML |
| `html_extras` | | Adds links, tables, code, quotes, checkboxes and inline CSS to HTML | HTML |
| `images` | `{%image}`, `{%%image}` | Draws an image from `FillHooks::image`, sized to its cell or page | Image |
| `charts` | `{$name}` | Fills an existing bar chart from categories and series | Chart |
| `centered_images` | `{%%image}` | Centres an image alone in its paragraph | Image |
| `subtemplates` | `{:include name}`, `{:segment name}`, `{:includesegment name}` | Prints other documents or segments, with their styles, lists, media and links | Subtemplate |
| `sections` | `{:subsection name}`, `{:replacesection name}`, `{:page props}` | Takes headers and footers from a subtemplate, or sets page size and margins | Subsection |
| `comments` | `{name}` with `{ "text", "comment" }` | Prints text with Word comments on it | |

Package rendering writes HTML lists as native Word lists, with numbering definitions, nesting, ordered starts and item-value resets. Existing numbering IDs are preserved. Word supports nine nesting levels; deeper HTML lists use the ninth level. Standalone `Template` and `fill_part` rendering keep literal markers because they do not return package parts.


HTML `<img>` data URLs support PNG, JPEG, GIF and SVG. Inline `<svg>` also embeds its source. Width and height come from CSS or attributes, then intrinsic dimensions; one supplied dimension preserves the aspect ratio. CSS pixels, points, inches, centimeters, millimeters and picas are accepted. Percentage dimensions use the document body width. `FillHooks::image` can resolve a `src` string, and `image_size`, `image_properties` and `svg_fallback` also apply. External URLs require a hook; the library does not fetch them. Image captions follow the containing template paragraph.

HTML table cells support combined `rowspan` and `colspan`. Vertical spans keep later cells in their columns and stop at the table's final row.

HTML CSS maps margins and padding to paragraph spacing/indents or table-cell margins. It also reads line height, text indentation, cell vertical alignment, page-break boundaries, `white-space: pre` and `direction`. Lengths accept `px`, `pt` and `em`; line height also accepts a multiplier or percentage. This is a Word layout mapping, not a full CSS box model.

`FillOptions::html_style_sheet` (`htmlStyleSheet` in JSON) and HTML `<style>` elements provide style rules. Selectors support elements, `*`, classes, IDs, selector lists, descendants, children, adjacent siblings and attribute tests. Cascade order, specificity and `!important` apply; inline styles win at equal importance. Pseudo-selectors, namespace selectors and CSS escapes are not supported. Selector chains are limited to 64 steps.

HTML validation is available through `Template::try_render[_with]` and `PackageTemplate::try_render[_with]`. These return `FillError` diagnostics for unknown tags, malformed CSS and invalid block placement, including errors in templated includes. Existing `render[_with]` methods keep their best-effort behavior. `html: {ignoreUnknownTags: true, ignoreCssErrors: true, onInvalidBlock: "inline"}` controls validation. Block modes are `inline` (the default, joining paragraphs with breaks), `block` (replace the containing paragraph), and `error`.

`HtmlBlockMode::Preserve` (`html: {onInvalidBlock: "preserve"}`) keeps template text around a block insertion in separate paragraphs. It applies to `{~~html}` and to `{~html}` values containing block elements. Paragraphs and tables remain blocks; inline-only `{~html}` values stay inline. Empty values do not split the paragraph, and empty surrounding fragments are omitted. Resulting paragraphs copy paragraph properties and each text fragment keeps its run formatting; section boundaries stay on the final fragment. This opt-in mode extends the documented `block`, `inline` and `error` modes.

`FillHooks::html_customize` returns `HtmlCustomization` with CSS or a paragraph style. `html_style_transform` maps an element to a paragraph style. `HtmlElement` exposes its name, attributes, inner HTML, text and `matches` for supported selectors with ancestor and sibling context. Register custom element kinds in `html.customTags` (`inline` or `block`), then return Word XML from `html_custom_tag` using `HtmlFragment::inline` or `HtmlFragment::block`. A registered tag with no fragment renders its children. Custom XML must be balanced; callers supply valid Word structure and existing style IDs. The callbacks are synchronous.

Inline HTML keeps spaces needed to separate surrounding text, including adjacent HTML values. Normal HTML whitespace collapses at line boundaries; the template's own spaces remain unchanged.

A style value looks like this. `fontSize` is in half-points. It can also include `text`, which is printed at the styling tag and escaped as Word text.

```json
{ "cellBackground": "#rrggbb", "textColor": "#f00", "textAlign": "center", "pStyle": "Heading1",
  "fontSize": 24, "fontFamily": "Arial", "bold": true, "text": "Hello", "borders": { "top": { "val": "single", "sz": 4, "color": "#000" } } }
```

After parsing a DOCX package, set theme colors before rendering:

```rust
use std::collections::BTreeMap;

template.set_theme_colors(BTreeMap::from([
    ("a:dk1".to_string(), "000000".to_string()),
    ("a:accent1".to_string(), "4F81BD".to_string()),
]))?;
```

The method updates standard slots in `word/theme/theme1.xml`; color values accept three- or six-digit hex with an optional `#`.

## Other options

Parse failures expose a stable `FillErrorId`, the tag and its paragraph and byte offset when known. `FillError::diagnostics()` lists all collected errors, including errors in other package parts. Offsets count bytes in each paragraph's joined XML text before entity decoding. The existing `part` and `message` fields remain available.

With expressions enabled, assignments store values for the current fill and print nothing: `{total = price * count}{total}`. Semicolon-separated expressions print their last result. Bare names are assigned on the current loop item, or on the nearest object scope inside a loop over plain values; filters apply after the assignment, so `{a = b | upper}` stores `b` and prints it upper-cased; property assignments update the visible object. The caller's JSON remains unchanged. Object values are copied, and array assignments are limited to index 1,000,000.
Paragraph placeholders inside text boxes expand to the whole shape by default. Set `paragraphPlaceholder: { expandToShape: false }` to target only the paragraph. `expandSoftBreaksInShapes: true` targets individual soft lines within shapes; `expandSoftBreaks: true` also enables this outside shapes. Surviving lines remain joined by soft breaks. When this mode expands a paragraph, parse errors retain their IDs and tags but omit source positions.


Place `{$sales}` alone in the paragraph after a bar chart, in the chart's paragraph, or in its title. Package rendering accepts `{"sales":{"categories":["A","B"],"series":[{"title":"Revenue","data":[10,20]}]}}`. Series titles are optional; omitted titles keep the template title. Numeric data and `null` gaps must match the category count. Chart layout, series formatting and number formats are preserved. Loops create separate chart parts for each value.

An inline chart tag targets the nearest preceding chart in its paragraph. A tag in the following paragraph requires exactly one chart in the preceding paragraph; multiple charts are ambiguous and cause a parse error. A tag before a chart drawing cannot target a chart in an earlier paragraph.

Category data fills bar, line, area, radar, pie, doughnut and surface charts, including supported 3D variants and pie-of-pie charts. Pie charts accept one series; doughnut charts accept multiple series. Title tags need one chart in their drawing paragraph. Combined plots, other chart types and invalid data keep the original chart. By default, the generated chart uses literal data; its embedded workbook stays unchanged and is detached from the generated chart. JSON can disable this feature with `extensions.charts`.

With the optional `chart-workbook` Cargo feature, set `chartData: "workbook"` (Rust: `ChartDataMode::Workbook`) to clone each chart's embedded XLSX and add a new data worksheet. The generated chart references that sheet, including cached values, so Edit Data uses the new values. Existing worksheet cells, formulas and formatting remain; only sheet-selection metadata changes. New cells use General formatting while chart number formats remain.

Use `PackageTemplate::try_render` or `try_render_with` to receive `chart_workbook_error` diagnostics. Unsupported updates keep the original chart and workbook together in infallible rendering. Workbook mode requires nonempty standard chart ranges or cached ChartEx dimensions, and an internal, unsigned XLSX workbook without structure protection. ChartEx string levels must have equal row counts; numeric dimensions support one level. Hierarchy columns run from parent to leaf while caches remain leaf-first. External workbooks, empty ranges, and malformed data are rejected. Archives are limited to 64 MiB compressed, 256 MiB declared contents and 10,000 entries; decoded XML is limited to 16 MiB, generated chart data to one million points and cell text to Excel's 32,767 characters. Literal mode remains the default.


Scatter charts accept `{"series":[{"title":"Points","data":[{"x":1,"y":2},{"x":3,"y":4}]}]}` without categories. Bubble points also require a numeric `size`. A `null` point or coordinate leaves a gap. Optional point `name` values become labels. Existing axis formats, markers, smooth lines, label styling and bubble effects are preserved. Scatter data with sizes, or bubble data missing required sizes, leaves the chart unchanged.

Office 2016 ChartEx sunburst, treemap, waterfall and region-map charts accept one series. Sunburst and treemap data can use recursive `categories: [{name, categories: [...]}]`, with a numeric `value` on each leaf. Waterfalls also accept `{categories: [...], data: [...]}`. Region maps update values only when category names and order match the template, preserving its geography cache and entity IDs. New regions need resolution by Office or the geographic provider and leave the template unchanged here.

ChartEx elements and chart references accept namespace aliases and default namespaces. `AlternateContent` uses the first supported chart choice for binding. Fallback chart and image assets remain unchanged; previews for older readers are not regenerated.

`FillOptions::footnotes` controls inserted footnotes. `FootnoteContent::Raw` accepts complete Word runs. `ref_properties`, `ref_run`, `paragraph_properties` and `run_properties` customize the note marker, separator and text. JSON uses `footnotes: { raw, refProperties, refRun, paragraphProperties, runProperties }`. XML fragments must be valid WordprocessingML.

`FillOptions::table` accepts `TableValidation::Ignore` to drop other text around a full-table tag, and `TableResize::KeepColumns` to resize vertical tables while retaining column widths. JSON uses `table: { paragraphValidation: "ignore", autoResizeTable: true }`. Table data can supply `fixedColumns`, with non-null entries spanning the body rows of each table chunk. Headers remain separate; cell spans stop before a fixed column.

`FilledDocument::bookmarks` lists named bookmarks across package parts. `get_xml` reads their raw content and `replace_xml` replaces it while retaining the boundary marks. Replacements must leave balanced XML and paired bookmarks. JSON edits use `getBookmarks`, `getXml` and `replaceXml`; the latter two take `bookmark` and an optional `part` to disambiguate names.

`FilledDocument` also supports `set_hyperlink_base`, `page_breaks`, `drop_content_between`, `loop_tags`, `search_regex` and `replace_all_regex`. Page-range removal accepts `ContentBoundary::{Start, End, PageBreak(index)}` and removes the boundary breaks. Its endpoints must be in body paragraphs, not tables or text boxes. Automatic page layout is not counted. Loop tags are read before filling. Regex methods use Rust regex syntax and capture replacements; look-around and backreferences in patterns are not supported.

JSON edit names are `setHyperlinkBase` (`url`), `getPageBreaks`, `dropContentBetween` (`start`/`end`, each `"start"`, `"end"` or a break index), `getLoopTags`, `searchRegex` (`pattern`) and `replaceAllRegex` (`pattern`, `text`).

`FillOptions::tag_prefixes` configures loop, inverted-loop, raw XML, image, HTML and raw-run prefixes. JSON accepts the same fields under `tagPrefixes` with camelCase names. Core prefixes must be nonempty, and all prefixes must be distinct. An empty extension prefix handles otherwise unmatched tags; the longest enabled prefix wins. Existing prefixes remain the defaults.

`FillHooks::image_size` can return an exact `ImageSize` after the image is loaded. Returning `None` keeps container fitting. Existing `Image` values need no changes.

`FillHooks::svg_fallback` can return PNG bytes for an SVG image. The package then includes both media files and uses the PNG in readers without SVG support.

The optional `svg-preview` Cargo feature generates these PNGs automatically, including for HTML images and replaced pictures. It renders clip paths, masks, patterns and filters. A hook override still takes precedence. Previews use bundled Noto Sans; `svg_fonts` can supply additional outline-only TrueType/OpenType fonts. Font collections and embedded SVG/bitmap/color glyph tables are not accepted. No fonts or referenced images are read from the filesystem or network.

Preview dimensions are capped at 4096 pixels per side. Resolution is reduced when effects exceed the 32-million cumulative surface-pixel budget; previews that still exceed the budget at 128 pixels on their longest side retain native SVG. Filter work is bounded separately. Input is limited to 8 MiB, embedded resources to 16 MiB, supplied fonts to 32 MiB and resource depth to 64. Invalid or recursive resources, external images, foreignObject content and missing glyphs retain native SVG. Stylesheet resource URLs or inheritance and context paint also retain native SVG; inline resource styles work. Callers can supply their own PNG through `svg_fallback`.

`FillHooks::image_properties` returns `ImageProperties` for alt text, names, paragraph alignment, solid borders, links and captions. Configure them with the `alt`, `name`, `align`, `border`, `link` and `caption` builders. `ImageBorder::new` takes a hex color and a width in points. Captions are paragraphs after the image paragraph; they do not use inline text boxes. `ImageCaption` supports prefix, sequence, style and alignment.

`ImageProperties::floating` accepts an `ImageFloat` with offsets, reference frames, wrap mode, layer and margins. `rotate`, `flip_horizontal` and `flip_vertical` transform either inline or floating pictures. Offsets accept `ImageLength`; invalid coordinates and reference-frame combinations are rejected. Tight and through wrapping use the rectangular image outline.

An image tag in an existing picture's alt text or name replaces that picture through `FillHooks::image`. Its size, position, crop and rotation stay unchanged unless `image_size` returns explicit dimensions. Tags use the configured delimiters and prefixes and resolve in the current loop scope. A missing value keeps the original picture.

With the `qr-codes` cargo feature, `Extensions::qr_codes` also reads image tags encoded in QR pictures. The feature is off by default because it adds an image decoder. JSON uses `extensions.qrCodes`. PNG, JPEG, GIF, BMP and TIFF sources are supported, up to 8192 pixels per side and 128 MiB of decoder allocation. An existing alt-text tag takes precedence. Ambiguous QR payloads, unreadable images, non-image tags and drawings with more than one picture keep the original picture. The original picture and alt text remain when no replacement image is supplied.

Automatic sizing insets the width and maximum page height by 5% while preserving the image's aspect ratio.

```rust
use docx_fill::{ImageLength, ImageSize};

let pixels = ImageSize::pixels(120.0, 80.0);
let physical = ImageSize::new(ImageLength::Inches(2.0), ImageLength::Centimeters(2.54));
```

Dimensions accept pixels at 96 DPI, inches, centimeters, millimeters, points, picas and EMUs. Constructors return `None` for non-finite, non-positive or out-of-range Word dimensions. Explicit sizes are not padded or fitted to the page or cell.

| Option | Does |
|---|---|
| `drop_empty_last_page` | Drops an empty last page left by a loop that printed nothing |
| `drop_style_paragraphs` | Drops the paragraphs of `{::stylepar}` tags |
| `comments` | Default author and date for comments |
| `continued_bookmarks` | Bookmark prefix for tag marks: rich text numbers later paragraphs' bookmarks, and loops keep these names |
| `document` | Read-only or password protection, a text watermark, page margins and proofing language |
| `expressions`, `filters` | Reads tag names as expressions, and names the filters they can use. See below. |

Always on: a table whose rows a loop removed goes, cells, headers, footers and text boxes keep the empty paragraph the spec requires, and ids a loop copies are renumbered.

## Expressions

With `FillOptions::expressions` on, a tag's name is an expression, as in docxtemplater's expression parser. It works in every tag that reads a value.

| Tag | Does |
|---|---|
| `{user.name}`, `{users[0].name}`, `{users.length}` | Reads properties, indexes and lengths |
| `{price * 1.2}`, `{first + ' ' + last}` | Does arithmetic with `+ - * / %` |
| `{#items.length > 1}` … `{/}` | Compares with `== != === !== < > <= >=` |
| `{a && b}`, `{a \|\| b}`, `{!a}`, `{a ? b : c}` | Logic and a ternary |
| `{#items}{$index + 1}. {.}{/items}` | The loop's index, and the current item as `.` or `this` |
| `{name \| upper}`, `{price \| round:2}` | Applies a filter |

A filter is named in `FillOptions::filters` and applied by `FillHooks::filter`. A name is looked up from the innermost scope out, and a missing result is tried again from the parent scope.

A syntax error or unknown filter is an error when parsing. Names such as `table:abc.value` are not expressions, so turn this on per template. Calls give nothing, as JSON holds no functions.

## Listing tags

`Template::tags` and `PackageTemplate::tags` list each tag with its name, `TagType` and the text as written. A loop's, grid's or segment's tags are its children, so the list follows the shape of the data.

## Text edits

`FilledDocument` works on a filled package's body text.

| Method | Does |
|---|---|
| `search` | Finds text within a paragraph. `\n` matches a line break, `\t` a tab. |
| `replace`, `replace_all` | Changes found text in the run of its first character |
| `add_comment` | Puts a Word comment around a found range |
| `comments` | Lists each comment with the text and XML it marks |

Positions are a paragraph index and a character offset.

`FilledDocument::annotate_errors` adds Word comments at the located diagnostics from a `FillError`, using `CommentDefaults` for their author and date. It supports body and header/footer paragraphs. Errors without a usable paragraph location are left unannotated; the original diagnostics remain available.

For callers that pass settings as JSON, such as a wasm build, `FillOptions::read_json` reads every option (camelCase keys, unknown keys refused), `read_include_options` a subtemplate's options, and `FilledDocument::run_json` a list of text edits.

## Contributing

Run `cargo test` and `cargo clippy --all-targets -- -D warnings`. See [CONTRIBUTING.md](CONTRIBUTING.md).

## Sources

Behaviour comes from the WordprocessingML spec (ECMA-376), docxtemplater's public tag syntax and documentation, and black-box runs of the MIT-licensed docxtemplater core. Extension tags follow the paid modules' public documentation. No code is taken from docxtemplater, and nothing is tuned against the paid modules' output. Password hash test vectors come from black-box runs of Apache POI; for characters over 0x7F the crate follows the spec's unsigned bytes where POI sign-extends them.

HTML `<a>` elements create hyperlinks.

Not affiliated with docxtemplater.

## Licence

MIT. See [LICENSE](LICENSE). Contributions are under the same licence.
