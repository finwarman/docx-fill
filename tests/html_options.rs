mod common;

use common::{document, paragraph_texts, text};
use docx_fill::{FillErrorId, FillHooks, FillOptions, HtmlBlockMode, HtmlCustomization, HtmlElement, HtmlFragment, HtmlTagKind, IncludeMode, IncludeStyles, PackageTemplate, Subtemplate, Template};
use serde_json::json;

fn template(body: &str, options: &FillOptions) -> Template {
    Template::parse(&document(body), options).unwrap()
}

#[test]
fn rejects_unknown_tags() {
    let template = template("<w:p><w:r><w:t>{~~html}</w:t></w:r></w:p>", &FillOptions::default());
    let data = json!({"html":"<p><unknown-tag>A</unknown-tag></p>"});
    let error = template.try_render(&data).unwrap_err();

    assert_eq!(error.id, FillErrorId::HtmlUnknownTag);
    assert_eq!(error.tag.as_deref(), Some("html"));
    assert_eq!(error.paragraph, Some(0));
    assert_eq!(text(&template.render(&data)), "A");
}

#[test]
fn keeps_unknown_children() {
    let options = FillOptions::default().read_json(&json!({"html":{"ignoreUnknownTags":true}})).unwrap();
    let template = template("<w:p><w:r><w:t>{~~html}</w:t></w:r></w:p>", &options);
    let xml = template.try_render(&json!({"html":"<unknown-tag><b>A</b></unknown-tag>"})).unwrap();

    assert_eq!(text(&xml), "A");
    assert!(xml.contains("<w:b/>"));
}

#[test]
fn recovers_valid_css() {
    let options = FillOptions::default().read_json(&json!({"html":{"ignoreCssErrors":true},"htmlStyleSheet":"p {color:red;broken}"})).unwrap();
    let xml = template("<w:p><w:r><w:t>{~~html}</w:t></w:r></w:p>", &options)
        .try_render(&json!({"html":"<p style='font-weight:bold;bad'>A</p>"})).unwrap();

    assert!(xml.contains("<w:b/>") && xml.contains("w:color w:val=\"FF0000\""));
    let error = template("<w:p><w:r><w:t>{~~html}</w:t></w:r></w:p>", &FillOptions::default())
        .try_render(&json!({"html":"<p style='color:red;broken'>A</p>"})).unwrap_err();
    assert_eq!(error.id, FillErrorId::HtmlCssInvalid);
}

#[test]
fn controls_invalid_blocks() {
    let body = "<w:p><w:r><w:t>before {~html} after</w:t></w:r></w:p>";
    let data = json!({"html":"<p>A</p><p>B</p>"});
    let mut options = FillOptions::default();
    assert_eq!(text(&template(body, &options).try_render(&data).unwrap()), "before AB after");
    options.html.on_invalid_block = HtmlBlockMode::Block;
    assert_eq!(paragraph_texts(&template(body, &options).try_render(&data).unwrap()), ["A", "B"]);
    options.html.on_invalid_block = HtmlBlockMode::Error;
    assert_eq!(template(body, &options).try_render(&data).unwrap_err().id, FillErrorId::HtmlBlockInvalid);
}

struct CustomHooks { calls: usize }

impl FillHooks for CustomHooks {
    fn html_custom_tag(&mut self, element: &HtmlElement<'_>) -> Option<HtmlFragment> {
        self.calls += 1;
        match element.name() {
            "check-box" => {
                assert_eq!(element.text(), "A&B");
                assert!(element.matches("section > check-box"));
                Some(HtmlFragment::inline("<w:r><w:rPr><w:b/></w:rPr><w:t>checked</w:t></w:r>"))
            }
            "block-box" => Some(HtmlFragment::block("<w:p><w:r><w:t>block</w:t></w:r></w:p>")),
            _ => None,
        }
    }
    fn html_style_transform(&mut self, element: &HtmlElement<'_>) -> Option<String> {
        (element.name() == "h1").then(|| "Heading1".into())
    }
    fn html_customize(&mut self, element: &HtmlElement<'_>) -> HtmlCustomization {
        if element.matches("h1 + p.note") {
            HtmlCustomization::default().paragraph_style("Note").css("color:red")
        } else { HtmlCustomization::default() }
    }
}

#[test]
fn customizes_elements() {
    let mut options = FillOptions::default();
    options.html.custom_tags.insert("check-box".into(), HtmlTagKind::Inline);
    options.html.custom_tags.insert("block-box".into(), HtmlTagKind::Block);
    let mut hooks = CustomHooks { calls: 0 };
    let data = json!({"html":"<h1>Title</h1><p class='note'>Note</p><section><check-box>A&amp;B</check-box><block-box/></section>"});
    let xml = template("<w:p><w:r><w:t>{~~html}</w:t></w:r></w:p>", &options).try_render_with(&data, &mut hooks).unwrap();

    assert_eq!(hooks.calls, 2);
    assert!(xml.contains("<w:pStyle w:val=\"Heading1\"/>") && xml.contains("<w:pStyle w:val=\"Note\"/>"));
    assert!(xml.contains("w:color w:val=\"FF0000\""));
    assert!(!xml.contains("w:sz w:val=\"32\""));
    assert_eq!(text(&xml), "TitleNotecheckedblock");
}

#[test]
fn collects_include_errors() {
    let options = FillOptions::default();
    let mut package = PackageTemplate::parse([("word/document.xml".into(), document("<w:p><w:r><w:t>{:include child}</w:t></w:r></w:p>").into_bytes())], &options).unwrap();
    let child = Subtemplate::parse([("word/document.xml".into(), document("<w:p><w:r><w:t>{~~html}</w:t></w:r></w:p>").into_bytes())], &options, IncludeMode::Templated, IncludeStyles::UseMain).unwrap();
    package.add_include("child", child);
    let error = package.try_render(&json!({"child":"child","html":"<unknown>A</unknown><p style='broken'>B</p>"})).unwrap_err();

    assert_eq!(error.id, FillErrorId::MultiError);
    assert_eq!(error.diagnostics().len(), 2);
    assert!(error.diagnostics().iter().all(|error| error.part.as_deref() == Some("word/document.xml")));
}

#[test]
fn rejects_unbalanced_custom_xml() {
    struct Broken;
    impl FillHooks for Broken {
        fn html_custom_tag(&mut self, _: &HtmlElement<'_>) -> Option<HtmlFragment> {
            Some(HtmlFragment::inline("<w:r>"))
        }
    }
    let options = FillOptions::default().read_json(&json!({"html":{"customTags":{"broken":"inline"}}})).unwrap();
    let error = template("<w:p><w:r><w:t>{~html}</w:t></w:r></w:p>", &options)
        .try_render_with(&json!({"html":"<broken/>"}), &mut Broken).unwrap_err();

    assert_eq!(error.id, FillErrorId::MalformedXml);
}

#[test]
fn keeps_custom_tag_children() {
    let options = FillOptions::default().read_json(&json!({"html":{"customTags":{"box":"block"}}})).unwrap();
    let xml = template("<w:p><w:r><w:t>{~~html}</w:t></w:r></w:p>", &options)
        .try_render(&json!({"html":"<box>A</box><box>B</box>"})).unwrap();

    assert_eq!(paragraph_texts(&xml), ["A", "B"]);
}
