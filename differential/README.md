# Differential tests

Fills random templates with docx-fill and with the MIT-licensed docxtemplater core, then compares the results. Only core is run, as a black box. The paid modules are never run.

```sh
cd differential
pnpm install
node run.mjs --seed 1 --count 1000
node run.mjs --seed 1 --count 1000 --expressions
```

`run.mjs` builds and runs `examples/differential_fill.rs` with cargo, so run it from a checkout with Rust. `DOCX_FILL_RUNNER` replaces the cargo command. It is given the cases file's path.

| File | Does |
|---|---|
| `generate.mjs` | Builds templates and data from a seed: values, inline and paragraph loops, row loops, raw tags, tags split across runs with tabs, breaks, proofErr and bookmarks between them, stray delimiters and three delimiter pairs. With `--expressions`, tag names are expressions. |
| `core.mjs` | Fills a case with core. A missing value prints nothing, as in docx-fill. |
| `run.mjs` | Compares paragraphs, tables, rows, cells, text, breaks and tabs. It exits 1 on any difference that is not known. |

A known difference is one we have chosen to keep. Each is listed in `Known` in `run.mjs` with the reason. The generator also avoids a few core quirks, each noted where it does so:

- A loop item that is `false`, `0` or `""` answers every lookup with itself.
- In an expression, `.` next to another symbol, as in `!.` or `.==1`, fails to compile.

When a new difference turns up, add its template as a test in `tests/` with the fix.
