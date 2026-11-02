//! Tag names read as expressions. Expected results come from black-box runs of docxtemplater core
//! 3.71.1 with its expressions parser.

mod common;

use std::borrow::Cow;

use common::{cell, document, paragraph, paragraph_texts, read_rows, row, table, text};
use docx_fill::{FillError, FillHooks, FillOptions, PackageTemplate, Template};
use serde_json::{Value, json};

/// The filters the tests use.
struct Filters;

impl FillHooks for Filters {
    fn filter(&mut self, name: &str, input: &Value, arguments: &[Value]) -> Value {
        match (name, input) {
            ("upper", Value::String(text)) => json!(text.to_uppercase()),
            ("rev", Value::Array(items)) => Value::Array(items.iter().rev().cloned().collect()),
            ("add", Value::Number(number)) => json!(number.as_f64().unwrap() + arguments[0].as_f64().unwrap()),
            ("wrap", value) => {
                let print = |value: &Value| value.as_str().map_or_else(|| value.to_string(), str::to_string);
                json!(format!("{}{}{}", print(&arguments[0]), print(value), print(&arguments[1])))
            }
            ("missing", Value::Null) => json!("was null"),
            (_, value) => value.clone(),
        }
    }
}

fn options() -> FillOptions {
    FillOptions { expressions: true, filters: ["upper", "rev", "add", "wrap", "missing"].map(String::from).to_vec(), ..FillOptions::core() }
}

fn try_fill(template: &str, data: &Value) -> Result<String, FillError> {
    Ok(Template::parse(&document(&paragraph(&[template])), &options())?.render_with(data, &mut Filters))
}

fn fill_text(template: &str, data: &Value) -> String {
    text(&try_fill(template, data).unwrap_or_else(|error| panic!("{template}: {error}")))
}

fn data() -> Value {
    json!({
        "a": 1, "b": 2, "c": 3, "s": "abc", "e": "", "z": 0, "t": true, "f": false, "n": null, "l": [1, 2, 3], "m": [],
        "o": { "x": 1, "y": { "w": 5 }, "a b": 7 }, "u": "é🎉", "ns": "12",
        "objs": [{ "v": 1, "k": "x" }, { "v": 2 }], "nest": [[1, 2], [3]],
    })
}

/// Each `(template, expected text)`, with XML escapes as a filled part holds them.
fn assert_all(cases: &[(&str, &str)]) {
    let data = data();
    for (template, expected) in cases {
        assert_eq!(fill_text(&template.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;"), &data), *expected, "{template}");
    }
}

#[test]
fn reads_properties_and_indexes() {
    assert_all(&[
        ("{a}", "1"),
        ("{o.x}", "1"),
        ("{o.y.w}", "5"),
        ("{o.q}", ""),
        ("{o.q.r}", ""),
        ("{q.r.s}", ""),
        ("{o['x']}", "1"),
        ("{o[\"a b\"]}", "7"),
        ("{l[0]}", "1"),
        ("{l[3]}", ""),
        ("{l[-1]}", ""),
        ("{l[1.5]}", ""),
        ("{l['0']}", "1"),
        ("{l[a]}", "2"),
        ("{l[a + 1]}", "3"),
        ("{l[l.length - 1]}", "3"),
        ("{s[0]}", "a"),
        ("{s[5]}", ""),
        ("{objs[1].v}", "2"),
        ("{objs[1].k}", ""),
        ("{nest[0][1]}", "2"),
        ("{o .x}", "1"),
        ("{o. x}", "1"),
    ]);
}

#[test]
fn reads_length_as_javascript_counts_it() {
    assert_all(&[("{s.length}", "3"), ("{l.length}", "3"), ("{u.length}", "3"), ("{m.length}", "0"), ("{o.length}", ""), ("{a.length}", ""), ("{n.length}", ""), ("{q.length}", ""), ("{[1,2].length}", "2")]);
}

#[test]
fn reads_literals() {
    assert_all(&[
        ("{1}", "1"),
        ("{1.5}", "1.5"),
        ("{.5}", "0.5"),
        ("{5.}", "5"),
        ("{1e3}", "1000"),
        ("{1E3}", "1000"),
        ("{1e-2}", "0.01"),
        ("{0}", "0"),
        ("{007}", "7"),
        ("{\"x\"}", "x"),
        ("{'x'}", "x"),
        ("{\"\"}", ""),
        ("{'it\\'s'}", "it&apos;s"),
        ("{\"\\u00e9\"}", "é"),
        ("{\"a\\\"b\"}", "a&quot;b"),
        ("{'a\"b'}", "a&quot;b"),
        ("{true}", "true"),
        ("{false}", "false"),
        ("{null}", ""),
        ("{undefined}", ""),
        ("{[]}", ""),
        ("{[1,2]}", "1,2"),
        ("{[1, 'x', a]}", "1,x,1"),
        ("{[1,]}", "1"),
        ("{[1,2][1]}", "2"),
    ]);
}

#[test]
fn reads_the_curly_quotes_word_autocorrects_to() {
    assert_all(&[("{“smart”}", "smart"), ("{‘smart’}", "smart"), ("{“mixed\"}", "mixed"), ("{s == “abc”}", "true")]);
}

#[test]
fn reads_names_in_any_alphabet() {
    let data = json!({ "prénom": "A", "日本": "B", "$a": "C", "_a": "D", "a1": "E", "a$": "F" });
    for (template, expected) in [("{prénom}", "A"), ("{日本}", "B"), ("{$a}", "C"), ("{_a}", "D"), ("{a1}", "E"), ("{a$}", "F")] {
        assert_eq!(fill_text(template, &data), expected);
    }
}

#[test]
fn adds_as_javascript_does_with_missing_values_passed_over() {
    assert_all(&[
        ("{1 + 2}", "3"),
        ("{a + b}", "3"),
        ("{a + q}", "1"),
        ("{q + a}", "1"),
        ("{q + q2}", ""),
        ("{s + a}", "abc1"),
        ("{a + s}", "1abc"),
        ("{s + q}", "abc"),
        ("{a + n}", "1"),
        ("{n + a}", "1"),
        ("{n + n}", ""),
        ("{s + n}", "abc"),
        ("{t + 1}", "2"),
        ("{t + t}", "2"),
        ("{t + s}", "trueabc"),
        ("{l + 1}", "1,2,31"),
        ("{l + s}", "1,2,3abc"),
        ("{o + 1}", "[object Object]1"),
        ("{m + m}", ""),
        ("{'3' + 2}", "32"),
        ("{0.1 + 0.2}", "0.30000000000000004"),
        ("{1e21 + 1}", "1e+21"),
    ]);
}

#[test]
fn does_other_arithmetic_as_javascript_does_and_a_missing_value_gives_nothing() {
    assert_all(&[
        ("{a - b}", "-1"),
        ("{a - q}", ""),
        ("{q - a}", ""),
        ("{s - 1}", "NaN"),
        ("{ns - 1}", "11"),
        ("{n - 1}", ""),
        ("{t - 1}", "0"),
        ("{l - 1}", "NaN"),
        ("{a * b}", "2"),
        ("{a * q}", ""),
        ("{ns * 2}", "24"),
        ("{s * 2}", "NaN"),
        ("{a / b}", "0.5"),
        ("{a / z}", "Infinity"),
        ("{z / z}", "NaN"),
        ("{a / q}", ""),
        ("{7 % 3}", "1"),
        ("{7 % 0}", "NaN"),
        ("{7.5 % 2}", "1.5"),
        ("{1 / 3}", "0.3333333333333333"),
        ("{1e-7 * 1}", "1e-7"),
    ]);
}

#[test]
fn keeps_operator_precedence() {
    assert_all(&[("{2 * 3 + 4}", "10"), ("{2 + 3 * 4}", "14"), ("{(2 + 3) * 4}", "20"), ("{2 - 3 - 4}", "-5"), ("{2 / 4 / 2}", "0.25"), ("{1 < 2 == true}", "true"), ("{a || b && z}", "1"), ("{(a || b) && z}", "0"), ("{!a && b}", "false"), ("{!z && b}", "2"), ("{((a))}", "1")]);
}

#[test]
fn applies_unary_operators() {
    assert_all(&[
        ("{-a}", "-1"),
        ("{--a}", "1"),
        ("{- -a}", "1"),
        ("{+a}", "1"),
        ("{+s}", "NaN"),
        ("{+ns}", "12"),
        ("{-s}", "NaN"),
        ("{-n}", "0"),
        ("{-q}", "0"),
        ("{+q}", "0"),
        ("{-t}", "-1"),
        ("{!a}", "false"),
        ("{!z}", "true"),
        ("{!!a}", "true"),
        ("{!e}", "true"),
        ("{!m}", "false"),
        ("{!o}", "false"),
        ("{!n}", "true"),
        ("{!q}", "true"),
    ]);
}

#[test]
fn compares_as_javascript_does() {
    assert_all(&[
        ("{a == 1}", "true"),
        ("{a == '1'}", "true"),
        ("{a === '1'}", "false"),
        ("{a != '1'}", "false"),
        ("{a !== '1'}", "true"),
        ("{n == q}", "true"),
        ("{n === q}", "false"),
        ("{n == 0}", "false"),
        ("{z == f}", "true"),
        ("{z == e}", "true"),
        ("{e == f}", "true"),
        ("{t == '1'}", "true"),
        ("{t === 1}", "false"),
        ("{s === 'abc'}", "true"),
        ("{l == 1}", "false"),
        ("{l == '1,2,3'}", "true"),
        ("{l == l}", "true"),
        ("{o == o}", "true"),
        ("{[] == 0}", "true"),
        ("{[1] == 1}", "true"),
        ("{[1] == [1]}", "false"),
        ("{'' == 0}", "true"),
        ("{' ' == 0}", "true"),
        ("{'abc' == 0}", "false"),
        ("{null == false}", "false"),
        ("{undefined === undefined}", "true"),
        ("{0 / 0 == 0 / 0}", "false"),
        ("{1 / 0 == 1 / 0}", "true"),
        ("{s < 'b'}", "true"),
        ("{'10' < 9}", "false"),
        ("{'10' < '9'}", "true"),
        ("{'a' < 1}", "false"),
        ("{n < 1}", "true"),
        ("{q < 1}", "false"),
        ("{t > 0}", "true"),
    ]);
}

#[test]
fn logic_returns_its_operands() {
    assert_all(&[("{a && b}", "2"), ("{z && b}", "0"), ("{a || b}", "1"), ("{z || b}", "2"), ("{q || b}", "2"), ("{q && b}", ""), ("{n || s}", "abc"), ("{e || 'x'}", "x"), ("{z || e || 'x'}", "x")]);
}

#[test]
fn chooses_a_ternary_branch_with_lists_and_objects_truthy() {
    assert_all(&[("{t ? 1 : 2}", "1"), ("{q ? 1 : 2}", "2"), ("{a ? b ? 1 : 2 : 3}", "1"), ("{z ? 1 : f ? 2 : 3}", "3"), ("{m ? 1 : 2}", "1"), ("{o ? 1 : 2}", "1"), ("{e ? 1 : 2}", "2"), ("{a ? q : 2}", "")]);
}

#[test]
fn a_call_is_undefined_as_json_holds_no_functions() {
    assert_all(&[("{fn()}", ""), ("{o.fn()}", ""), ("{s.toUpperCase()}", ""), ("{a.b(1, c)}", "")]);
}

#[test]
fn applies_filters_after_everything_else_unless_in_brackets() {
    assert_all(&[
        ("{s | upper}", "ABC"),
        ("{s|upper}", "ABC"),
        ("{q | upper}", ""),
        ("{(s | upper) + 'x'}", "ABCx"),
        ("{t ? s : e | upper}", "ABC"),
        ("{s | wrap:'[':']'}", "[abc]"),
        ("{s | wrap:a:b}", "1abc2"),
        ("{s | wrap:a + 1:'x'}", "2abcx"),
        ("{s | wrap:t ? 1 : 2:'x'}", "1abcx"),
        ("{a | add:1 | add:2}", "4"),
        ("{#l | rev}{.}{/}", "321"),
        ("{#objs | rev}{v}{/}", "21"),
    ]);
}

#[test]
fn passes_a_missing_input_to_a_filter_as_null() {
    assert_all(&[("{q | missing}", "was null"), ("{n | missing}", "was null")]);
}

#[test]
fn reads_the_current_item_and_the_loop_index() {
    assert_all(&[
        ("{#l}{.}{/l}", "123"),
        ("{#l}{this}{/l}", "123"),
        ("{#l}{. + 1}{/l}", "234"),
        ("{#l}{this + 1}{/l}", "234"),
        ("{#objs}{this.v}{/objs}", "12"),
        ("{#l}{$index}{/l}", "012"),
        ("{#objs}{$index + 1}{/objs}", "12"),
        ("{#objs}{$index}{#l}{$index}{/l}{/objs}", "00121012"),
        ("{#objs}{#o}{$index}{/o}{/objs}", "01"),
        ("{#o}{$index}{/o}", ""),
        ("{#t}{$index}{/t}", ""),
        ("{$index}", ""),
        ("{#objs}{#$index == 0}first{/}{v}{/objs}", "first12"),
        ("{#objs}{l[$index]}{/objs}", "12"),
        ("{this}", "[object Object]"),
        ("{.}", "[object Object]"),
    ]);
}

#[test]
fn loops_over_any_expression() {
    assert_all(&[
        ("{#o.y}{w}{/o.y}", "5"),
        ("{#o.y}{w}{/}", "5"),
        ("{#l.length > 1}many{/l.length > 1}", "many"),
        ("{#m.length > 1}many{/}", ""),
        ("{^m.length}none{/m.length}", "none"),
        ("{^l.length}none{/l.length}", ""),
        ("{#a == 1}one{/}", "one"),
        ("{#s == 'abc'}eq{/}", "eq"),
        ("{#t && a}both{/}", "both"),
        ("{#[1,2]}{.}{/}", "12"),
        ("{#'s'}x{/}", "x"),
        ("{#0}x{/}", ""),
        ("{#1}x{/}", "x"),
        ("{^q.r}x{/}", "x"),
        ("{#t ? l : m}{.}{/}", "123"),
        ("{#f ? l : m}{.}{/}", ""),
        ("{#a ? [1,2] : []}{.}{/}", "12"),
        ("{#nest}{#.}{.}{/.}{/nest}", "123"),
        ("{#nest}{#this}{.}{/this}{/nest}", "123"),
    ]);
}

#[test]
fn looks_up_each_name_from_the_innermost_scope_out() {
    let cases: [(&str, Value, &str); 13] = [
        ("{#i}{x}{/i}", json!({ "x": "R", "i": [{ "x": null }] }), "R"),
        ("{#i}{x}{/i}", json!({ "x": "R", "i": [{}] }), "R"),
        ("{#i}{x}{/i}", json!({ "x": "R", "i": [{ "x": 0 }] }), "0"),
        ("{#i}{x}{/i}", json!({ "x": "R", "i": [{ "x": "" }] }), ""),
        ("{#i}{x}{/i}", json!({ "x": "R", "i": [{ "x": false }] }), "false"),
        ("{#i}{a + b}{/i}", json!({ "a": 1, "b": 2, "i": [{ "a": 10 }] }), "12"),
        ("{#i}{a + b}{/i}", json!({ "a": 1, "b": 2, "i": [{ "a": null }] }), "3"),
        ("{#i}{a + b}{/i}", json!({ "a": 1, "i": [{ "b": 2 }] }), "3"),
        ("{#i}{a > 0 ? 'y' : 'n'}{/i}", json!({ "a": 1, "i": [{ "a": 0 }] }), "n"),
        ("{#i}{a > 0 ? 'y' : 'n'}{/i}", json!({ "a": 1, "i": [{}] }), "y"),
        ("{#i}{#j}{a}{b}{c}{/j}{/i}", json!({ "a": 1, "i": [{ "b": 2, "j": [{ "c": 3 }] }] }), "123"),
        ("{#i}{v}{/i}", json!({ "v": "R", "i": [0, 1] }), "RR"),
        ("{#i}{v + 1}{/i}", json!({ "v": 5, "i": [0, 1] }), "66"),
    ];
    for (template, data, expected) in cases {
        assert_eq!(fill_text(template, &data), expected, "{template} with {data}");
    }
}

#[test]
fn tries_a_missing_result_again_from_the_parent_scope() {
    let cases: [(&str, Value, &str); 6] = [
        ("{#i}{p.q}{/i}", json!({ "p": { "q": "R" }, "i": [{ "p": {} }] }), "R"),
        ("{#i}{p.q}{/i}", json!({ "p": { "q": "R" }, "i": [{ "p": null }] }), "R"),
        ("{#i}{p.q}{/i}", json!({ "p": { "q": "R" }, "i": [{ "p": 1 }] }), "R"),
        ("{#i}{p.q}{/i}", json!({ "p": { "q": "R" }, "i": [{}] }), "R"),
        ("{#i}{a.b}{/i}", json!({ "i": [{ "a": { "b": 1 } }, { "a": {} }] }), "1"),
        ("{#i}{.}{/i}", json!({ "i": [0, false, "", null, 1] }), "0false[object Object]1"),
    ];
    for (template, data, expected) in cases {
        assert_eq!(fill_text(template, &data), expected, "{template} with {data}");
    }
}

#[test]
fn prints_lists_and_objects_as_javascript_does() {
    for (value, expected) in [(json!([1, 2]), "1,2"), (json!({ "b": 1 }), "[object Object]"), (json!([[1, 2], [3]]), "1,2,3"), (json!([{ "b": 1 }]), "[object Object]"), (json!(1.5), "1.5"), (json!(1e21), "1e+21")] {
        assert_eq!(fill_text("{a}", &json!({ "a": value })), expected);
    }
}

#[test]
fn an_empty_tag_prints_nothing() {
    assert_all(&[("{}", ""), ("{ }", ""), ("{#}x{/}", "")]);
}

#[test]
fn refuses_what_is_not_an_expression_when_parsing() {
    let data = data();
    let cases = [
        "{a b}", "{a +}", "{+}", "{*}", "{a *}", "{(a}", "{a)}", "{()}", "{[1}", "{a[}", "{a[]}", "{a.}", "{a..b}", "{a.1}", "{.a}", "{a ?}", "{a ? 1}", "{a ? 1 :}", "{a : 1}", "{?}", "{:}", "{a == }", "{a ===}", "{a &amp;}", "{a |}", "{a ||}", "{&amp;&amp; a}", "{!}", "{-}", "{a,b}", "{a@b}", "{\\}", "{'x}", "{\"x}", "{'}", "{name++}", "{a++}", "{new a}", "{typeof a}",
        "{1a}", "{0x10}", "{1e}", "{[,]}", "{s |}", "{| upper}", "{table:abc.value}", "{first name}", "{a = 1}", "{a;b}",
    ];
    for template in cases {
        let error = try_fill(template, &data).expect_err(template);
        assert!(error.message.contains("is not a valid expression"), "{template}: {error}");
    }
}

#[test]
fn names_the_tag_and_the_reason_in_its_error() {
    let error = try_fill("{a b}", &data()).unwrap_err();

    assert_eq!(error.message, "The tag \"a b\" is not a valid expression: unexpected \"b\"");
}

#[test]
fn refuses_a_filter_it_was_not_told_about() {
    let error = try_fill("{s | other}", &data()).unwrap_err();

    assert_eq!(error.message, "The tag \"s | other\" is not a valid expression: the filter \"other\" is not in FillOptions::filters");
}

#[test]
fn refuses_expressions_too_long_or_nested_to_read_safely() {
    let brackets = format!("{{{}a{}}}", "(".repeat(100_000), ")".repeat(100_000));
    let negations = format!("{{{}a}}", "!".repeat(100_000));
    let long = format!("{{{}1}}", "1 + ".repeat(100_000));
    for template in [brackets, negations, long] {
        assert!(try_fill(&template, &data()).is_err());
    }
}

#[test]
fn reads_values_for_every_tag_that_prints_one() {
    let options = FillOptions { expressions: true, ..FillOptions::default() };
    let fill = |template: &str, data: Value| Template::parse(&document(template), &options).unwrap().render(&data);

    assert_eq!(text(&fill(&paragraph(&["{@o.raw}"]), json!({ "o": { "raw": "<w:p><w:r><w:t>raw</w:t></w:r></w:p>" } }))), "raw");
    assert_eq!(paragraph_texts(&fill(&paragraph(&["{-w:p o.l}{.}{/o.l}"]), json!({ "o": { "l": [1, 2] } }))), ["1", "2"]);
    assert_eq!(paragraph_texts(&fill(&paragraph(&["{-w:p l.length > 0 ? l : m}{.}{/}"]), json!({ "l": [1, 2], "m": [] }))), ["1", "2"]);
    assert_eq!(text(&fill(&paragraph(&["{~~o.h}"]), json!({ "o": { "h": "<p>html</p>" } }))), "html");
    assert_eq!(text(&fill(&paragraph(&["a{~o.h}b"]), json!({ "o": { "h": "<b>x</b>" } }))), "axb");
    assert_eq!(text(&fill(&paragraph(&["{r@o.run}"]), json!({ "o": { "run": "<w:r><w:t>run</w:t></w:r>" } }))), "run");
    assert_eq!(paragraph_texts(&fill(&paragraph(&["{?o.q}"]), json!({ "o": { "q": ["x", "y"] } }))), ["x", "y"]);
    assert_eq!(read_rows(&fill(&paragraph(&["{:table o.t}"]), json!({ "o": { "t": { "data": [["a"]] } } }))), [["a"]]);
    assert!(fill(&table(&[row(&[cell(&["{:stylecell o.s}x"])])]), json!({ "o": { "s": { "cellBackground": "#ff0000" } } })).contains("FF0000"));
    assert!(fill(&paragraph(&["{:stylepar o.s}x"]), json!({ "o": { "s": { "bold": true } } })).contains("<w:b/>"));
}

#[test]
fn keeps_a_value_a_loop_item_was_worked_out_from() {
    let items: Vec<Value> = (0..20).map(|index| json!({ "v": index })).collect();

    let filled = fill_text("{#i | rev}{v}{/}", &json!({ "i": items }));

    assert_eq!(filled, (0..20).rev().map(|index| index.to_string()).collect::<String>());
}

#[test]
fn does_not_overflow_on_many_computed_values() {
    const ITEMS: usize = 300_000;
    let items: Vec<usize> = (0..ITEMS).collect();

    let filled = try_fill("{#l}{. + 1}{/l}", &json!({ "l": items })).unwrap();

    assert_eq!(filled.matches("<w:t").count(), 1);
    assert!(filled.contains(&ITEMS.to_string()));
}

#[test]
fn leaves_tag_names_whole_with_expressions_off() {
    let data = json!({ "a.b": "literal", "a": { "b": "deep" }, "table:abc.value": "kept", "first name": "spaced", "a + b": "summed" });
    let fill = |template: &str| text(&Template::parse(&document(&paragraph(&[template])), &FillOptions::core()).unwrap().render(&data));

    assert_eq!(fill("{a.b}"), "literal");
    assert_eq!(fill("{table:abc.value}"), "kept");
    assert_eq!(fill("{first name}"), "spaced");
    assert_eq!(fill("{a + b}"), "summed");
}

#[test]
fn reads_a_dash_at_the_start_of_a_name_as_part_of_it_without_an_element_and_a_name() {
    let data = json!({ "-a": "V", "--a": "W", "- a": "X", "-": "Y", "-w:p": "Z" });
    let fill = |template: &str| text(&Template::parse(&document(&paragraph(&[template])), &FillOptions::core()).unwrap().render(&data));

    assert_eq!(fill("{-a}"), "V");
    assert_eq!(fill("{--a}"), "W");
    assert_eq!(fill("{- a}"), "X");
    assert_eq!(fill("{-}"), "Y");
    assert_eq!(fill("{-w:p}"), "Z");
}

#[test]
fn still_reads_a_dash_tag_with_an_element_and_a_name() {
    let template = Template::parse(&document(&paragraph(&["{-w:p l}{.}{/l}"])), &FillOptions::core()).unwrap();

    assert_eq!(paragraph_texts(&template.render(&json!({ "l": [1, 2] }))), ["1", "2"]);
}

#[test]
fn reads_the_option_from_json() {
    let options = FillOptions::core().read_json(&json!({ "expressions": true, "filters": ["upper"] })).unwrap();
    let filled = Template::parse(&document(&paragraph(&["{a.b | upper}"])), &options).unwrap().render_with(&json!({ "a": { "b": "x" } }), &mut Filters);

    assert_eq!(text(&filled), "X");
}

#[test]
fn fills_a_package_with_a_filter_from_its_hooks() {
    let parts = vec![
        ("[Content_Types].xml".to_string(), br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"></Types>"#.to_vec()),
        ("word/document.xml".to_string(), document(&paragraph(&["{a.b | upper}"])).into_bytes()),
    ];
    let template = PackageTemplate::parse(parts, &options()).unwrap();

    let filled = template.render_with(&json!({ "a": { "b": "x" } }), &mut Filters);

    let (_, bytes): &(String, Cow<[u8]>) = filled.iter().find(|(name, _)| name == "word/document.xml").unwrap();
    assert_eq!(text(&String::from_utf8_lossy(bytes)), "X");
}

#[test]
fn reports_the_part_an_invalid_expression_is_in() {
    let parts = vec![("word/document.xml".to_string(), document(&paragraph(&["{a b}"])).into_bytes())];

    let error = PackageTemplate::parse(parts, &options()).unwrap_err();

    assert_eq!(error.part.as_deref(), Some("word/document.xml"));
}

#[test]
fn reads_the_loop_index_outside_loops_as_null() {
    assert_all(&[("{$index}", ""), ("{$index === null}", "true"), ("{0 <= $index}", "true"), ("{$index + 1}", "1"), ("{!$index}", "true")]);
}

#[test]
fn reads_names_from_the_innermost_scope_when_trying_an_outer_one() {
    let fill = |data: Value| fill_text("{#o}{#p}[{y ? q.r : 2}]{/p}{/o}", &data);

    assert_eq!(fill(json!({ "y": 0, "o": { "q": { "r": "M" }, "p": { "y": 1, "q": {} } } })), "[M]");
    assert_eq!(fill(json!({ "y": 1, "o": { "q": { "r": "M" }, "p": { "y": 0, "q": {} } } })), "[2]");
    assert_eq!(fill(json!({ "o": { "y": null, "q": { "r": "M" }, "p": { "y": 1, "q": {} } } })), "[M]");
    assert_eq!(fill_text("{#o}[{w ? p.q : 2}]{/o}", &json!({ "o": { "p": {}, "w": 1 }, "w": null, "p": { "q": "Q" } })), "[Q]");
}

#[test]
fn adds_two_missing_values_as_the_right_one() {
    assert_all(&[("{n + n === null}", "true"), ("{q + n === null}", "true"), ("{n + q === null}", "false"), ("{n + q === undefined}", "true"), ("{q + n >= 0}", "true"), ("{o.q + n === null}", "true")]);
}

#[test]
fn keeps_the_innermost_loop_index_when_trying_an_outer_scope() {
    let data = json!({ "l": [{ "m": [{}, {}] }, { "m": [{}, {}] }] });

    assert_eq!(fill_text("{#l}{#m}[{$index ? 'Y' : q.r}]{/m}{/l}", &data), "[][Y][][Y]");
}
