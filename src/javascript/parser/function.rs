//! Parsers for function expressions, function bodies, and argument lists.
//!
//! Implements a subset of ECMAScript function syntax:
//! <https://tc39.es/ecma262/#sec-function-definitions>

use super::{
    Expression, FunctionKind, FunctionParameter, JsError, Parser, Statement, TokenKind,
    UserFunction,
};

impl Parser<'_> {
    pub(super) fn parse_function_expression(&mut self) -> Result<Expression, JsError> {
        let previous = std::mem::replace(&mut self.await_allowed, false);

        let result = self.parse_ordinary_function_expression();

        self.await_allowed = previous;

        result
    }

    fn parse_ordinary_function_expression(&mut self) -> Result<Expression, JsError> {
        self.cursor
            .consume(&TokenKind::LeftParen, "expected '(' after function keyword")?;

        let parameters = self.parse_function_parameters()?;
        let body = self.parse_function_body()?;

        Ok(Expression::Function(UserFunction {
            kind: FunctionKind::Ordinary,
            parameters,
            body,
        }))
    }

    fn parse_function_parameters(&mut self) -> Result<Vec<FunctionParameter>, JsError> {
        let mut parameters = Vec::new();

        if self.cursor.consume_optional(&TokenKind::RightParen)? {
            return Ok(parameters);
        }

        loop {
            let is_rest = self.cursor.consume_optional(&TokenKind::Ellipsis)?;

            let (name, _) = self.consume_identifier("expected a function identifier")?;

            if is_rest {
                let mut names = std::collections::HashSet::new();

                let duplicate_existing = parameters
                    .iter()
                    .any(|parameter: &FunctionParameter| !names.insert(parameter.name.as_str()));

                if duplicate_existing || !names.insert(name.as_str()) {
                    return Err(JsError::syntax(
                        self.cursor.peek()?.offset,
                        "duplicate parameter in function with rest parameter",
                    ));
                }

                parameters.push(FunctionParameter {
                    name,
                    default: None,
                    is_rest: true,
                });

                self.cursor.consume(
                    &TokenKind::RightParen,
                    "rest parameter must be last and cannot have a default or trailing comma",
                )?;

                return Ok(parameters);
            }

            let default = if self.cursor.consume_optional(&TokenKind::Assign)? {
                Some(self.parse_expression()?)
            } else {
                None
            };

            parameters.push(FunctionParameter {
                name,
                default,
                is_rest: false,
            });

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
        self.parse_function_declaration_with_kind(FunctionKind::Ordinary)
    }

    pub(super) fn parse_async_function_declaration(&mut self) -> Result<Statement, JsError> {
        self.parse_function_declaration_with_kind(FunctionKind::Async)
    }

    fn parse_function_declaration_with_kind(
        &mut self,
        kind: FunctionKind,
    ) -> Result<Statement, JsError> {
        let previous = std::mem::replace(&mut self.await_allowed, false);

        let result = self.parse_function_declaration_inner(kind);

        self.await_allowed = previous;

        result
    }

    fn parse_function_declaration_inner(
        &mut self,
        kind: FunctionKind,
    ) -> Result<Statement, JsError> {
        let (name, _) = self.consume_identifier("expected a function name")?;

        self.cursor
            .consume(&TokenKind::LeftParen, "expected '(' after function name")?;

        let parameters = self.parse_function_parameters()?;

        if kind == FunctionKind::Async
            && parameters.iter().any(|parameter| parameter.name == "await")
        {
            return Err(JsError::syntax(
                self.cursor.peek()?.offset,
                "await cannot be an async function parameter",
            ));
        }

        self.await_allowed = kind == FunctionKind::Async;

        let body = self.parse_function_body()?;

        Ok(Statement::FunctionDeclaration {
            name,
            function: UserFunction {
                kind,
                parameters,
                body,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::parser::parse_program;
    use crate::javascript::{Expression, FunctionKind, Statement};

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

    #[test]
    fn parses_rest_parameter_from_testharness() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        let header = "function assert_wrapper(...args)";

        assert!(source.contains(header));

        let program = parse_program(&format!("{header} {{}}")).unwrap();

        let [Statement::FunctionDeclaration { function, .. }] = program.as_slice() else {
            panic!("expected function declaration");
        };

        assert_eq!(function.parameters.len(), 1);
        assert_eq!(function.parameters[0].name, "args");
        assert!(function.parameters[0].is_rest);
        assert!(function.parameters[0].default.is_none());
    }

    #[test]
    fn rejects_invalid_rest_parameters() {
        for source in [
            "function f(...x, y) {}",
            "function f(...x,) {}",
            "function f(...x = 1) {}",
            "function f(...) {}",
            "function f(x, ...x) {}",
            "function f(x, x, ...z) {}",
        ] {
            assert!(parse_program(source).is_err(), "{source}");
        }
    }

    #[test]
    fn parses_async_function_and_await_expressions_from_testharness() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        let start = source
            .find("async function fetch_json(resource) {")
            .expect("WPT async function must exist");

        let tail = &source[start..];

        let end = tail.find("\n    }").expect("WPT function end must exist") + "\n    }".len();

        let program = parse_program(&tail[..end]).unwrap();

        let [Statement::FunctionDeclaration { name, function }] = program.as_slice() else {
            panic!("expected function declaration");
        };

        assert_eq!(name, "fetch_json");
        assert_eq!(function.kind, FunctionKind::Async);

        assert!(matches!(
            &function.body[..],
            [
                Statement::Declaration {
                    name,
                    value: Expression::Await(first)
                },
                Statement::Return(Expression::Await(second))
            ] if name == "response"
                && matches!(first.as_ref(), Expression::Call { .. })
                && matches!(second.as_ref(), Expression::Call { .. })
        ));
    }

    #[test]
    fn async_is_contextual_and_line_break_disallows_async_declaration() {
        for gap in ["\n", "\r\n", "/*\n*/", "\u{2028}", "\u{2029}"] {
            let program = parse_program(&format!("async{gap}function f() {{}}")).unwrap();

            assert!(
                matches!(
                    &program[..],
                    [
                        Statement::Expression(Expression::Identifier(name)),
                        Statement::FunctionDeclaration { function, .. }
                    ] if name == "async"
                        && function.kind == FunctionKind::Ordinary
                ),
                "{gap:?}"
            );
        }

        for gap in [" ", "/* comment */"] {
            let program = parse_program(&format!("async{gap}function f() {{}}")).unwrap();

            assert!(
                matches!(
                    &program[..],
                    [Statement::FunctionDeclaration { function, .. }]
                        if function.kind == FunctionKind::Async
                ),
                "{gap:?}"
            );
        }
    }

    #[test]
    fn nested_ordinary_function_restores_await_context() {
        let program = parse_program(
            "async function f() { \
                function g() { \
                    let await = 1; \
                    return await; \
                } \
                return await task; \
            }",
        )
        .unwrap();

        let [Statement::FunctionDeclaration { function, .. }] = &program[..] else {
            panic!("expected declaration");
        };

        assert!(matches!(
            &function.body[..],
            [
                Statement::FunctionDeclaration {
                    function: nested,
                    ..
                },
                Statement::Return(Expression::Await(_))
            ] if nested.kind == FunctionKind::Ordinary
                && matches!(
                    &nested.body[..],
                    [
                        Statement::Declaration { name, .. },
                        Statement::Return(Expression::Identifier(id))
                    ] if name == "await" && id == "await"
                )
        ));
    }

    #[test]
    fn rejects_await_as_async_function_parameter() {
        assert!(parse_program("async function f(await) {}").is_err());
    }
}
