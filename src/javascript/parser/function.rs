//! Parsers for function expressions, function bodies, and argument lists.
//!
//! Implements a subset of ECMAScript function syntax:
//! <https://tc39.es/ecma262/#sec-function-definitions>

use super::{Expression, JsError, Parser, Statement, TokenKind, UserFunction};

impl Parser {
    pub(super) fn parse_function_expression(&mut self) -> Result<Expression, JsError> {
        self.cursor
            .consume(&TokenKind::LeftParen, "expected '(' after function keyword")?;

        let parameters = self.parse_function_parameters()?;
        let body = self.parse_function_body()?;

        Ok(Expression::Function(UserFunction { parameters, body }))
    }

    fn parse_function_parameters(&mut self) -> Result<Vec<String>, JsError> {
        let mut parameters = Vec::new();

        if self.cursor.consume_optional(&TokenKind::RightParen) {
            return Ok(parameters);
        }

        loop {
            let (parameter, _) = self.consume_identifier("expected a function identifier")?;

            parameters.push(parameter);

            if self.cursor.consume_optional(&TokenKind::Comma) {
                continue;
            }

            self.cursor.consume(
                &TokenKind::RightParen,
                "expected ',' or ')' after function parameter",
            )?;

            return Ok(parameters);
        }
    }

    pub(super) fn parse_function_body(&mut self) -> Result<Vec<Statement>, JsError> {
        self.cursor
            .consume(&TokenKind::LeftBrace, "expected '{' before function body")?;

        let mut body = Vec::new();

        while !self.cursor.check(&TokenKind::RightBrace) {
            if self.cursor.check(&TokenKind::EndOfFile) {
                return Err(JsError::syntax(
                    self.cursor.peek().offset,
                    "unterminated function body",
                ));
            }

            body.push(self.parse_statement()?);
        }

        self.cursor.advance();

        Ok(body)
    }

    pub(super) fn parse_arguments(&mut self) -> Result<Vec<Expression>, JsError> {
        let mut arguments = Vec::new();

        if self.cursor.consume_optional(&TokenKind::RightParen) {
            return Ok(arguments);
        }

        loop {
            arguments.push(self.parse_expression()?);

            if self.cursor.consume_optional(&TokenKind::Comma) {
                if self.cursor.consume_optional(&TokenKind::RightParen) {
                    return Ok(arguments);
                }

                continue;
            }

            self.cursor
                .consume(&TokenKind::RightParen, "expected ',' or ')' after argument")?;

            return Ok(arguments);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::parser::parse_program;
    use crate::javascript::{Expression, Statement};

    #[test]
    fn parses_function_parameters_and_body() {
        let program = parse_program("const f = function(a, b) { return a; };").unwrap();

        let Statement::Declaration {
            value: Expression::Function(function),
            ..
        } = &program[0]
        else {
            panic!("expected function declaration");
        };

        assert_eq!(function.parameters, ["a", "b"]);
        assert!(matches!(
            function.body.as_slice(),
            [Statement::Return(Expression::Identifier(name))] if name == "a"
        ));
    }

    #[test]
    fn reports_unterminated_function_body() {
        let source = "const f = function() {";
        let error = parse_program(source).unwrap_err();

        assert_eq!(error.offset, Some(source.len()));
        assert_eq!(error.message, "unterminated function body");
    }

    #[test]
    fn parses_constructor_and_call_arguments() {
        let program = parse_program(
            "const value = new URL(\"https://example.org\", \
            \"https://example.org/\"); value.method(1, 2);",
        )
        .unwrap();

        assert!(matches!(
            &program[0],
            Statement::Declaration {
                value:Expression::New {
                    constructor,
                    arguments
                },
                ..
            } if constructor == "URL" && arguments.len() == 2
        ));

        assert!(matches!(
            &program[1],
            Statement::Expression(Expression::Call { arguments, .. })
                if arguments.len() == 2
        ));
    }
}
