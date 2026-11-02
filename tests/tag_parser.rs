//! `FillHooks::tag_value` reads a tag's name in the caller's own syntax, as `parser` does. Results
//! for `a/b` paths match black-box runs of docxtemplater core 3.71.1 with the same parser.

mod common;

use common::{document, paragraph, paragraph_texts};
use docx_fill::{FillHooks, FillOptions, MissingValue, ModuleTag, Template};
use serde_json::{Value, json};

/// Reads `a/b` as a path, from the innermost scope out. Names without a `/` are left alone.
#[derive(Default)]
struct Paths {
    seen: Vec<(String, Vec<Value>)>,
    missing: Vec<String>,
    module_values: Vec<Option<Value>>,
}

impl FillHooks for Paths {
    fn tag_value(&mut self, name: &str, scopes: &[&Value]) -> Option<Value> {
        self.seen.push((name.to_string(), scopes.iter().map(|scope| (*scope).clone()).collect()));
        if !name.contains('/') {
            return None;
        }
        let found = scopes.iter().rev().find_map(|scope| name.trim().split('/').try_fold(*scope, |value, key| value.get(key)).filter(|value| !value.is_null()));
        Some(found.cloned().unwrap_or(Value::Null))
    }

    fn missing_value(&mut self, _part: &str, tag: MissingValue) -> Option<String> {
        self.missing.push(tag.name.to_string());
        None
    }

    fn module_tag(&mut self, _part: &str, tag: ModuleTag) -> String {
        self.module_values.push(tag.value.cloned());
        String::new()
    }
}

fn fill_with(body: &str, data: &Value, options: &FillOptions, hooks: &mut Paths) -> Vec<String> {
    paragraph_texts(&Template::parse(&document(body), options).unwrap().render_with(data, hooks))
}

fn fill(body: &str, data: &Value) -> Vec<String> {
    fill_with(body, data, &FillOptions { paragraph_loop: false, ..FillOptions::core() }, &mut Paths::default())
}

fn data() -> Value {
    json!({ "a": { "b": "AB" }, "l": [{ "x": 1 }, { "x": 2 }], "o": { "p": [{ "q": "Q1" }, { "q": "Q2" }], "t": true, "z": 0, "f": { "g": "G" } } })
}

#[test]
fn reads_value_tags() {
    assert_eq!(fill(&paragraph(&["{a/b}"]), &data()), ["AB"]);
    assert_eq!(fill(&paragraph(&["{ a/b }"]), &data()), ["AB"]);
    assert_eq!(fill(&paragraph(&["[{a/q}]"]), &data()), ["[]"]);
    assert_eq!(fill(&paragraph(&["{#l}{x}{a/b}{/l}"]), &data()), ["1AB2AB"]);
}

#[test]
fn reads_loops() {
    assert_eq!(fill(&paragraph(&["{#o/p}{q}{/o/p}"]), &data()), ["Q1Q2"]);
    assert_eq!(fill(&paragraph(&["{#o/p}{q}{/}"]), &data()), ["Q1Q2"]);
    assert_eq!(fill(&paragraph(&["{#o/f}{g}{/o/f}"]), &data()), ["G"]);
    assert_eq!(fill(&paragraph(&["{#o/t}yes{/o/t}"]), &data()), ["yes"]);
    assert_eq!(fill(&paragraph(&["{#o/z}yes{/o/z}{^o/z}no{/o/z}"]), &data()), ["no"]);
    assert_eq!(fill(&paragraph(&["{^o/q}none{/o/q}"]), &data()), ["none"]);
    assert_eq!(fill(&paragraph(&["{^o/p}none{/o/p}"]), &data()), [""]);
    assert_eq!(fill(&paragraph(&["{-w:p o/p}{q}{/o/p}"]), &data()), ["Q1", "Q2"]);
}

#[test]
fn gives_the_scopes_innermost_last() {
    let mut hooks = Paths::default();
    let data = json!({ "i": [{ "j": [5] }] });
    fill_with(&paragraph(&["{#i}{#j}{.}{/j}{/i}"]), &data, &FillOptions::core(), &mut hooks);

    let names: Vec<&str> = hooks.seen.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["i", "j", "."]);
    let (item, inner) = (&data["i"][0], &data["i"][0]["j"][0]);
    assert_eq!(hooks.seen[2].1, [data.clone(), item.clone(), inner.clone()]);
}

#[test]
fn leaves_a_tag_to_the_usual_reading_for_none() {
    assert_eq!(fill(&paragraph(&["{#l}{x}{/l}"]), &data()), ["12"]);

    let options = FillOptions { expressions: true, ..FillOptions::core() };
    let data = json!({ "n": 2, "p/q": 7 });
    assert_eq!(fill_with(&paragraph(&["{n + 1}"]), &data, &options, &mut Paths::default()), ["3"]);
}

#[test]
fn takes_over_from_a_key_of_the_same_name() {
    assert_eq!(fill(&paragraph(&["{a/b}"]), &json!({ "a/b": "whole", "a": { "b": "path" } })), ["path"]);
}

#[test]
fn counts_a_null_as_missing() {
    let mut hooks = Paths::default();
    let filled = fill_with(&paragraph(&["[{a/q}]"]), &json!({ "a/q": "whole", "a": {} }), &FillOptions::core(), &mut hooks);

    assert_eq!(filled, ["[]"]);
    assert_eq!(hooks.missing, ["a/q"]);
}

#[test]
fn reads_raw_module_and_extension_tags() {
    let data = json!({ "r": { "xml": "<w:p><w:r><w:t>raw</w:t></w:r></w:p>", "run": "<w:r><w:t>run</w:t></w:r>", "html": "<b>bold</b>" } });
    assert_eq!(fill(&paragraph(&["{@r/xml}"]), &data), ["raw"]);

    let options = FillOptions { module_prefixes: vec!["$".into()], ..FillOptions::default() };
    let mut hooks = Paths::default();
    assert_eq!(fill_with(&paragraph(&["{r@r/run}"]), &data, &options, &mut hooks), ["run"]);
    assert_eq!(fill_with(&paragraph(&["{~r/html}"]), &data, &options, &mut hooks), ["bold"]);
    fill_with(&paragraph(&["{$r/xml}{$r/none}"]), &data, &options, &mut hooks);
    assert_eq!(hooks.module_values, [Some(data["r"]["xml"].clone()), None]);
}
