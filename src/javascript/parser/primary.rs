//! Partial parser for primary expressions and literal containers.
//!
//! <https://tc39.es/ecma262/#sec-primary-expression>
//!
//! This module implements only the interpreter's existing grammar subset.

use super::{
    Expression, FunctionKind, FunctionParameter, JsError, Parser, TokenKind, UserFunction, Value,
};

impl Parser<'_> {
    pub(super) fn parse_primary(&mut self) -> Result<Expression, JsError> {
        let token = self.cursor.advance_primary()?;

        match token.kind {
            TokenKind::String(value) => Ok(Expression::Literal(Value::String(value))),
            TokenKind::TemplateChunk { value, tail } => self.parse_template(value, tail),
            TokenKind::RegExpLiteral { pattern, flags } => {
                crate::javascript::regexp::RegExpFlags::parse(&flags).map_err(|error| {
                    JsError::syntax(
                        token.offset,
                        format!("invalid RegExp flags: {:?}", error.kind),
                    )
                })?;

                Ok(Expression::RegExpLiteral { pattern, flags })
            }
            TokenKind::Number(value) => Ok(Expression::Literal(Value::Number(value))),
            TokenKind::True => Ok(Expression::Literal(Value::Boolean(true))),
            TokenKind::False => Ok(Expression::Literal(Value::Boolean(false))),
            TokenKind::Null => Ok(Expression::Literal(Value::Null)),

            TokenKind::Identifier(name) => Ok(Expression::Identifier(name)),

            TokenKind::New => self.parse_new_expression(token.offset),

            TokenKind::EndOfFile => Err(JsError::syntax(
                token.offset,
                "unexpected end of JavaScript source",
            )),

            TokenKind::Function => self.parse_function_expression(),

            TokenKind::LeftParen => {
                if self.cursor.consume_optional(&TokenKind::RightParen)? {
                    if self.cursor.consume_optional(&TokenKind::Arrow)? {
                        return Ok(Expression::Function(UserFunction {
                            kind: FunctionKind::Arrow,
                            parameters: Vec::new(),
                            body: self.parse_arrow_body()?,
                        }));
                    }

                    return Err(JsError::syntax(
                        token.offset,
                        "expected an expression inside parentheses",
                    ));
                }

                if let TokenKind::Identifier(parameter) = self.cursor.peek()?.kind.clone() {
                    let has_closing_parenthesis = self
                        .cursor
                        .peek_n(1)?
                        .is_some_and(|token| matches!(token.kind, TokenKind::RightParen));

                    let has_arrow = self
                        .cursor
                        .peek_n(2)?
                        .is_some_and(|token| matches!(token.kind, TokenKind::Arrow));

                    if has_closing_parenthesis && has_arrow {
                        self.cursor.advance()?;
                        self.cursor.advance()?;
                        self.cursor.advance()?;

                        return Ok(Expression::Function(UserFunction {
                            kind: FunctionKind::Arrow,
                            parameters: vec![FunctionParameter::simple(parameter)],
                            body: self.parse_arrow_body()?,
                        }));
                    }
                }

                let expression = self.parse_expression()?;
                self.cursor
                    .consume(&TokenKind::RightParen, "expected ')' after expression")?;

                Ok(expression)
            }

            TokenKind::LeftBracket => self.parse_array_literal(),

            TokenKind::LeftBrace => self.parse_object_literal(),

            _ => Err(JsError::syntax(token.offset, "expected an expression")),
        }
    }

    fn parse_array_literal(&mut self) -> Result<Expression, JsError> {
        let mut values = Vec::new();

        loop {
            if self.cursor.consume_optional(&TokenKind::RightBracket)? {
                return Ok(Expression::Array(values));
            }

            if self.cursor.consume_optional(&TokenKind::Comma)? {
                values.push(None);
                continue;
            }

            values.push(Some(self.parse_expression()?));

            if self.cursor.consume_optional(&TokenKind::RightBracket)? {
                return Ok(Expression::Array(values));
            }

            self.cursor
                .consume(&TokenKind::Comma, "expected ',' or ']' after array element")?;
        }
    }

    fn parse_object_literal(&mut self) -> Result<Expression, JsError> {
        let mut properties = Vec::new();

        if self.cursor.consume_optional(&TokenKind::RightBrace)? {
            return Ok(Expression::Object(properties));
        }

        loop {
            let token = self.cursor.advance()?;

            let (name, shorthand_allowed) = match token.kind {
                TokenKind::Identifier(name) => (name, true),

                TokenKind::String(name) => (name, false),

                TokenKind::Number(number) => {
                    let name = crate::javascript::number::integer_property_key(number).ok_or_else(
                        || {
                            JsError::unsupported(
                                "numeric property key formatting is not implemented for this number",
                            )
                        },
                    )?;

                    (name, false)
                }

                _ => {
                    return Err(JsError::syntax(
                        token.offset,
                        "expected an object property name",
                    ));
                }
            };

            let value = if self.cursor.consume_optional(&TokenKind::Colon)? {
                self.parse_expression()?
            } else if shorthand_allowed {
                Expression::Identifier(name.clone())
            } else {
                return Err(JsError::syntax(
                    self.cursor.peek()?.offset,
                    "expected ':' after object property name",
                ));
            };

            properties.push((name, value));

            if self.cursor.consume_optional(&TokenKind::Comma)? {
                if self.cursor.consume_optional(&TokenKind::RightBrace)? {
                    return Ok(Expression::Object(properties));
                }

                continue;
            }

            self.cursor.consume(
                &TokenKind::RightBrace,
                "expected ',' or '}' after object property",
            )?;

            return Ok(Expression::Object(properties));
        }
    }

    fn parse_template(&mut self, head: String, mut tail: bool) -> Result<Expression, JsError> {
        let mut parts = Vec::new();

        while !tail {
            let expression = self.parse_expression()?;

            self.cursor.consume(
                &TokenKind::RightBrace,
                "expected '}' after template expression",
            )?;

            let token = self.cursor.advance_template_chunk()?;

            let TokenKind::TemplateChunk {
                value,
                tail: is_tail,
            } = token.kind
            else {
                unreachable!("template scanner returns a chunk");
            };

            parts.push((expression, value));
            tail = is_tail;
        }

        Ok(Expression::Template { head, parts })
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::parser::parse_program;
    use crate::javascript::{Expression, Statement};

    #[test]
    fn parses_nested_object_and_array_literals() {
        let program = parse_program("const value = {items: [1, 2]};").unwrap();

        let Statement::Declaration {
            name,
            value: Expression::Object(fields),
        } = &program[0]
        else {
            panic!("expected object declaration");
        };

        assert_eq!(name, "value");
        assert_eq!(fields[0].0, "items");

        assert!(matches!(
            &fields[0].1,
            Expression::Array(items) if items.len()==2
        ));
    }

    #[test]
    fn reports_missing_array_separator_at_next_token() {
        let input = "const x = [1 2];";
        let error = parse_program(input).unwrap_err();

        assert_eq!(error.offset, input.find('2'));
        assert_eq!(error.message, "expected ',' or ']' after array element");
    }

    #[test]
    fn parses_arrow_function_across_parser_modules() {
        let program = parse_program("const f = (x) => { return x; };").unwrap();

        let Statement::Declaration {
            value: Expression::Function(function),
            ..
        } = &program[0]
        else {
            panic!("expected function declaration value");
        };

        assert_eq!(function.parameters.len(), 1);
        assert_eq!(function.parameters[0].name, "x");
        assert!(function.parameters[0].default.is_none());

        assert!(matches!(
            function.body.as_slice(),
            [Statement::Return(Expression::Identifier(name))]
            if name == "x"
        ));
    }

    #[test]
    fn parses_template_statement_from_real_testharness() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        let line = source
            .lines()
            .find(|line| line.contains("const formatEscapePattern = new RegExp(`"))
            .expect("WPT template must exist")
            .trim();

        let program = parse_program(line).unwrap();

        assert!(matches!(
            program.as_slice(),
            [Statement::Declaration {
                value: Expression::New { arguments, .. },
                ..
            }]
                if matches!(
                    arguments.first(),
                    Some(Expression::Template { parts, .. })
                        if parts.len() == 1
                )
        ));
    }

    #[test]
    fn parses_shorthand_property_from_testharness() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        assert!(source.contains("{func}"));

        let program = parse_program("let value = {func};").unwrap();

        assert!(matches!(
            program.as_slice(),
            [Statement::Declaration {
                value:Expression::Object(properties),
                ..
            }]
                if matches!(
                    properties.as_slice(),
                    [(name, Expression::Identifier(identifier))]
                        if name == "func" && identifier == "func"
                )
        ));
    }

    #[test]
    fn rejects_invalid_shorthand_forms() {
        for source in ["let o = {'x'};", "let o = {x = 1};", "let o = {x y};"] {
            assert!(parse_program(source).is_err(), "{source}");
        }
    }

    #[test]
    fn parses_numeric_object_keys_from_testharness() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        let marker = "Test.prototype.status_formats = {";

        let start = source
            .find(marker)
            .expect("WPT fixture must contain the object");

        let rest = &source[start..];

        let end = rest.find("};").expect("object must be terminated") + 2;

        let program = parse_program(&rest[..end]).unwrap();

        assert!(matches!(
            program.as_slice(),
            [Statement::Expression(Expression::Assignment {
                value,
                ..
            })]
                if matches!(
                    value.as_ref(),
                    Expression::Object(properties)
                        if properties
                            .iter()
                            .map(|(name, _)| name.as_str())
                            .eq(["0", "1", "2", "3", "4"])
            )
        ));
    }

    #[test]
    fn rejects_unsupported_numeric_key_formatting() {
        for source in ["let x = {1.5: 1};", "let x = {1e21: 1};"] {
            let error = parse_program(source).unwrap_err();

            assert_eq!(
                error.kind,
                crate::javascript::JsErrorKind::UnsupportedFeature,
                "{source}"
            );
        }

        assert!(parse_program("let x = {1};").is_err());
    }

    #[test]
    fn parses_outer_iife_of_local_testharness() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        let program = parse_program(source).unwrap();

        let [Statement::Expression(Expression::Call { callee, arguments })] = program.as_slice()
        else {
            panic!("expected one top-level call");
        };

        let Expression::Function(function) = callee.as_ref() else {
            panic!("expected function expression");
        };

        assert_eq!(function.parameters.len(), 1);
        assert_eq!(function.parameters[0].name, "global_scope");
        assert!(function.body.len() > 1);

        assert!(matches!(
            arguments.as_slice(),
            [Expression::Identifier(name)] if name == "self"
        ));
    }

    #[test]
    fn parses_elisions_and_trailing_commas() {
        for (source, expected) in [
            ("[];", vec![]),
            ("[,];", vec![false]),
            ("[,,];", vec![false, false]),
            ("[1,];", vec![true]),
            ("[1,,];", vec![true, false]),
            ("[,1,,2,];", vec![false, true, false, true]),
        ] {
            let program = parse_program(source).unwrap();

            let Statement::Expression(Expression::Array(slots)) = &program[0] else {
                panic!("expected array literal: {source}");
            };

            let actual: Vec<bool> = slots.iter().map(Option::is_some).collect();

            assert_eq!(actual, expected, "{source}");
        }
    }
}
