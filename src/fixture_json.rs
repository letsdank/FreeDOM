//! Partial JSON fixture reader used by the WPT adapters.
//!
//! <https://tc39.es/ecma262/#sec-json.parse>
//!
//! This reader currently reuses the JavaScript tokenizer. Its accepted syntax
//! is not yet equivalent to JSON, so it must not be treated as a JSON validator.

use crate::fixture_cursor::TokenCursor;
use crate::javascript::{Token, TokenKind, tokenize};
use std::{collections::HashMap, error::Error, fmt};

/// Syntax error from the partial fixture reader, with a byte offset.
#[derive(Debug)]
pub(crate) struct FixtureJsonError {
    pub(crate) offset: Option<usize>,
    pub(crate) message: String,
}

impl FixtureJsonError {
    pub(crate) fn syntax(offset: usize, message: impl Into<String>) -> Self {
        Self {
            offset: Some(offset),
            message: message.into(),
        }
    }
}

impl fmt::Display for FixtureJsonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.offset {
            Some(offset) => write!(formatter, "{} at byte {}", self.message, offset),
            None => formatter.write_str(&self.message),
        }
    }
}

impl Error for FixtureJsonError {}

/// Data parsed from fixtures, independent of JavaScript object storage.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum JsonValue {
    Null,
    Boolean(bool),
    Number(f64),
    String(String),
    Array(Vec<JsonValue>),
    Object(HashMap<String, JsonValue>),
}

impl JsonValue {
    pub(crate) fn as_array(&self) -> Option<&[JsonValue]> {
        match self {
            Self::Array(value) => Some(value),
            _ => None,
        }
    }

    pub(crate) fn as_object(&self) -> Option<&HashMap<String, JsonValue>> {
        match self {
            Self::Object(value) => Some(value),
            _ => None,
        }
    }

    pub(crate) fn as_string(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    pub(crate) fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Boolean(value) => Some(*value),
            _ => None,
        }
    }
}

pub(crate) fn parse_json(source: &str) -> Result<JsonValue, FixtureJsonError> {
    let tokens =
        tokenize(source).map_err(|error| FixtureJsonError::syntax(error.offset, error.message))?;

    JsonParser::new(tokens).parse()
}

struct JsonParser {
    cursor: TokenCursor,
}

impl JsonParser {
    fn new(tokens: Vec<Token>) -> Self {
        Self {
            cursor: TokenCursor::new(tokens),
        }
    }

    fn parse(&mut self) -> Result<JsonValue, FixtureJsonError> {
        let value = self.parse_value()?;

        if !self.cursor.check(&TokenKind::EndOfFile) {
            return Err(FixtureJsonError::syntax(
                self.cursor.peek().offset,
                "unexpected token after JSON value",
            ));
        }

        Ok(value)
    }

    fn parse_value(&mut self) -> Result<JsonValue, FixtureJsonError> {
        let token = self.cursor.advance();

        match token.kind {
            TokenKind::String(value) => Ok(JsonValue::String(value)),
            TokenKind::Number(value) => Ok(JsonValue::Number(value)),
            TokenKind::True => Ok(JsonValue::Boolean(true)),
            TokenKind::False => Ok(JsonValue::Boolean(false)),
            TokenKind::Null => Ok(JsonValue::Null),

            TokenKind::LeftBracket => self.parse_array(),
            TokenKind::LeftBrace => self.parse_object(),

            TokenKind::EndOfFile => Err(FixtureJsonError::syntax(
                token.offset,
                "unexpected end of JSON input",
            )),

            _ => Err(FixtureJsonError::syntax(
                token.offset,
                "expected a JSON value",
            )),
        }
    }

    fn parse_array(&mut self) -> Result<JsonValue, FixtureJsonError> {
        let mut values = Vec::new();

        if self.cursor.consume_optional(&TokenKind::RightBracket) {
            return Ok(JsonValue::Array(values));
        }

        loop {
            values.push(self.parse_value()?);

            if self.cursor.consume_optional(&TokenKind::Comma) {
                if self.cursor.check(&TokenKind::RightBracket) {
                    return Err(FixtureJsonError::syntax(
                        self.cursor.peek().offset,
                        "trailing comma is not valid JSON",
                    ));
                }

                continue;
            }

            self.cursor.consume(
                &TokenKind::RightBracket,
                "expected ',' or ']' after JSON array item",
            )?;

            return Ok(JsonValue::Array(values));
        }
    }

    fn parse_object(&mut self) -> Result<JsonValue, FixtureJsonError> {
        let mut values = HashMap::new();

        if self.cursor.consume_optional(&TokenKind::RightBrace) {
            return Ok(JsonValue::Object(values));
        }

        loop {
            let token = self.cursor.advance();

            let TokenKind::String(key) = token.kind else {
                return Err(FixtureJsonError::syntax(
                    token.offset,
                    "JSON object keys must be strings",
                ));
            };

            self.cursor
                .consume(&TokenKind::Colon, "expected ':' after JSON object key")?;

            let value = self.parse_value()?;
            values.insert(key, value);

            if self.cursor.consume_optional(&TokenKind::Comma) {
                if self.cursor.check(&TokenKind::RightBrace) {
                    return Err(FixtureJsonError::syntax(
                        self.cursor.peek().offset,
                        "trailing comma is not valid JSON",
                    ));
                }

                continue;
            }

            self.cursor.consume(
                &TokenKind::RightBrace,
                "expected ',' or '}' after JSON object property",
            )?;

            return Ok(JsonValue::Object(values));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{JsonValue, parse_json};

    #[test]
    fn json_lexing_errors_preserve_offsets() {
        let error = parse_json("@").unwrap_err();

        assert_eq!(error.offset, Some(0));
        assert_eq!(error.to_string(), "unexpected character '@' at byte 0");
    }

    #[test]
    fn reads_supported_nested_fixture_values() {
        let value =
            parse_json(r#"{"name":"example","items":[1,true,null],"child":{"ok":false}}"#).unwrap();

        let object = value.as_object().unwrap();

        assert_eq!(
            object.get("name"),
            Some(&JsonValue::String("example".to_owned()))
        );

        assert_eq!(
            object.get("items").unwrap().as_array().unwrap(),
            vec![
                JsonValue::Number(1.0),
                JsonValue::Boolean(true),
                JsonValue::Null
            ]
        );

        let child = object.get("child").unwrap().as_object().unwrap();
        assert_eq!(child.get("ok"), Some(&JsonValue::Boolean(false)));
    }

    #[test]
    fn rejects_trailing_commas_and_extra_values() {
        for input in [r#"{"a":1,}"#, "[1,]", "true false"] {
            assert!(parse_json(input).is_err(), "{input:?}");
        }
    }
}
