//! Parsers for function expressions, function bodies, and argument lists.
//!
//! Implements a subset of ECMAScript function syntax:
//! <https://tc39.es/ecma262/#sec-function-definitions>

use super::{Expression, FunctionParameter, JsError, Parser, Statement, TokenKind, UserFunction};

impl Parser<'_> {
    pub(super) fn parse_function_expression(&mut self) -> Result<Expression, JsError> {
        self.cursor
            .consume(&TokenKind::LeftParen, "expected '(' after function keyword")?;

        let parameters = self.parse_function_parameters()?;
        let body = self.parse_function_body()?;

        Ok(Expression::Function(UserFunction { parameters, body }))
    }

    fn parse_function_parameters(&mut self) -> Result<Vec<FunctionParameter>, JsError> {
        let mut parameters = Vec::new();

        if self.cursor.consume_optional(&TokenKind::RightParen)? {
            return Ok(parameters);
        }

        loop {
            let (name, _) = self.consume_identifier("expected a function identifier")?;

            let default = if self.cursor.consume_optional(&TokenKind::Assign)? {
                Some(self.parse_expression()?)
            } else {
                None
            };

            parameters.push(FunctionParameter { name, default });

            if self.cursor.consume_optional(&TokenKind::Comma)? {
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

        while !self.cursor.check(&TokenKind::RightBrace)? {
            if self.cursor.check(&TokenKind::EndOfFile)? {
                return Err(JsError::syntax(
                    self.cursor.peek()?.offset,
                    "unterminated function body",
                ));
            }

            body.push(self.parse_statement()?);
        }

        self.cursor.advance()?;

        Ok(body)
    }

    pub(super) fn parse_arguments(&mut self) -> Result<Vec<Expression>, JsError> {
        let mut arguments = Vec::new();

        if self.cursor.consume_optional(&TokenKind::RightParen)? {
            return Ok(arguments);
        }

        loop {
            arguments.push(self.parse_expression()?);

            if self.cursor.consume_optional(&TokenKind::Comma)? {
                if self.cursor.consume_optional(&TokenKind::RightParen)? {
                    return Ok(arguments);
                }

                continue;
            }

            self.cursor
                .consume(&TokenKind::RightParen, "expected ',' or ')' after argument")?;

            return Ok(arguments);
        }
    }

    pub(super) fn parse_function_declaration(&mut self) -> Result<Statement, JsError> {
        let (name, _) = self.consume_identifier("expected a function name")?;

        self.cursor
            .consume(&TokenKind::LeftParen, "expected '(' after function name")?;

        let parameters = self.parse_function_parameters()?;
        let body = self.parse_function_body()?;

        Ok(Statement::FunctionDeclaration {
            name,
            function: UserFunction { parameters, body },
        })
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

        let names: Vec<_> = function
            .parameters
            .iter()
            .map(|parameter| parameter.name.as_str())
            .collect();

        assert_eq!(names, ["a", "b"]);
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

    #[test]
    fn parses_function_declaration_header_from_wpt_harness() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        let header = "function WindowTestEnvironment()";

        assert!(source.contains(header));

        let program = parse_program(&format!("{header} {{}}")).unwrap();

        assert!(matches!(
            &program[0],
            Statement::FunctionDeclaration { name, function }
                if name == "WindowTestEnvironment" && function.parameters.is_empty()
        ));
    }

    #[test]
    fn rejects_function_declaration_without_name() {
        let error = parse_program("function() {}").unwrap_err();

        assert_eq!(error.message, "expected a function name");
    }

    #[test]
    fn parses_default_parameter_from_testharness() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        let header = "function promise_setup(func, properties={})";

        assert!(source.contains(header));

        let program = parse_program(&format!("{header} {{}}")).unwrap();

        let [Statement::FunctionDeclaration { function, .. }] = program.as_slice() else {
            panic!("expected function");
        };

        assert_eq!(function.parameters.len(), 2);
        assert_eq!(function.parameters[1].name, "properties");

        assert!(matches!(
            function.parameters[1].default,
            Some(Expression::Object(ref entries)) if entries.is_empty()
        ));
    }
}
