//! Tag names read as expressions, as docxtemplater's expression parser documents them: dot and
//! bracket access, literals, arithmetic, comparison, logic, a ternary, filters, `$index` and `this`.
//! Where the docs are silent, results follow black-box runs of docxtemplater core 3.71.1 with its
//! expressions parser, which keeps JavaScript's rules for mixed types.

use std::borrow::Cow;
use std::cell::OnceCell;

use serde_json::{Number, Value, json};

use crate::render::{FillHooks, print_value};

// Nested tags and long chains recurse when read and when evaluated.
const MAX_NESTING: usize = 64;
const MAX_TOKENS: usize = 1024;
// Above this, a float cannot be told from its neighbours, so it prints as a float.
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// A parsed expression.
#[derive(Debug)]
pub(crate) enum Expression {
    Literal(Value),
    Undefined,
    This,
    LoopIndex,
    Identifier(String),
    Property(Box<Expression>, String),
    Index(Box<Expression>, Box<Expression>),
    /// JSON data holds no functions, so a call is always undefined.
    Call,
    Array(Vec<Expression>),
    Unary(UnaryOperator, Box<Expression>),
    Binary(BinaryOperator, Box<Expression>, Box<Expression>),
    And(Box<Expression>, Box<Expression>),
    Or(Box<Expression>, Box<Expression>),
    Conditional(Box<Expression>, Box<Expression>, Box<Expression>),
    Filter { input: Box<Expression>, name: String, arguments: Vec<Expression> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnaryOperator {
    Negate,
    Plus,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BinaryOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Equal,
    NotEqual,
    StrictEqual,
    StrictNotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    Text(String),
    Name(String),
    Symbol(&'static str),
}

// Longest first, so `===` is not read as `==` and `=`.
const SYMBOLS: [&str; 25] = ["===", "!==", "==", "!=", "<=", ">=", "&&", "||", "+", "-", "*", "/", "%", "<", ">", "!", "?", ":", ".", ",", "(", ")", "[", "]", "|"];

fn read_tokens(text: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let mut rest = text;
    while let Some(first) = rest.chars().next() {
        if first.is_whitespace() {
            rest = &rest[first.len_utf8()..];
        } else if first.is_ascii_digit() || (first == '.' && rest[1..].starts_with(|c: char| c.is_ascii_digit())) {
            let (number, after) = read_number(rest)?;
            tokens.push(Token::Number(number));
            rest = after;
        } else if let Some(close) = closing_quote(first) {
            let (value, after) = read_string(&rest[first.len_utf8()..], close)?;
            tokens.push(Token::Text(value));
            rest = after;
        } else if is_name_start(first) {
            let end = rest.find(|c: char| !is_name_part(c)).unwrap_or(rest.len());
            tokens.push(Token::Name(rest[..end].to_string()));
            rest = &rest[end..];
        } else {
            let symbol = SYMBOLS.iter().find(|symbol| rest.starts_with(**symbol)).ok_or_else(|| format!("unexpected \"{first}\""))?;
            tokens.push(Token::Symbol(symbol));
            rest = &rest[symbol.len()..];
        }
        if tokens.len() > MAX_TOKENS {
            return Err("too long".to_string());
        }
    }
    Ok(tokens)
}

fn is_name_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || c == '$'
}

fn is_name_part(c: char) -> bool {
    is_name_start(c) || c.is_numeric()
}

/// The quotes that close a string opened by `open`. Word's autocorrect turns straight quotes into
/// curly ones, so each kind closes with any of its family.
fn closing_quote(open: char) -> Option<&'static [char]> {
    match open {
        '"' | '\u{201c}' | '\u{201d}' => Some(&['"', '\u{201c}', '\u{201d}']),
        '\'' | '\u{2018}' | '\u{2019}' => Some(&['\'', '\u{2018}', '\u{2019}']),
        _ => None,
    }
}

fn read_number(text: &str) -> Result<(f64, &str), String> {
    let digits = |from: usize| text[from..].find(|c: char| !c.is_ascii_digit()).map_or(text.len(), |length| from + length);
    let mut end = digits(0);
    if text[end..].starts_with('.') {
        end = digits(end + 1);
    }
    if text[end..].starts_with(['e', 'E']) {
        let mut exponent = end + 1;
        if text[exponent..].starts_with(['+', '-']) {
            exponent += 1;
        }
        let exponent_end = digits(exponent);
        if exponent_end == exponent {
            return Err(format!("the number \"{}\" has no exponent", &text[..exponent]));
        }
        end = exponent_end;
    }
    if text[end..].starts_with(is_name_part) {
        return Err(format!("unexpected \"{}\" after a number", &text[..end]));
    }
    let number = text[..end].parse().map_err(|_| format!("the number \"{}\" is not valid", &text[..end]))?;
    Ok((number, &text[end..]))
}

fn read_string<'t>(text: &'t str, close: &[char]) -> Result<(String, &'t str), String> {
    let mut value = String::new();
    let mut characters = text.char_indices();
    while let Some((at, c)) = characters.next() {
        if close.contains(&c) {
            return Ok((value, &text[at + c.len_utf8()..]));
        }
        if c != '\\' {
            value.push(c);
            continue;
        }
        match characters.next() {
            Some((_, 'n')) => value.push('\n'),
            Some((_, 't')) => value.push('\t'),
            Some((_, 'r')) => value.push('\r'),
            Some((_, 'f')) => value.push('\u{c}'),
            Some((_, 'v')) => value.push('\u{b}'),
            Some((_, 'u')) => {
                let code: String = characters.by_ref().take(4).map(|(_, digit)| digit).collect();
                let decoded = u32::from_str_radix(&code, 16).ok().and_then(char::from_u32).ok_or_else(|| format!("the escape \"\\u{code}\" is not valid"))?;
                value.push(decoded);
            }
            Some((_, escaped)) => value.push(escaped),
            None => break,
        }
    }
    Err("a string is not closed".to_string())
}

/// Reads tokens with the precedence the docs' operator table gives, lowest first.
struct Parser<'t> {
    tokens: Vec<Token>,
    at: usize,
    nesting: usize,
    filters: &'t [String],
}

impl Expression {
    /// Reads `text`. A filter must be named in `filters`.
    pub(crate) fn parse(text: &str, filters: &[String]) -> Result<Expression, String> {
        let mut parser = Parser { tokens: read_tokens(text)?, at: 0, nesting: 0, filters };
        if parser.tokens.is_empty() {
            return Ok(Expression::Undefined);
        }
        let expression = parser.filter_chain()?;
        match parser.tokens.get(parser.at) {
            None => Ok(expression),
            Some(token) => Err(format!("unexpected {}", describe(token))),
        }
    }
}

fn describe(token: &Token) -> String {
    match token {
        Token::Number(number) => format!("\"{number}\""),
        Token::Text(text) | Token::Name(text) => format!("\"{text}\""),
        Token::Symbol(symbol) => format!("\"{symbol}\""),
    }
}

impl Parser<'_> {
    fn eat(&mut self, symbol: &'static str) -> bool {
        let is_next = self.tokens.get(self.at) == Some(&Token::Symbol(symbol));
        self.at += usize::from(is_next);
        is_next
    }

    fn expect(&mut self, symbol: &'static str) -> Result<(), String> {
        if self.eat(symbol) {
            return Ok(());
        }
        match self.tokens.get(self.at) {
            Some(token) => Err(format!("expected \"{symbol}\", found {}", describe(token))),
            None => Err(format!("expected \"{symbol}\" at the end")),
        }
    }

    fn filter_chain(&mut self) -> Result<Expression, String> {
        let mut expression = self.conditional()?;
        while self.eat("|") {
            let Some(Token::Name(name)) = self.tokens.get(self.at).cloned() else { return Err("expected a filter's name".to_string()) };
            self.at += 1;
            if !self.filters.contains(&name) {
                return Err(format!("the filter \"{name}\" is not in FillOptions::filters"));
            }
            let mut arguments = Vec::new();
            while self.eat(":") {
                arguments.push(self.conditional()?);
            }
            expression = Expression::Filter { input: Box::new(expression), name, arguments };
        }
        Ok(expression)
    }

    fn conditional(&mut self) -> Result<Expression, String> {
        let condition = self.or()?;
        if !self.eat("?") {
            return Ok(condition);
        }
        self.nest()?;
        let shown = self.conditional()?;
        self.expect(":")?;
        let otherwise = self.conditional()?;
        self.nesting -= 1;
        Ok(Expression::Conditional(Box::new(condition), Box::new(shown), Box::new(otherwise)))
    }

    fn or(&mut self) -> Result<Expression, String> {
        let mut left = self.and()?;
        while self.eat("||") {
            left = Expression::Or(Box::new(left), Box::new(self.and()?));
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Expression, String> {
        let mut left = self.equality()?;
        while self.eat("&&") {
            left = Expression::And(Box::new(left), Box::new(self.equality()?));
        }
        Ok(left)
    }

    fn binary(&mut self, operators: &[(&'static str, BinaryOperator)], next: fn(&mut Self) -> Result<Expression, String>) -> Result<Expression, String> {
        let mut left = next(self)?;
        while let Some((_, operator)) = operators.iter().find(|(symbol, _)| self.eat(symbol)) {
            left = Expression::Binary(*operator, Box::new(left), Box::new(next(self)?));
        }
        Ok(left)
    }

    fn equality(&mut self) -> Result<Expression, String> {
        self.binary(&[("===", BinaryOperator::StrictEqual), ("!==", BinaryOperator::StrictNotEqual), ("==", BinaryOperator::Equal), ("!=", BinaryOperator::NotEqual)], Self::relational)
    }

    fn relational(&mut self) -> Result<Expression, String> {
        self.binary(&[("<=", BinaryOperator::LessOrEqual), (">=", BinaryOperator::GreaterOrEqual), ("<", BinaryOperator::Less), (">", BinaryOperator::Greater)], Self::additive)
    }

    fn additive(&mut self) -> Result<Expression, String> {
        self.binary(&[("+", BinaryOperator::Add), ("-", BinaryOperator::Subtract)], Self::multiplicative)
    }

    fn multiplicative(&mut self) -> Result<Expression, String> {
        self.binary(&[("*", BinaryOperator::Multiply), ("/", BinaryOperator::Divide), ("%", BinaryOperator::Remainder)], Self::unary)
    }

    fn unary(&mut self) -> Result<Expression, String> {
        let operator = [("-", UnaryOperator::Negate), ("+", UnaryOperator::Plus), ("!", UnaryOperator::Not)].into_iter().find(|(symbol, _)| self.eat(symbol));
        let Some((_, operator)) = operator else { return self.access() };
        self.nest()?;
        let operand = self.unary()?;
        self.nesting -= 1;
        Ok(Expression::Unary(operator, Box::new(operand)))
    }

    fn nest(&mut self) -> Result<(), String> {
        self.nesting += 1;
        if self.nesting > MAX_NESTING {
            return Err("nested too deeply".to_string());
        }
        Ok(())
    }

    fn access(&mut self) -> Result<Expression, String> {
        let mut expression = self.primary()?;
        loop {
            if self.eat(".") {
                let Some(Token::Name(name)) = self.tokens.get(self.at).cloned() else { return Err("expected a name after \".\"".to_string()) };
                self.at += 1;
                expression = Expression::Property(Box::new(expression), name);
            } else if self.eat("[") {
                self.nest()?;
                let index = self.filter_chain()?;
                self.nesting -= 1;
                self.expect("]")?;
                expression = Expression::Index(Box::new(expression), Box::new(index));
            } else if self.eat("(") {
                self.nest()?;
                while !self.eat(")") {
                    self.conditional()?;
                    if !self.eat(",") {
                        self.expect(")")?;
                        break;
                    }
                }
                self.nesting -= 1;
                expression = Expression::Call;
            } else {
                return Ok(expression);
            }
        }
    }

    fn primary(&mut self) -> Result<Expression, String> {
        let token = self.tokens.get(self.at).cloned().ok_or("the expression ends too soon")?;
        self.at += 1;
        match token {
            Token::Number(number) => Ok(Expression::Literal(float_value(number))),
            Token::Text(text) => Ok(Expression::Literal(Value::String(text))),
            Token::Name(name) => Ok(match name.as_str() {
                "true" => Expression::Literal(Value::Bool(true)),
                "false" => Expression::Literal(Value::Bool(false)),
                "null" => Expression::Literal(Value::Null),
                "undefined" => Expression::Undefined,
                "this" => Expression::This,
                "$index" => Expression::LoopIndex,
                _ => Expression::Identifier(name),
            }),
            Token::Symbol(".") => match self.tokens.get(self.at) {
                Some(Token::Name(name)) => Err(format!("unexpected \"{name}\" after \".\"")),
                _ => Ok(Expression::This),
            },
            Token::Symbol("(") => {
                self.nest()?;
                let inner = self.filter_chain()?;
                self.nesting -= 1;
                self.expect(")")?;
                Ok(inner)
            }
            Token::Symbol("[") => {
                self.nest()?;
                let mut items = Vec::new();
                while !self.eat("]") {
                    items.push(self.conditional()?);
                    if !self.eat(",") {
                        self.expect("]")?;
                        break;
                    }
                }
                self.nesting -= 1;
                Ok(Expression::Array(items))
            }
            other => Err(format!("unexpected {}", describe(&other))),
        }
    }
}

/// A result while evaluating. Arithmetic keeps floats, as `NaN` and `Infinity` fit no JSON number.
enum Evaluated<'a> {
    Undefined,
    Float(f64),
    Data(Cow<'a, Value>),
}

impl<'a> Evaluated<'a> {
    fn owned(value: Value) -> Evaluated<'a> {
        Evaluated::Data(Cow::Owned(value))
    }

    fn is_missing(&self) -> bool {
        match self {
            Evaluated::Undefined => true,
            Evaluated::Float(_) => false,
            Evaluated::Data(value) => value.is_null(),
        }
    }

    fn is_truthy(&self) -> bool {
        match self {
            Evaluated::Undefined => false,
            Evaluated::Float(number) => *number != 0.0 && !number.is_nan(),
            Evaluated::Data(value) => match &**value {
                Value::Null => false,
                Value::Bool(is_true) => *is_true,
                Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
                Value::String(text) => !text.is_empty(),
                Value::Array(_) | Value::Object(_) => true,
            },
        }
    }

    fn to_number(&self) -> f64 {
        match self {
            Evaluated::Undefined => f64::NAN,
            Evaluated::Float(number) => *number,
            Evaluated::Data(value) => match &**value {
                Value::Null => 0.0,
                Value::Bool(is_true) => f64::from(u8::from(*is_true)),
                Value::Number(number) => number.as_f64().unwrap_or(f64::NAN),
                Value::String(text) => text_to_number(text),
                Value::Array(_) => text_to_number(&self.to_text()),
                Value::Object(_) => f64::NAN,
            },
        }
    }

    fn to_text(&self) -> String {
        match self {
            Evaluated::Undefined => "undefined".to_string(),
            Evaluated::Float(number) => print_value(&float_value(*number)),
            Evaluated::Data(value) if value.is_null() => "null".to_string(),
            Evaluated::Data(value) => print_value(value),
        }
    }

    fn is_string(&self) -> bool {
        matches!(self, Evaluated::Data(value) if value.is_string())
    }

    /// Lists and objects turn to text before they are added or compared with something else.
    fn is_text_when_mixed(&self) -> bool {
        matches!(self, Evaluated::Data(value) if value.is_string() || value.is_array() || value.is_object())
    }

    fn is_collection(&self) -> bool {
        matches!(self, Evaluated::Data(value) if value.is_array() || value.is_object())
    }

    fn into_value(self) -> Option<Cow<'a, Value>> {
        match self {
            Evaluated::Undefined => None,
            Evaluated::Float(number) => Some(Cow::Owned(float_value(number))),
            Evaluated::Data(value) => Some(value),
        }
    }
}

fn float_value(number: f64) -> Value {
    if number.is_nan() {
        return json!("NaN");
    }
    if number.is_infinite() {
        return json!(if number > 0.0 { "Infinity" } else { "-Infinity" });
    }
    if number.fract() == 0.0 && number.abs() <= MAX_SAFE_INTEGER {
        return json!(number as i64);
    }
    Number::from_f64(number).map_or(Value::Null, Value::Number)
}

fn text_to_number(text: &str) -> f64 {
    let text = text.trim();
    match text {
        "" => 0.0,
        "Infinity" | "+Infinity" => f64::INFINITY,
        "-Infinity" => f64::NEG_INFINITY,
        // Rust also reads `inf` and `nan`, which JavaScript does not.
        _ if text.contains(|c: char| c.is_ascii_alphabetic() && c != 'e' && c != 'E') => f64::NAN,
        _ => text.parse().unwrap_or(f64::NAN),
    }
}

fn loosely_equal(left: &Evaluated, right: &Evaluated) -> bool {
    if left.is_missing() || right.is_missing() {
        return left.is_missing() && right.is_missing();
    }
    if left.is_collection() && right.is_collection() {
        return strictly_equal(left, right);
    }
    if left.is_text_when_mixed() && right.is_text_when_mixed() {
        return left.to_text() == right.to_text();
    }
    left.to_number() == right.to_number()
}

fn strictly_equal(left: &Evaluated, right: &Evaluated) -> bool {
    match (left, right) {
        (Evaluated::Undefined, Evaluated::Undefined) => true,
        (Evaluated::Float(left), Evaluated::Float(right)) => left == right,
        (Evaluated::Float(left), Evaluated::Data(right)) | (Evaluated::Data(right), Evaluated::Float(left)) => right.as_f64() == Some(*left),
        (Evaluated::Data(left), Evaluated::Data(right)) => match (&**left, &**right) {
            // JavaScript compares lists and objects by identity.
            (Value::Array(_) | Value::Object(_), Value::Array(_) | Value::Object(_)) => matches!((left, right), (Cow::Borrowed(left), Cow::Borrowed(right)) if std::ptr::eq(*left, *right)),
            (Value::Number(left), Value::Number(right)) => left.as_f64() == right.as_f64(),
            (left, right) => left == right,
        },
        _ => false,
    }
}

fn compare(operator: BinaryOperator, left: &Evaluated, right: &Evaluated) -> bool {
    let ordering = if left.is_text_when_mixed() && right.is_text_when_mixed() {
        Some(left.to_text().encode_utf16().cmp(right.to_text().encode_utf16()))
    } else {
        left.to_number().partial_cmp(&right.to_number())
    };
    let Some(ordering) = ordering else { return false };
    match operator {
        BinaryOperator::Less => ordering.is_lt(),
        BinaryOperator::LessOrEqual => ordering.is_le(),
        BinaryOperator::Greater => ordering.is_gt(),
        _ => ordering.is_ge(),
    }
}

fn add<'a>(left: Evaluated<'a>, right: Evaluated<'a>) -> Evaluated<'a> {
    match (left.is_missing(), right.is_missing()) {
        // `null + undefined` is undefined and `undefined + null` is null, as in core.
        (true, _) => right,
        (false, true) => left,
        (false, false) if left.is_text_when_mixed() || right.is_text_when_mixed() => Evaluated::owned(Value::String(left.to_text() + &right.to_text())),
        (false, false) => Evaluated::Float(left.to_number() + right.to_number()),
    }
}

fn arithmetic<'a>(operator: BinaryOperator, left: &Evaluated, right: &Evaluated) -> Evaluated<'a> {
    if left.is_missing() || right.is_missing() {
        return Evaluated::Undefined;
    }
    let (left, right) = (left.to_number(), right.to_number());
    Evaluated::Float(match operator {
        BinaryOperator::Subtract => left - right,
        BinaryOperator::Multiply => left * right,
        BinaryOperator::Divide => left / right,
        _ => left % right,
    })
}

fn property<'a>(object: Evaluated<'a>, name: &str) -> Evaluated<'a> {
    let Evaluated::Data(value) = object else { return Evaluated::Undefined };
    match &*value {
        Value::Array(items) if name == "length" => Evaluated::Float(items.len() as f64),
        Value::String(text) if name == "length" => Evaluated::Float(text.encode_utf16().count() as f64),
        Value::Array(_) => name.parse::<usize>().map_or(Evaluated::Undefined, |index| element(Evaluated::Data(value), index)),
        Value::String(_) => name.parse::<usize>().map_or(Evaluated::Undefined, |index| element(Evaluated::Data(value), index)),
        Value::Object(_) => match value {
            Cow::Borrowed(value) => value.get(name).map_or(Evaluated::Undefined, |field| Evaluated::Data(Cow::Borrowed(field))),
            Cow::Owned(value) => value.get(name).map_or(Evaluated::Undefined, |field| Evaluated::owned(field.clone())),
        },
        _ => Evaluated::Undefined,
    }
}

fn element(list: Evaluated<'_>, index: usize) -> Evaluated<'_> {
    let Evaluated::Data(value) = list else { return Evaluated::Undefined };
    match value {
        Cow::Borrowed(Value::Array(items)) => items.get(index).map_or(Evaluated::Undefined, |item| Evaluated::Data(Cow::Borrowed(item))),
        Cow::Owned(Value::Array(items)) => items.get(index).map_or(Evaluated::Undefined, |item| Evaluated::owned(item.clone())),
        Cow::Borrowed(Value::String(text)) => text_unit(text, index),
        Cow::Owned(Value::String(text)) => text_unit(&text, index),
        _ => Evaluated::Undefined,
    }
}

/// The UTF-16 unit at `index`, as JavaScript indexes text. Half of a pair prints as U+FFFD.
fn text_unit<'a>(text: &str, index: usize) -> Evaluated<'a> {
    text.encode_utf16().nth(index).map_or(Evaluated::Undefined, |unit| Evaluated::owned(Value::String(String::from_utf16_lossy(&[unit]))))
}

fn index<'a>(object: Evaluated<'a>, key: &Evaluated) -> Evaluated<'a> {
    if key.is_missing() {
        return Evaluated::Undefined;
    }
    let is_list = object.is_string() || matches!(&object, Evaluated::Data(value) if value.is_array());
    let number = key.to_number();
    if is_list && number.fract() == 0.0 && number >= 0.0 {
        return element(object, number as usize);
    }
    property(object, &key.to_text())
}

/// What one evaluation reads.
pub(crate) struct Evaluation<'e, 'a> {
    pub scopes: &'e [&'a Value],
    /// Each array loop item's index, innermost last.
    pub loop_indexes: &'e [usize],
    pub hooks: &'e mut dyn FillHooks,
}

impl<'a> Evaluation<'_, 'a> {
    /// The value of `expression`. Names are looked up from the innermost scope out; a whole result
    /// that is missing is tried again from the parent scope, so `{p.q}` finds an outer `p`.
    pub(crate) fn evaluate(&mut self, expression: &Expression) -> Option<Cow<'a, Value>> {
        (0..self.scopes.len()).rev().find_map(|level| {
            let result = self.at_level(expression, level);
            if result.is_missing() { None } else { result.into_value() }
        })
    }

    fn at_level(&mut self, expression: &Expression, level: usize) -> Evaluated<'a> {
        match expression {
            Expression::Literal(value) => Evaluated::owned(value.clone()),
            Expression::Undefined | Expression::Call => Evaluated::Undefined,
            Expression::This => Evaluated::Data(Cow::Borrowed(self.scopes[level])),
            // The innermost loop's, even when tried again from an outer scope, and null outside
            // any loop, as in core.
            Expression::LoopIndex => self.loop_indexes.last().map_or(Evaluated::owned(Value::Null), |&index| Evaluated::Float(index as f64)),
            Expression::Identifier(name) => self.identifier(name, level),
            Expression::Property(object, name) => property(self.at_level(object, level), name),
            Expression::Index(object, key) => {
                let object = self.at_level(object, level);
                let key = self.at_level(key, level);
                index(object, &key)
            }
            Expression::Array(items) => Evaluated::owned(Value::Array(items.iter().map(|item| self.json_at(item, level)).collect())),
            Expression::Unary(operator, operand) => {
                let operand = self.at_level(operand, level);
                match operator {
                    UnaryOperator::Not => Evaluated::owned(Value::Bool(!operand.is_truthy())),
                    // Undefined counts as zero here, though JavaScript gives NaN.
                    UnaryOperator::Negate if operand.is_missing() => Evaluated::Float(0.0),
                    UnaryOperator::Plus if operand.is_missing() => Evaluated::Float(0.0),
                    UnaryOperator::Negate => Evaluated::Float(-operand.to_number()),
                    UnaryOperator::Plus => Evaluated::Float(operand.to_number()),
                }
            }
            Expression::Binary(operator, left, right) => {
                let left = self.at_level(left, level);
                let right = self.at_level(right, level);
                match operator {
                    BinaryOperator::Add => add(left, right),
                    BinaryOperator::Equal => Evaluated::owned(Value::Bool(loosely_equal(&left, &right))),
                    BinaryOperator::NotEqual => Evaluated::owned(Value::Bool(!loosely_equal(&left, &right))),
                    BinaryOperator::StrictEqual => Evaluated::owned(Value::Bool(strictly_equal(&left, &right))),
                    BinaryOperator::StrictNotEqual => Evaluated::owned(Value::Bool(!strictly_equal(&left, &right))),
                    BinaryOperator::Less | BinaryOperator::LessOrEqual | BinaryOperator::Greater | BinaryOperator::GreaterOrEqual => Evaluated::owned(Value::Bool(compare(*operator, &left, &right))),
                    _ => arithmetic(*operator, &left, &right),
                }
            }
            Expression::And(left, right) => {
                let left = self.at_level(left, level);
                if left.is_truthy() { self.at_level(right, level) } else { left }
            }
            Expression::Or(left, right) => {
                let left = self.at_level(left, level);
                if left.is_truthy() { left } else { self.at_level(right, level) }
            }
            Expression::Conditional(condition, shown, otherwise) => {
                let chosen = if self.at_level(condition, level).is_truthy() { shown } else { otherwise };
                self.at_level(chosen, level)
            }
            Expression::Filter { input, name, arguments } => {
                let input = self.json_at(input, level);
                let arguments: Vec<Value> = arguments.iter().map(|argument| self.json_at(argument, level)).collect();
                Evaluated::owned(self.hooks.filter(name, &input, &arguments))
            }
        }
    }

    fn json_at(&mut self, expression: &Expression, level: usize) -> Value {
        self.at_level(expression, level).into_value().map_or(Value::Null, Cow::into_owned)
    }

    /// The name's value in the scope at `level`, else in the innermost scope that has one, as core
    /// reads it when it tries an expression again from an outer scope. A null is passed over for
    /// another value, and only stays when no scope has one.
    fn identifier(&self, name: &str, level: usize) -> Evaluated<'a> {
        let mut found = Evaluated::Undefined;
        for scope in std::iter::once(&self.scopes[level]).chain(self.scopes.iter().rev()) {
            match scope.get(name) {
                Some(value) if !value.is_null() => return Evaluated::Data(Cow::Borrowed(value)),
                Some(value) => found = Evaluated::Data(Cow::Borrowed(value)),
                None => {}
            }
        }
        found
    }
}

/// Keeps values worked out during a fill, so they can be scopes as the data's own values are.
#[derive(Default)]
pub(crate) struct Computed {
    first: OnceCell<Box<Node>>,
}

struct Node {
    value: Value,
    next: OnceCell<Box<Node>>,
}

/// Where the next computed value goes.
pub(crate) struct ComputedEnd<'a> {
    free: &'a OnceCell<Box<Node>>,
}

impl Computed {
    pub(crate) fn end(&self) -> ComputedEnd<'_> {
        ComputedEnd { free: &self.first }
    }
}

impl<'a> ComputedEnd<'a> {
    pub(crate) fn keep(&mut self, value: Value) -> &'a Value {
        let node = self.free.get_or_init(|| Box::new(Node { value, next: OnceCell::new() }));
        self.free = &node.next;
        &node.value
    }
}

impl Drop for Computed {
    // One by one: dropping a long list recursively would overflow the stack.
    fn drop(&mut self) {
        let mut next = self.first.take();
        while let Some(mut node) = next {
            next = node.next.take();
        }
    }
}
