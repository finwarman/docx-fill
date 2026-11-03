# Contributing

Put one change in each pull request. Without write access, open it from a fork.

## Checks

CI runs these on every pull request:

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --target wasm32-unknown-unknown
```

The wasm build needs `rustup target add wasm32-unknown-unknown` once.

CI does not check formatting. The code doesn't follow rustfmt's defaults, so don't run `cargo fmt`: it rewrites hundreds of unrelated lines. Match the surrounding code.

## Sources

Build behaviour from the WordprocessingML spec (ECMA-376), docxtemplater's public tag syntax and documentation, and black-box runs of the MIT-licensed docxtemplater core. Extension tags follow the paid modules' public documentation.

Don't copy code from docxtemplater, and don't tune anything against the paid modules' output. Name your source in the pull request.

Password-hash test vectors come from Apache POI. For characters above 0x7F, this crate follows the spec's unsigned bytes; POI sign-extends them.

## Adding a feature

- Keep `serde_json` and `regex` as the default dependencies. Put optional dependencies behind Cargo features and keep wasm builds working.
- A new tag kind gets a flag in `Extensions` and a key in `read_extensions` in `src/json.rs`.
- A new option goes in `FillOptions::read_json`, which refuses unknown keys, and in `reads_every_option` in `tests/json.rs`.
- Tests go in `tests/<feature>.rs`. `tests/common/mod.rs` has helpers for building and reading document XML.
- Document tags and options in [the reference](docs/reference.md). Keep setup and common usage in the README.

Contributions use the [MIT licence](LICENSE).
