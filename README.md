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

With expressions enabled, assignments store values for the current fill and print nothing: `{total = price * count}{total}`. Semicolon-separated expressions print their last result. Bare names are assigned on the current loop item; property assignments update the visible object. The caller's JSON remains unchanged. Object values are copied, and array assignments are limited to index 1,000,000.

`FillOptions::footnotes` controls inserted footnotes. `FootnoteContent::Raw` accepts complete Word runs. `ref_properties`, `ref_run`, `paragraph_properties` and `run_properties` customize the note marker, separator and text. JSON uses `footnotes: { raw, refProperties, refRun, paragraphProperties, runProperties }`. XML fragments must be valid WordprocessingML.

`FillOptions::table` accepts `TableValidation::Ignore` to drop other text around a full-table tag, and `TableResize::KeepColumns` to resize vertical tables while retaining column widths. JSON uses `table: { paragraphValidation: "ignore", autoResizeTable: true }`. Table data can supply `fixedColumns`, with non-null entries spanning the body rows of each table chunk. Headers remain separate; cell spans stop before a fixed column.

`FilledDocument::bookmarks` lists named bookmarks across package parts. `get_xml` reads their raw content and `replace_xml` replaces it while retaining the boundary marks. Replacements must leave balanced XML and paired bookmarks. JSON edits use `getBookmarks`, `getXml` and `replaceXml`; the latter two take `bookmark` and an optional `part` to disambiguate names.

`FillOptions::tag_prefixes` configures loop, inverted-loop, raw XML, image, HTML and raw-run prefixes. JSON accepts the same fields under `tagPrefixes` with camelCase names. Core prefixes must be nonempty, and all prefixes must be distinct. An empty extension prefix handles otherwise unmatched tags; the longest enabled prefix wins. Existing prefixes remain the defaults.

`FillHooks::image_size` can return an exact `ImageSize` after the image is loaded. Returning `None` keeps container fitting. Existing `Image` values need no changes.

`FillHooks::svg_fallback` can return PNG bytes for an SVG image. The package then includes both media files and uses the PNG in readers without SVG support. The caller supplies the preview; the crate does not rasterize SVG.

`FillHooks::image_properties` returns `ImageProperties` for alt text, names, paragraph alignment, solid borders, links and captions. Configure them with the `alt`, `name`, `align`, `border`, `link` and `caption` builders. `ImageBorder::new` takes a hex color and a width in points. Captions are paragraphs after the image paragraph; they do not use inline text boxes. `ImageCaption` supports prefix, sequence, style and alignment.

`ImageProperties::floating` accepts an `ImageFloat` with offsets, reference frames, wrap mode, layer and margins. `rotate`, `flip_horizontal` and `flip_vertical` transform either inline or floating pictures. Offsets accept `ImageLength`; invalid coordinates and reference-frame combinations are rejected. Tight and through wrapping use the rectangular image outline.

An image tag in an existing picture's alt text or name replaces that picture through `FillHooks::image`. Its size, position, crop and rotation stay unchanged unless `image_size` returns explicit dimensions. Tags use the configured delimiters and prefixes and resolve in the current loop scope. A missing value keeps the original picture.

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

A syntax error or unknown filter is an error when parsing. Names such as `table:abc.value` are not expressions, so turn this on per template. Assignments (`{a = 1}`) are refused, and calls give nothing, as JSON holds no functions.

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

For callers that pass settings as JSON, such as a wasm build, `FillOptions::read_json` reads every option (camelCase keys, unknown keys refused), `read_include_options` a subtemplate's options, and `FilledDocument::run_json` a list of text edits.

## Contributing

Run `cargo test` and `cargo clippy --all-targets -- -D warnings`. See [CONTRIBUTING.md](CONTRIBUTING.md).

## Sources

Behaviour comes from the WordprocessingML spec (ECMA-376), docxtemplater's public tag syntax and documentation, and black-box runs of the MIT-licensed docxtemplater core. Extension tags follow the paid modules' public documentation. No code is taken from docxtemplater, and nothing is tuned against the paid modules' output. Password hash test vectors come from black-box runs of Apache POI; for characters over 0x7F the crate follows the spec's unsigned bytes where POI sign-extends them.

HTML `<a>` elements create hyperlinks.

Not affiliated with docxtemplater.

## Licence

MIT. See [LICENSE](LICENSE). Contributions are under the same licence.
