// SPDX-License-Identifier: AGPL-3.0-or-later

use super::computed::ComputedError;
use crate::{Error, Result};
use chrono::{DateTime, Datelike, NaiveDate};
use serde_json::{Number, Value};
use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::ops::Range;

const MAX_EXPRESSION_BYTES: usize = 8_192;
const MAX_TOKENS: usize = 256;
const MAX_PARSE_DEPTH: usize = 32;
const MAX_TEXT: usize = 2_048;

#[derive(Debug)]
pub(super) enum Expression {
    Literal(Value),
    Field(String),
    Unary(Operator, Box<Expression>),
    Binary(Operator, Box<Expression>, Box<Expression>),
    Call(Function, Vec<Expression>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Operator {
    Plus,
    Minus,
    Times,
    Divide,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    And,
    Or,
    Not,
}

impl Operator {
    fn precedence(self) -> u8 {
        match self {
            Self::Or => 1,
            Self::And => 2,
            Self::Equal | Self::NotEqual => 3,
            Self::Less | Self::LessEqual | Self::Greater | Self::GreaterEqual => 4,
            Self::Plus | Self::Minus => 5,
            Self::Times | Self::Divide => 6,
            Self::Not => 0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Function {
    If,
    Coalesce,
    IsEmpty,
    Text,
    Length,
    Lower,
    Upper,
    Concat,
    Contains,
    Date,
    Year,
    Month,
    Day,
    DaysBetween,
    UtcDate,
}

impl Function {
    fn named(name: &str) -> Option<(Self, usize)> {
        Some(match name {
            "if" => (Self::If, 3),
            "coalesce" => (Self::Coalesce, 2),
            "isEmpty" => (Self::IsEmpty, 1),
            "text" => (Self::Text, 1),
            "length" => (Self::Length, 1),
            "lower" => (Self::Lower, 1),
            "upper" => (Self::Upper, 1),
            "concat" => (Self::Concat, 2),
            "contains" => (Self::Contains, 2),
            "date" => (Self::Date, 1),
            "year" => (Self::Year, 1),
            "month" => (Self::Month, 1),
            "day" => (Self::Day, 1),
            "daysBetween" => (Self::DaysBetween, 2),
            "utcDate" => (Self::UtcDate, 1),
            _ => return None,
        })
    }
}

#[derive(Debug)]
enum Token {
    Value(Value),
    Word(String),
    Operator(Operator),
    Open,
    Close,
    Comma,
    End,
}

pub(super) struct Formula {
    pub expression: Expression,
    pub fields: BTreeSet<String>,
}

pub(super) fn compile(
    source: &str,
    resolve: impl Fn(&str) -> Option<String>,
) -> Result<(String, Formula)> {
    if source.len() > MAX_EXPRESSION_BYTES {
        return Err(limit());
    }
    let mut parser = Parser {
        source,
        offset: 0,
        consumed: 0,
        next: (Token::End, 0..0),
        replacements: Vec::new(),
        fields: BTreeSet::new(),
        resolve,
    };
    parser.advance()?;
    let expression = parser.expression(1, 0)?;
    if !matches!(parser.next.0, Token::End) {
        return Err(syntax("表达式末尾存在多余内容。"));
    }
    let mut canonical = String::new();
    let mut start = 0;
    for (range, id) in parser.replacements {
        canonical.push_str(&source[start..range.start]);
        canonical.push_str(&serde_json::to_string(&id)?);
        start = range.end;
    }
    canonical.push_str(&source[start..]);
    if canonical.len() > MAX_EXPRESSION_BYTES {
        return Err(limit());
    }
    Ok((
        canonical,
        Formula {
            expression,
            fields: parser.fields,
        },
    ))
}

pub(crate) fn remap_fields(source: &str, map: impl Fn(&str) -> Option<String>) -> Result<String> {
    compile(source, map).map(|(expression, _)| expression)
}

struct Parser<'a, F> {
    source: &'a str,
    offset: usize,
    consumed: usize,
    next: (Token, Range<usize>),
    replacements: Vec<(Range<usize>, String)>,
    fields: BTreeSet<String>,
    resolve: F,
}

impl<F: Fn(&str) -> Option<String>> Parser<'_, F> {
    fn advance(&mut self) -> Result<(Token, Range<usize>)> {
        let rest = self.source[self.offset..].trim_start();
        let start = self.source.len() - rest.len();
        let (token, length) = if rest.is_empty() {
            (Token::End, 0)
        } else {
            self.consumed += 1;
            if self.consumed > MAX_TOKENS {
                return Err(limit());
            }
            lex(rest)?
        };
        self.offset = start + length;
        Ok(std::mem::replace(
            &mut self.next,
            (token, start..self.offset),
        ))
    }

    fn expression(&mut self, minimum: u8, depth: usize) -> Result<Expression> {
        if depth >= MAX_PARSE_DEPTH {
            return Err(limit());
        }
        let mut left = match self.advance()?.0 {
            Token::Value(value) => Expression::Literal(value),
            Token::Operator(op @ (Operator::Minus | Operator::Not)) => {
                Expression::Unary(op, Box::new(self.expression(7, depth + 1)?))
            }
            Token::Open => {
                let node = self.expression(1, depth + 1)?;
                self.close()?;
                node
            }
            Token::Word(name) => self.call(&name, depth + 1)?,
            _ => return Err(syntax("这里需要字面量、属性、函数或括号。")),
        };
        while let Token::Operator(op) = self.next.0 {
            let precedence = op.precedence();
            if precedence < minimum {
                break;
            }
            self.advance()?;
            let right = self.expression(precedence + 1, depth + 1)?;
            left = Expression::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn close(&mut self) -> Result<()> {
        if matches!(self.advance()?.0, Token::Close) {
            Ok(())
        } else {
            Err(syntax("这里需要右括号。"))
        }
    }

    fn call(&mut self, name: &str, depth: usize) -> Result<Expression> {
        if !matches!(self.advance()?.0, Token::Open) {
            return Err(syntax("函数名后需要参数括号。"));
        }
        if name == "prop" {
            let (Token::Value(Value::String(field)), range) = self.advance()? else {
                return Err(syntax("prop 需要一个带双引号的属性名称。"));
            };
            let id = (self.resolve)(&field)
                .ok_or_else(|| Error::new("FORMULA_PROPERTY", "公式引用了找不到的属性。"))?;
            self.close()?;
            self.fields.insert(id.clone());
            self.replacements.push((range, id.clone()));
            return Ok(Expression::Field(id));
        }
        let (function, count) = Function::named(name).ok_or_else(|| syntax("未知函数。"))?;
        let mut arguments = Vec::with_capacity(count);
        for index in 0..count {
            if index != 0 && !matches!(self.advance()?.0, Token::Comma) {
                return Err(syntax("函数参数数量错误。"));
            }
            arguments.push(self.expression(1, depth)?);
        }
        self.close()?;
        Ok(Expression::Call(function, arguments))
    }
}

fn syntax(message: &str) -> Error {
    Error::new("FORMULA_SYNTAX", message)
}

fn limit() -> Error {
    Error::new("LIMIT_EXCEEDED", "公式超过表达式长度、项数或嵌套上限。")
}

fn lex(source: &str) -> Result<(Token, usize)> {
    for (symbol, op) in [
        ("==", Operator::Equal),
        ("!=", Operator::NotEqual),
        ("<=", Operator::LessEqual),
        (">=", Operator::GreaterEqual),
        ("&&", Operator::And),
        ("||", Operator::Or),
        ("+", Operator::Plus),
        ("-", Operator::Minus),
        ("*", Operator::Times),
        ("/", Operator::Divide),
        ("<", Operator::Less),
        (">", Operator::Greater),
        ("!", Operator::Not),
    ] {
        if source.starts_with(symbol) {
            return Ok((Token::Operator(op), symbol.len()));
        }
    }
    let bytes = source.as_bytes();
    let one = match bytes[0] {
        b'(' => Token::Open,
        b')' => Token::Close,
        b',' => Token::Comma,
        b'"' => {
            let mut escaped = false;
            for (index, byte) in bytes.iter().enumerate().skip(1) {
                if escaped {
                    escaped = false;
                } else if *byte == b'\\' {
                    escaped = true;
                } else if *byte == b'"' {
                    let length = index + 1;
                    let text: String = serde_json::from_str(&source[..length])
                        .map_err(|_| syntax("文字字面量无效。"))?;
                    if text.chars().count() > MAX_TEXT {
                        return Err(limit());
                    }
                    return Ok((Token::Value(Value::String(text)), length));
                }
            }
            return Err(syntax("文字缺少结束引号。"));
        }
        byte if byte.is_ascii_digit() => {
            let mut length = digits(bytes, 0);
            if bytes.get(length) == Some(&b'.') {
                length = digits(bytes, length + 1);
            }
            if matches!(bytes.get(length), Some(b'e' | b'E')) {
                length += 1;
                if matches!(bytes.get(length), Some(b'+' | b'-')) {
                    length += 1;
                }
                length = digits(bytes, length);
            }
            let number = serde_json::from_str::<Number>(&source[..length])
                .map_err(|_| syntax("数字字面量无效。"))?;
            return Ok((Token::Value(Value::Number(number)), length));
        }
        byte if byte.is_ascii_alphabetic() => {
            let length = bytes
                .iter()
                .take_while(|b| b.is_ascii_alphanumeric())
                .count();
            let word = &source[..length];
            let token = match word {
                "true" => Token::Value(Value::Bool(true)),
                "false" => Token::Value(Value::Bool(false)),
                "null" => Token::Value(Value::Null),
                _ => Token::Word(word.into()),
            };
            return Ok((token, length));
        }
        _ => return Err(syntax("表达式含有无法识别的字符。")),
    };
    Ok((one, 1))
}

fn digits(bytes: &[u8], start: usize) -> usize {
    start
        + bytes[start..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count()
}

type ValueResult<T = Value> = std::result::Result<T, ComputedError>;

pub(super) fn number(value: f64) -> ValueResult {
    Number::from_f64(value)
        .map(Value::Number)
        .ok_or(ComputedError::OutOfRange)
}

pub(super) fn boolean(value: &Value) -> ValueResult<bool> {
    value.as_bool().ok_or(ComputedError::TypeError)
}

pub(super) fn unary(op: Operator, value: Value) -> ValueResult {
    match op {
        Operator::Not => Ok(Value::Bool(!boolean(&value)?)),
        Operator::Minus if value.is_null() => Ok(Value::Null),
        Operator::Minus => number(-numeric(&value)?),
        _ => unreachable!("parser constructs unary operators"),
    }
}

pub(super) fn binary(op: Operator, left: Value, right: Value) -> ValueResult {
    use Operator::*;
    if matches!(op, Equal | NotEqual) {
        let equal = if left.is_null() || right.is_null() {
            left.is_null() && right.is_null()
        } else {
            compare(&left, &right)? == Ordering::Equal
        };
        return Ok(Value::Bool(if op == Equal { equal } else { !equal }));
    }
    if left.is_null() || right.is_null() {
        return Ok(Value::Null);
    }
    match op {
        Less | LessEqual | Greater | GreaterEqual => {
            let order = compare(&left, &right)?;
            Ok(Value::Bool(match op {
                Less => order.is_lt(),
                LessEqual => order.is_le(),
                Greater => order.is_gt(),
                _ => order.is_ge(),
            }))
        }
        Plus => number(numeric(&left)? + numeric(&right)?),
        Minus => number(numeric(&left)? - numeric(&right)?),
        Times => number(numeric(&left)? * numeric(&right)?),
        Divide => {
            let (left, right) = (numeric(&left)?, numeric(&right)?);
            if right == 0.0 {
                return Err(ComputedError::DivisionByZero);
            }
            number(left / right)
        }
        _ => unreachable!("boolean operators are evaluated lazily"),
    }
}

pub(super) fn apply(function: Function, values: &[Value]) -> ValueResult {
    use Function::*;
    let first = &values[0];
    if matches!(function, IsEmpty) {
        return Ok(Value::Bool(first.is_null() || first.as_str() == Some("")));
    }
    if values.iter().any(Value::is_null) {
        return Ok(Value::Null);
    }
    match function {
        Text => match first {
            Value::String(_) => Ok(first.clone()),
            Value::Bool(_) | Value::Number(_) => Ok(Value::String(first.to_string())),
            _ => Err(ComputedError::TypeError),
        },
        Length => Ok(Value::from(text(first)?.chars().count())),
        Lower => bounded_text(text(first)?.to_lowercase()),
        Upper => bounded_text(text(first)?.to_uppercase()),
        Concat => bounded_text(format!("{}{}", text(first)?, text(&values[1])?)),
        Contains => Ok(Value::Bool(text(first)?.contains(text(&values[1])?))),
        Date => Ok(Value::String(date(first)?.to_string())),
        Year => Ok(Value::from(date(first)?.year())),
        Month => Ok(Value::from(date(first)?.month())),
        Day => Ok(Value::from(date(first)?.day())),
        DaysBetween => Ok(Value::from((date(&values[1])? - date(first)?).num_days())),
        UtcDate => {
            let millis = first
                .as_i64()
                .or_else(|| {
                    first
                        .as_f64()
                        .filter(|value| {
                            value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_991.0
                        })
                        .map(|value| value as i64)
                })
                .ok_or(ComputedError::TypeError)?;
            let time = DateTime::from_timestamp_millis(millis).ok_or(ComputedError::InvalidDate)?;
            if !(1..=9999).contains(&time.year()) {
                return Err(ComputedError::InvalidDate);
            }
            Ok(Value::String(time.date_naive().to_string()))
        }
        If | Coalesce | IsEmpty => unreachable!("special forms handled before regular functions"),
    }
}

fn compare(left: &Value, right: &Value) -> ValueResult<Ordering> {
    match (left, right) {
        (Value::String(a), Value::String(b)) => Ok(a.cmp(b)),
        (Value::Bool(a), Value::Bool(b)) => Ok(a.cmp(b)),
        (Value::Number(_), Value::Number(_)) => numeric(left)?
            .partial_cmp(&numeric(right)?)
            .ok_or(ComputedError::OutOfRange),
        _ => Err(ComputedError::TypeError),
    }
}

fn numeric(value: &Value) -> ValueResult<f64> {
    value.as_f64().ok_or(ComputedError::TypeError)
}

fn bounded_text(text: String) -> ValueResult {
    if text.chars().count() > MAX_TEXT {
        return Err(ComputedError::LimitExceeded);
    }
    Ok(Value::String(text))
}

fn text(value: &Value) -> ValueResult<&str> {
    value.as_str().ok_or(ComputedError::TypeError)
}

fn date(value: &Value) -> ValueResult<NaiveDate> {
    let text = text(value)?;
    if !super::schema::valid_date(text) {
        return Err(ComputedError::InvalidDate);
    }
    NaiveDate::parse_from_str(text, "%Y-%m-%d").map_err(|_| ComputedError::InvalidDate)
}
