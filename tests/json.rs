use docx_fill::{Extensions, FillOptions, FilledDocument, HashAlgorithm, IncludeMode, IncludeStyles, ProtectionKind, WatermarkLayout, read_include_options};
use serde_json::json;

#[test]
fn reads_every_option() {
    let value = json!({
        "paragraphLoop": false,
        "linebreaks": false,
        "delimiters": { "start": "[[", "end": "]]" },
        "extensions": { "tableMerge": false, "sections": true },
        "dropEmptyLastPage": false,
        "modulePrefixes": ["$"],
        "dropStyleParagraphs": true,
        "comments": { "author": "Jane", "initials": "J", "date": "2026-11-02T09:00:00Z" },
        "footnotes": { "raw": true, "refProperties": "<w:b/>", "refRun": "<w:r/>", "paragraphProperties": "<w:keepNext/>", "runProperties": "<w:i/>" },
        "document": {
            "readOnly": true,
            "language": "en-GB",
            "protection": { "password": "secret1", "protect": "forms", "algorithm": "SHA-256", "spinCount": 10, "salt": "AAAA" },
            "watermark": { "text": "DRAFT", "diagonal": false },
            "margins": { "top": 1, "right": 0.5, "bottom": 1, "left": 0.5 }
        }
    });
    let base = FillOptions { extensions: Extensions { sections: false, ..Extensions::ALL }, ..FillOptions::default() };
    let options = base.read_json(&value).unwrap();

    assert!(!options.paragraph_loop && !options.linebreaks && !options.drop_empty_last_page && options.drop_style_paragraphs);
    assert_eq!((options.delimiters.start.as_str(), options.delimiters.end.as_str()), ("[[", "]]"));
    assert_eq!(options.extensions, Extensions { table_merge: false, ..Extensions::ALL });
    assert_eq!(options.module_prefixes, ["$"]);
    assert_eq!((options.comments.author.as_str(), options.comments.initials.as_deref()), ("Jane", Some("J")));
    assert_eq!(options.footnotes.content, docx_fill::FootnoteContent::Raw);
    assert_eq!(options.footnotes.ref_properties.as_deref(), Some("<w:b/>"));
    assert_eq!(options.footnotes.ref_run.as_deref(), Some("<w:r/>"));
    assert_eq!(options.footnotes.paragraph_properties.as_deref(), Some("<w:keepNext/>"));
    assert_eq!(options.footnotes.run_properties.as_deref(), Some("<w:i/>"));
    let document = &options.document;
    assert!(document.read_only);
    assert_eq!(document.language.as_deref(), Some("en-GB"));
    let protection = document.protection.as_ref().unwrap();
    assert_eq!((protection.kind, protection.algorithm, protection.spin_count, protection.salt.as_deref()), (ProtectionKind::Forms, HashAlgorithm::Sha256, 10, Some("AAAA")));
    assert_eq!(document.watermark.as_ref().unwrap().layout, WatermarkLayout::Horizontal);
    assert_eq!(document.margins.unwrap().right, 0.5);
}

#[test]
fn reads_extension_shorthands_and_refuses_unknown_keys() {
    assert_eq!(FillOptions::default().read_json(&json!({ "extensions": false })).unwrap().extensions, Extensions::NONE);
    assert_eq!(FillOptions::core().read_json(&json!({ "extensions": "all" })).unwrap().extensions, Extensions::ALL);
    for value in [json!({ "paragraphloop": true }), json!({ "extensions": { "tablemerge": true } }), json!({ "linebreaks": "yes" }), json!({ "document": { "protection": { "password": "secret1", "protect": "all" } } })] {
        assert!(FillOptions::default().read_json(&value).is_err(), "{value}");
    }
}

#[test]
fn reads_subtemplate_options() {
    assert_eq!(read_include_options(&json!({})).unwrap(), (IncludeMode::Templated, IncludeStyles::UseMain));
    assert_eq!(read_include_options(&json!({ "mode": "asIs", "styles": "keepOwn" })).unwrap(), (IncludeMode::AsIs, IncludeStyles::KeepOwn));
    assert!(read_include_options(&json!({ "mode": "other" })).is_err());
}

#[test]
fn runs_text_edits() {
    let document = r#"<w:document xmlns:w="w"><w:body><w:p><w:r><w:t>Lorem ipsum Lorem</w:t></w:r></w:p></w:body></w:document>"#;
    let mut document = FilledDocument::new([("word/document.xml".to_string(), document.as_bytes().to_vec())]);
    let edits = json!([
        { "op": "search", "find": "Lorem" },
        { "op": "addComment", "find": "ipsum", "comment": "Latin", "author": "Jane" },
        { "op": "replace", "find": "Lorem", "index": 1, "text": "Dolor" },
        { "op": "replaceAll", "find": "Lorem", "text": "Sit" },
        { "op": "comments" }
    ]);
    let results = document.run_json(&edits).unwrap();

    assert_eq!(results[0][1], json!({ "start": { "paragraph": 0, "offset": 12 }, "end": { "paragraph": 0, "offset": 17 }, "text": "Lorem" }));
    assert_eq!(results[1], json!(0));
    assert_eq!((results[2].clone(), results[3].clone()), (json!(null), json!(1)));
    assert_eq!((results[4][0]["comment"].as_str(), results[4][0]["author"].as_str(), results[4][0]["range"]["text"].as_str()), (Some("Latin"), Some("Jane"), Some("ipsum")));
    assert_eq!(document.search("Sit ipsum Dolor").len(), 1);

    assert!(document.run_json(&json!([{ "op": "replace", "find": "absent" }])).is_err());
    assert!(document.run_json(&json!([{ "op": "other" }])).is_err());
}

#[test]
fn reads_the_change_delimiter_prefix() {
    let options = docx_fill::FillOptions::default().read_json(&serde_json::json!({ "changeDelimiterPrefix": null })).unwrap();
    assert_eq!(options.change_delimiter_prefix, None);
    let options = docx_fill::FillOptions::default().read_json(&serde_json::json!({ "changeDelimiterPrefix": "$" })).unwrap();
    assert_eq!(options.change_delimiter_prefix.as_deref(), Some("$"));
}
