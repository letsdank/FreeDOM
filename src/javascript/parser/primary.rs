//! Partial parser for primary expressions and literal containers.
//!
//! <https://tc39.es/ecma262/#sec-primary-expression>
//!
//! This module implements only the interpreter's existing grammar subset.

use super::{Expression, FunctionParameter, JsError, Parser, TokenKind, UserFunction, Value};

impl Parser<'_> {
    pub(super) fn parse_primary(&mut self) -> Result<Expression, JsError> {
        let token = self.cursor.advance_primary()?;

        match token.kind {
            TokenKind::String(value) => Ok(Expression::Literal(Value::String(value))),
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

        if self.cursor.consume_optional(&TokenKind::RightBracket)? {
            return Ok(Expression::Array(values));
        }

        loop {
            values.push(self.parse_expression()?);

            if self.cursor.consume_optional(&TokenKind::Comma)? {
                if self.cursor.consume_optional(&TokenKind::RightBracket)? {
                    return Ok(Expression::Array(values));
                }

                continue;
            }

            self.cursor.consume(
                &TokenKind::RightBracket,
                "expected ',' or ']' after array element",
            )?;

            return Ok(Expression::Array(values));
        }
    }

    fn parse_object_literal(&mut self) -> Result<Expression, JsError> {
        let mut properties = Vec::new();

        if self.cursor.consume_optional(&TokenKind::RightBrace)? {
            return Ok(Expression::Object(properties));
        }

        loop {
            let token = self.cursor.advance()?;

            let name = match token.kind {
                TokenKind::Identifier(name) | TokenKind::String(name) => name,
                _ => {
                    return Err(JsError::syntax(
                        token.offset,
                        "expected an object property name",
                    ));
                }
            };

            self.cursor
                .consume(&TokenKind::Colon, "expected ':' after object property name")?;

            let value = self.parse_expression()?;
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
}
