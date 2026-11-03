# Reference

See the [README](../README.md) for installation and a working file example.

- [Extension tags](#extension-tags)
- [HTML](#html)
- [Images](#images)
- [Charts](#charts)
- [Expressions](#expressions)
- [Options](#options)
- [Text edits](#text-edits)

## Extension tags

Set flags through `FillOptions::extensions`. All flags default to enabled; QR decoding also requires the `qr-codes` Cargo feature.

| Flag | Tags | Purpose |
|---|---|---|
| `vertical_tables` | `{:vt#cols}` … `{:vt/cols}` | Repeats columns, including ranges across rows. |
| `cell_styling`, `styling` | `{:stylecell}`, `{:stylepar}`, `{:stylerow}`, `{:stylerun}`, `{:stylebullets}`, `{:styleshape}` | Styles cells, paragraphs, rows, runs, bullets or shapes. |
| `styling` | `{:stylestart}` … `{:/style}`, `{::stylepar}`, `{::stylerun}` | Styles a range or changes a style definition. |
| `table_merge` | `{:merge-cells-col 1,2}`, `{:merge-cells-row 1,-1}` | Merges adjacent cells with matching text. |
| `full_tables` | `{:table name}`, `{:#grid name}` … `{:/grid}` | Builds a table or grid from data. |
| `paragraph_placeholder` | `{?name}` | Removes, repeats or keeps its paragraph. |
| `word_run` | `{r@name}` | Replaces a run with raw Word XML. |
| `footnotes` | `{:footnote name}` | Adds a footnote. |
| `rich_text` | `{~~html}`, `{~html}` | Inserts block or inline HTML. |
| `html_extras` | | Enables additional HTML elements and CSS. |
| `images`, `centered_images` | `{%image}`, `{%%image}` | Inserts an image; the second form centres a standalone image. |
| `charts` | `{$name}` | Fills an existing chart. |
| `qr_codes` | Image tag encoded in a QR picture | Replaces the picture. Requires `qr-codes`. |
| `subtemplates` | `{:include name}`, `{:segment name}`, `{:includesegment name}` | Includes documents or named segments. |
| `sections` | `{:subsection name}`, `{:replacesection name}`, `{:page props}` | Uses a subtemplate's headers and footers, or changes page settings. |
| `comments` | `{name}` with `{"text": "...", "comment": "..."}` | Inserts text with a Word comment. |

## HTML

`{~~html}` inserts block content. `{~html}` inserts inline content. With `html_extras` disabled, supported elements are paragraphs, breaks, headings, bold, italic, underline and lists. Enabling it adds links, tables, code, quotes, checkboxes and CSS. HTML `<a>` elements create hyperlinks.

### Lists and headings

`PackageTemplate` creates native Word lists, including nested lists, ordered starts and item-value resets. List text is indented a quarter inch per level with a quarter-inch hanging marker. CSS indents override those defaults. Marker formatting comes from the tag run; bullets use Symbol when that run has no explicit font. Existing numbering IDs are preserved. Nesting beyond Word's nine levels uses the ninth level.

Block headings use `Title` for `h1` and `Heading1` through `Heading5` for `h2` through `h6`. Existing paragraph styles are matched by ID or name and left unchanged. Their font, size and weight replace the tag run's formatting; explicit CSS, emphasis and style hooks can override them. Language and proofing metadata are retained.

Missing heading styles are added when used. They use the theme heading font with Arial as a fallback. Sizes are 28, 20, 16, 14, 12 and 11 points. The title is regular; the other headings are bold.

Set `html.headingMode` to `"direct"` to keep the tag paragraph style and run font. This mode makes `h1` through `h6` bold at 16, 14, 13, 12, 12 and 12 points without adding heading styles. Explicit CSS and custom paragraph styles still take precedence. The default, `"styles"`, uses the paragraph-style mapping above.

Inline headings use direct formatting. `Template` and `fill_part` also use direct heading formatting and literal list markers, since they cannot add package styles or numbering parts.

### Tables

HTML cells support combined `rowspan` and `colspan`. Vertical spans end at the last table row. Side borders cover the full span; the bottom border appears on its last row.

Tables default to a maximum width of six inches, constrained by the section column or containing cell. Explicit table widths or fully specified column widths may exceed six inches. Nested tables subtract the parent cell's padding.

A template cell's width comes from `w:tcW` or its grid columns. Margins come from `w:tcMar`, `w:tblCellMar` or the default table style, falling back to 108 twips per side. Percentage widths are retained in text boxes and cells of unknown width. Unequal page columns use the first column's width.

Generated cells have 55/45/55/55 twips of padding (top/left/bottom/right) and 0.25pt borders. HTML and CSS widths, padding, `cellpadding`, margins and borders override these defaults. `table-layout: fixed` fixes the grid; otherwise Word may adjust preferred widths to fit content.

### CSS

CSS supports paragraph margins, padding, line height, indentation, cell alignment, page breaks, `white-space: pre` and `direction`. These map to Word properties rather than a browser box model. Lengths accept `px`, `pt` and `em`; line height also accepts a multiplier or percentage.

Use `FillOptions::html_style_sheet` (`htmlStyleSheet` in JSON) or HTML `<style>` elements. Supported selectors include elements, `*`, classes, IDs, lists, descendants, children, adjacent siblings and attribute tests. Cascade order, specificity and `!important` apply. Inline styles win at equal importance. Pseudo-selectors, namespace selectors and CSS escapes are unsupported; selector chains are limited to 64 steps.

Inline HTML collapses whitespace at line boundaries while retaining separators between values and the template's own spaces.

### Validation and block placement

`try_render` and `try_render_with` report unknown tags, malformed CSS and invalid block placement, including in templated includes. `render` and `render_with` return best-effort output.

JSON configuration:

```json
{
  "html": {
    "ignoreUnknownTags": true,
    "ignoreCssErrors": true,
    "onInvalidBlock": "inline"
  }
}
```

`onInvalidBlock` controls block HTML inserted alongside template text:

| Mode | Result |
|---|---|
| `inline` (default) | Joins paragraphs with line breaks. |
| `block` | Replaces the containing paragraph. |
| `error` | Reports invalid block placement. |
| `preserve` | Keeps surrounding template text in separate paragraphs. |

`preserve` (`HtmlBlockMode::Preserve` in Rust) applies to `{~~html}` and to `{~html}` values containing block elements. Empty values do not split the paragraph. Surrounding text keeps its run formatting and paragraph properties; the final fragment retains section boundaries.

### Custom HTML

`FillHooks::html_customize` returns CSS or a paragraph style through `HtmlCustomization`. `html_style_transform` maps elements to paragraph styles. `HtmlElement` exposes the element name, attributes, inner HTML, text and selector matching with ancestor and sibling context.

Register custom tags in `html.customTags` as `inline` or `block`, then return `HtmlFragment::inline` or `HtmlFragment::block` from `html_custom_tag`. Without a returned fragment, the tag's children render normally. Hooks are synchronous. Custom Word XML must be balanced and use valid structure and existing style IDs.

## Images

`FillHooks::image` supplies image data for `{%image}` and `{%%image}`. HTML also accepts PNG, JPEG, GIF and SVG data URLs, plus inline `<svg>`. External image URLs require a hook; the library does not fetch them.

An image tag in an existing picture's alt text or name replaces that picture. It keeps its size, position, crop and rotation unless `image_size` overrides the dimensions. A missing value keeps the original picture.

### Size and placement

Automatic sizing preserves aspect ratio and fits the containing cell or page, with a 5% inset. `FillHooks::image_size` can return an explicit `ImageSize`; `None` keeps automatic sizing.

```rust
use docx_fill::{ImageLength, ImageSize};

let pixels = ImageSize::pixels(120.0, 80.0);
let physical = ImageSize::new(ImageLength::Inches(2.0), ImageLength::Centimeters(2.54));
```

Sizes accept pixels at 96 DPI, inches, centimetres, millimetres, points, picas and EMUs. Invalid, non-positive or out-of-range dimensions return `None`. Explicit sizes are not fitted or inset.

HTML images use CSS or attribute dimensions, then intrinsic dimensions. A single dimension preserves aspect ratio. Percentage dimensions use the document body width.

`FillHooks::image_properties` returns `ImageProperties` for alt text, names, alignment, borders, links and captions. Builders include `alt`, `name`, `align`, `border`, `link` and `caption`. `ImageBorder::new` takes a hex colour and a width in points. `ImageCaption` supports a prefix, sequence, style and alignment; captions appear as paragraphs after the containing image paragraph.

`ImageProperties::floating` accepts `ImageFloat` offsets, reference frames, wrapping, layers and margins. `rotate`, `flip_horizontal` and `flip_vertical` also apply to inline pictures. Coordinates use `ImageLength`. Invalid coordinates or reference-frame combinations are rejected. Tight and through wrapping use the rectangular image outline.

### SVG previews

`FillHooks::svg_fallback` can supply a PNG preview. The package then contains both files, allowing readers without SVG support to show the PNG.

The `svg-preview` Cargo feature generates previews automatically, including clip paths, masks, patterns and filters. A hook-supplied PNG takes precedence. Rendering uses bundled Noto Sans; `svg_fonts` accepts additional outline-only TrueType/OpenType fonts. Font collections and embedded SVG, bitmap or colour glyph tables are unsupported. Resources are not loaded from the filesystem or network.

| Preview limit | Value |
|---|---|
| Output size | 4096 pixels per side |
| Cumulative surface pixels | 32 million |
| SVG input | 8 MiB |
| Embedded resources | 16 MiB |
| Supplied fonts | 32 MiB |
| Resource depth | 64 |

Resolution is reduced to stay within the surface budget. If it still exceeds the budget at 128 pixels on the longest side, the image retains native SVG. Filter work is bounded separately. Invalid or recursive resources, external images, `foreignObject`, missing glyphs, context paint and escaped CSS resource names also retain native SVG. Callers can provide a PNG through `svg_fallback`.

### QR pictures

The `qr-codes` Cargo feature enables `Extensions::qr_codes` (`extensions.qrCodes` in JSON). It reads image tags from PNG, JPEG, GIF, BMP and TIFF QR pictures, up to 8192 pixels per side and 128 MiB of decoder allocation.

Alt-text tags take precedence. Unreadable or ambiguous codes, non-image tags and drawings with multiple pictures are left unchanged. Without a replacement image, the original picture and alt text remain.

## Charts

Put `{$sales}` in a chart's paragraph or title, or alone in the following paragraph. An inline tag binds to the nearest preceding chart in its paragraph. A tag in the next paragraph requires exactly one chart in the preceding paragraph. Ambiguous matches are errors.

Category charts accept data such as:

```json
{
  "sales": {
    "categories": ["A", "B"],
    "series": [{"title": "Revenue", "data": [10, 20]}]
  }
}
```

Supported types include bar, line, area, radar, pie, doughnut and surface charts, their supported 3D variants, and pie-of-pie. Pie charts accept one series; doughnut charts accept several. Omitted series titles keep the template title. Numeric values and `null` gaps must match the category count.

Layout, formatting and number formats are retained. Loops create separate chart parts. Unsupported types, combined plots and invalid data leave the chart unchanged.

### Scatter, bubble and ChartEx

Scatter data uses `{"series":[{"title":"Points","data":[{"x":1,"y":2},{"x":3,"y":4}]}]}` without categories. Bubble points also require numeric `size`. Supplying sizes to a scatter chart, or omitting them from a bubble chart, leaves the chart unchanged. Null points or coordinates leave gaps; optional `name` values become labels. Axis formats, markers, smooth lines, label styling and bubble effects are retained.

Office 2016 ChartEx sunburst, treemap, waterfall and region maps accept one series. Sunburst and treemap categories can be nested as `{name, categories: [...]}`, with a numeric `value` on each leaf. Waterfalls accept `{categories: [...], data: [...]}`.

Region maps can reorder or subset regions when the template has unique category names and cached entity IDs. New regions need provider-resolved data:

```json
{
  "map": {
    "entityIds": ["253"],
    "geography": "<cx:geography ...>...</cx:geography>"
  }
}
```

IDs must be unique and follow category order. Geography XML is limited to 8 MiB. Clear caches must contain polygons for every ID; binary caches are checked as base64 but their provider-specific contents are not decoded. Invalid input keeps the original chart. Place lookup requires Office or another provider; removing a cache does not trigger a supported refresh. See [cache examples](../tests/fixtures/map-sources.md).

ChartEx accepts namespace aliases and default namespaces. `AlternateContent` binds to its first supported chart choice. Fallback chart and image assets remain unchanged; previews for older readers are not regenerated.

### Embedded workbooks

By default, charts use literal data and detach the original embedded workbook without changing it.

Enable `chart-workbook` and set `chartData: "workbook"` (`ChartDataMode::Workbook` in Rust) to clone the embedded XLSX and add a data worksheet. The chart references the new sheet and cached values, so Edit Data shows the updated data. Existing cells, formulas and formatting remain; sheet selection changes. New cells use General formatting.

Use `try_render` or `try_render_with` for `chart_workbook_error` diagnostics. Best-effort rendering keeps the original chart and workbook on an unsupported update.

Workbook mode requires nonempty standard chart ranges or cached ChartEx dimensions and an internal, unsigned XLSX without structure protection. ChartEx string levels must have equal row counts; numeric dimensions support one level. Hierarchy columns run parent-to-leaf while caches remain leaf-first.

| Workbook limit | Value |
|---|---|
| Compressed archive | 64 MiB |
| Declared uncompressed contents | 256 MiB |
| Archive entries | 10,000 |
| Decoded XML | 16 MiB |
| Generated chart data | One million cells |
| Cell text | 32,767 UTF-16 code units |

External workbooks, empty ranges and malformed data are rejected.

## Expressions

Set `FillOptions::expressions` to enable expressions in value-reading tags.

| Expression | Purpose |
|---|---|
| `{user.name}`, `{users[0].name}`, `{users.length}` | Property, index and length access. |
| `{price * 1.2}`, `{first + ' ' + last}` | Arithmetic with `+ - * / %`. |
| `{#items.length > 1}` … `{/}` | Comparisons with `== != === !== < > <= >=`. |
| `{a && b}`, `{a \|\| b}`, `{!a}`, `{a ? b : c}` | Boolean operations and conditionals. |
| `{#items}{$index + 1}. {.}{/items}` | Loop index and current item (`.` or `this`). |
| `{name \| upper}`, `{price \| round:2}` | Filters. |

Assignments update the current fill without changing the caller's JSON. `{total = price * count}{total}` stores and then prints a value. Semicolon-separated expressions print their final result. Bare names assign to the current loop item, or the nearest object scope in loops over plain values. Property assignments update the visible object. Filters run after assignment: `{a = b | upper}` stores `b` and prints its uppercase value. Object values are copied; array assignment indices are limited to 1,000,000.

Register filter names in `FillOptions::filters` and implement `FillHooks::filter`. Lookup starts in the innermost scope and retries missing results in parent scopes. Syntax errors and unknown filters fail parsing. Function calls produce no value.

Enable expressions per template: names such as `table:abc.value` are otherwise treated as whole keys.

## Options

`FillOptions::read_json` accepts camelCase keys and rejects unknown keys. `read_include_options` reads subtemplate options.

`FillHooks::missing_value` controls missing/null output. `tag_value` implements custom lookup, and `module_prefixes` routes custom tags to `FillHooks`. Delimiters can be changed, for example to `[[ ]]`.

`tag_prefixes` (`tagPrefixes` in JSON) configures loop, inverted-loop, raw XML, image, HTML and raw-run prefixes. Core prefixes must be nonempty; all prefixes must be distinct. Prefixes already claimed by an enabled fixed-syntax extension are rejected. For example, disable charts before reusing `$`. An empty extension prefix handles unmatched tags; the longest enabled prefix wins.

| Option | Purpose |
|---|---|
| `paragraph_loop` | Repeats whole paragraphs when loop tags occupy separate paragraphs. |
| `linebreaks` | Converts newlines in values to Word line breaks. |
| `drop_empty_last_page` | Removes an empty final page left by an empty loop. |
| `drop_style_paragraphs` | Removes paragraphs containing `{::stylepar}` tags. |
| `comments` | Sets default comment author and date. |
| `continued_bookmarks` | Sets the bookmark prefix retained across repeated paragraphs. |
| `document` | Controls protection, watermarks, margins and proofing language. |

Loop rendering removes empty tables, retains required empty paragraphs in cells, headers, footers and text boxes, and renumbers copied IDs.

### Styling and themes

A styling tag can use a value like this. `fontSize` is in half-points; `text` inserts escaped Word text at the tag.

```json
{
  "cellBackground": "#abc",
  "textColor": "#f00",
  "textAlign": "center",
  "pStyle": "Heading1",
  "fontSize": 24,
  "fontFamily": "Arial",
  "bold": true,
  "text": "Hello",
  "borders": {"top": {"val": "single", "sz": 4, "color": "#000"}}
}
```

Set theme colours on a mutable template after parsing and before rendering:

```rust
use std::collections::BTreeMap;

template.set_theme_colors(BTreeMap::from([
    ("a:dk1".to_string(), "000000".to_string()),
    ("a:accent1".to_string(), "4F81BD".to_string()),
]))?;
```

This updates standard slots in `word/theme/theme1.xml`. Colours accept three- or six-digit hex with an optional `#`.

### Paragraph placeholders

Paragraph placeholders in text boxes replace the whole shape by default. Use `paragraphPlaceholder: {expandToShape: false}` to target only the paragraph.

`expandSoftBreaksInShapes: true` targets individual soft lines in shapes; `expandSoftBreaks: true` also applies outside shapes. Retained lines stay joined by soft breaks. Errors after this expansion keep their IDs and tags but omit source positions.

### Footnotes and generated tables

`FillOptions::footnotes` controls note content and formatting. `FootnoteContent::Raw` accepts complete Word runs. `ref_properties`, `ref_run`, `paragraph_properties` and `run_properties` set the marker, separator and text. JSON uses `footnotes: {raw, refProperties, refRun, paragraphProperties, runProperties}`. XML fragments must be valid WordprocessingML.

`FillOptions::table` accepts `TableValidation::Ignore` to discard text around a full-table tag and `TableResize::KeepColumns` to retain column widths when resizing vertical tables. JSON uses `table: {paragraphValidation: "ignore", autoResizeTable: true}`. Non-null `fixedColumns` entries span body rows within each table chunk. Headers stay separate, and cell spans stop before a fixed column.

### Diagnostics

`FillError` includes a stable `FillErrorId`, the tag, and paragraph and byte offsets when available. `diagnostics()` lists all errors across package parts. Byte offsets refer to the paragraph's joined XML text before entity decoding; `part` and `message` remain available.

## Text edits

`FilledDocument` searches and edits body text. Positions use a paragraph index and character offset.

| Method | Purpose |
|---|---|
| `search` | Finds text within a paragraph; `\n` matches a break and `\t` a tab. |
| `replace`, `replace_all` | Replaces a match in the run containing its first character. |
| `add_comment`, `comments` | Adds comments or reads comments and their marked text/XML. |
| `bookmarks`, `get_xml`, `replace_xml` | Lists named bookmarks and reads or replaces their contents. |
| `search_regex`, `replace_all_regex` | Uses Rust regex syntax and capture replacements. |
| `set_hyperlink_base` | Sets the hyperlink base URL. |
| `page_breaks`, `drop_content_between` | Lists explicit breaks or removes a range between boundaries. |

Bookmark methods cover package parts. Replacements retain boundary marks and must leave balanced XML with paired bookmarks. Regex patterns do not support look-around or backreferences.

`drop_content_between` accepts `ContentBoundary::{Start, End, PageBreak(index)}`. It removes boundary breaks, and both endpoints must be in body paragraphs rather than tables or text boxes. Automatic page layout is not counted.

`Template::tags` and `PackageTemplate::tags` list tag names, `TagType` and source text before filling. Loop, grid and segment tags contain their children. `FilledDocument::loop_tags` reads loop tags from the current package; use it before filling removes them.

`FilledDocument::annotate_errors` adds Word comments at diagnostics with usable body or header/footer locations. `CommentDefaults` supplies author and date; diagnostics without a usable location remain in the error list.

JSON callers can pass edits to `FilledDocument::run_json`:

| Edit | Fields |
|---|---|
| `getBookmarks` | |
| `getXml` | `bookmark`, optional `part` |
| `replaceXml` | `bookmark`, `xml`, optional `part` |
| `setHyperlinkBase` | `url` |
| `getPageBreaks`, `getLoopTags` | |
| `dropContentBetween` | `start`, `end`: `"start"`, `"end"` or a break index |
| `searchRegex` | `pattern` |
| `replaceAllRegex` | `pattern`, `text` |
