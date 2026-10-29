//! Partial JSON fixture reader used by the WPT adapters.
//!
//! <https://tc39.es/ecma262/#sec-json.parse>
//!
//! This reader currently reuses the JavaScript tokenizer. Its accepted syntax
//! is not yet equivalent to JSON, so it must not be treated as a JSON validator.

use super::token_cursor::TokenCursor;
use std::collections::HashMap;

use super::{JsError, Token, TokenKind, Value, tokenize};

pub fn parse_json(source: &str) -> Result<Value, JsError> {
    let tokens = tokenize(source).map_err(|error| JsError {
        offset: Some(error.offset),
        message: error.message,
    })?;

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

    fn parse(&mut self) -> Result<Value, JsError> {
        let value = self.parse_value()?;

        if !self.cursor.check(&TokenKind::EndOfFile) {
            return Err(JsError::syntax(
                self.cursor.peek().offset,
                "unexpected token after JSON value",
            ));
        }

        Ok(value)
    }

    fn parse_value(&mut self) -> Result<Value, JsError> {
        let token = self.cursor.advance();

        match token.kind {
            TokenKind::String(value) => Ok(Value::String(value)),
            TokenKind::Number(value) => Ok(Value::Number(value)),
            TokenKind::True => Ok(Value::Boolean(true)),
            TokenKind::False => Ok(Value::Boolean(false)),
            TokenKind::Null => Ok(Value::Null),

            TokenKind::LeftBracket => self.parse_array(),
            TokenKind::LeftBrace => self.parse_object(),

            TokenKind::EndOfFile => Err(JsError::syntax(
                token.offset,
                "unexpected end of JSON input",
            )),

            _ => Err(JsError::syntax(token.offset, "expected a JSON value")),
        }
    }

    fn parse_array(&mut self) -> Result<Value, JsError> {
        let mut values = Vec::new();

        if self.cursor.consume_optional(&TokenKind::RightBracket) {
            return Ok(Value::array(values));
        }

        loop {
            values.push(self.parse_value()?);

            if self.cursor.consume_optional(&TokenKind::Comma) {
                if self.cursor.check(&TokenKind::RightBracket) {
                    return Err(JsError::syntax(
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

            return Ok(Value::array(values));
        }
    }

    fn parse_object(&mut self) -> Result<Value, JsError> {
        let mut values = HashMap::new();

        if self.cursor.consume_optional(&TokenKind::RightBrace) {
            return Ok(Value::object(values));
        }

        loop {
            let token = self.cursor.advance();

            let TokenKind::String(key) = token.kind else {
                return Err(JsError::syntax(
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
                    return Err(JsError::syntax(
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

            return Ok(Value::object(values));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Value, parse_json};

    #[test]
    fn reads_supported_nested_fixture_values() {
        let value =
            parse_json(r#"{"name":"example","items":[1,true,null],"child":{"ok":false}}"#).unwrap();

        let object = value.as_object().unwrap();

        assert_eq!(
            object.get("name"),
            Some(&Value::String("example".to_owned()))
        );

        assert_eq!(
            object.get("items").unwrap().as_array().unwrap(),
            vec![Value::Number(1.0), Value::Boolean(true), Value::Null]
        );

        let child = object.get("child").unwrap().as_object().unwrap();
        assert_eq!(child.get("ok"), Some(&Value::Boolean(false)));
    }

    #[test]
    fn rejects_trailing_commas_and_extra_values() {
        for input in [r#"{"a":1,}"#, "[1,]", "true false"] {
            assert!(parse_json(input).is_err(), "{input:?}");
        }
    }
}
