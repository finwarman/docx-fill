# Agent guide

Read [CONTRIBUTING.md](CONTRIBUTING.md) first. It covers the CI checks, allowed sources and how to add a feature. This file adds the rules for docs and code style.

## Docs

- The README holds setup, the core DOCX tags and the supported-features summary. Keep it short.
- Detail goes in `docs/`. Tags and options go in [the reference](docs/reference.md). Each other format gets its own page, such as [Excel templates](docs/excel.md), plus one or two sentences under "Other formats" in the README.
- A new feature adds one row to the README's feature table, linking to its reference section. Don't add a README section for it.
- Write plain English. Use short sentences with one idea each. Don't use em-dashes. Don't use filler words such as "simply", "just" or "basically".
- Say what the library does and its limits. Don't tell the story of how it was built.
- Keep examples correct. Run the file examples in `examples/` after you change one.

## Code

- Don't run `cargo fmt`. Match the surrounding code's line length and layout.
- Each module opens with a `//!` comment. It names the tags it handles and the spec it follows.
- Doc comments describe what the item returns or does, in one or two sentences. Inline comments are rare. Use one only when the code would surprise a reader.
- Keep items `pub(crate)` unless they belong to the public API. Ask before widening visibility.
- Name XML element names as `const`s, such as `const ROW: &str = "w:tr";`, rather than repeating literals.
- Prefer early returns over nested `if`s. New parameters use an enum, not a `bool`, when a call site would otherwise read `f(true)`.
- Every bug fix gets a test that fails without the fix, in `tests/<feature>.rs`.
- Run the three CI checks before you push. Run the benchmarks in [BENCHMARKS.md](BENCHMARKS.md) when you change a hot path such as tag scanning or loop expansion.
