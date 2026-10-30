//! Partial parser for JavaScript statements and declarations.
//!
//! <https://tc39.es/ecma262/#sec-ecmascript-language-statements-and-declarations>
//!
//! This module preserves the existing subset and is not a complete grammar.

use super::{Expression, JsError, Parser, Statement, TokenKind, Value};
use crate::javascript::ast::CatchClause;

impl Parser<'_> {
    pub(super) fn parse_statement(&mut self) -> Result<Statement, JsError> {
        if self.consume_contextual_keyword("try")? {
            return self.parse_try_statement();
        }

        if self.cursor.consume_optional(&TokenKind::Function)? {
            return self.parse_function_declaration();
        }

        if self.cursor.consume_optional(&TokenKind::For)? {
            return self.parse_for_statement();
        }

        if self.cursor.consume_optional(&TokenKind::Break)? {
            self.cursor.consume_optional(&TokenKind::Semicolon)?;
            return Ok(Statement::Break);
        }

        if self.cursor.consume_optional(&TokenKind::Continue)? {
            self.cursor.consume_optional(&TokenKind::Semicolon)?;
            return Ok(Statement::Continue);
        }

        if self.cursor.consume_optional(&TokenKind::If)? {
            return self.parse_if_statement();
        }

        if self.cursor.consume_optional(&TokenKind::While)? {
            return self.parse_while_statement();
        }

        if self.cursor.consume_optional(&TokenKind::Return)? {
            let value = self.parse_expression()?;
            self.cursor.consume_optional(&TokenKind::Semicolon)?;

            return Ok(Statement::Return(value));
        }

        if matches!(
            self.cursor.peek()?.kind,
            TokenKind::Let | TokenKind::Const | TokenKind::Var
        ) {
            self.cursor.advance()?;
            return self.parse_declaration();
        }

        let expression = self.parse_expression()?;
        self.cursor.consume_optional(&TokenKind::Semicolon)?;

        Ok(Statement::Expression(expression))
    }

    fn parse_for_statement(&mut self) -> Result<Statement, JsError> {
        self.cursor
            .consume(&TokenKind::LeftParen, "expected '(' after for")?;

        if matches!(
            self.cursor.peek()?.kind,
            TokenKind::Let | TokenKind::Const | TokenKind::Var
        ) {
            self.cursor.advance()?;

            let (name, offset) = self.consume_identifier("expected a variable name")?;

            if self.consume_contextual_keyword("of")? {
                if name.is_empty() {
                    return Err(JsError::syntax(offset, "variable name cannot be empty"));
                }

                let iterable = self.parse_expression()?;

                self.cursor
                    .consume(&TokenKind::RightParen, "expected ')' after for-of iterable")?;

                let body = self.parse_statement_body()?;

                return Ok(Statement::ForOf {
                    binding: name,
                    iterable,
                    body,
                });
            }

            let value = if self.cursor.consume_optional(&TokenKind::Assign)? {
                self.parse_expression()?
            } else {
                Expression::Literal(Value::Undefined)
            };

            self.cursor
                .consume(&TokenKind::Semicolon, "expected ';' after for declaration")?;

            let initializer = Some(Box::new(Statement::Declaration { name, value }));

            let condition = if self.cursor.consume_optional(&TokenKind::Semicolon)? {
                None
            } else {
                let condition = self.parse_expression()?;

                self.cursor
                    .consume(&TokenKind::Semicolon, "expected ';' after for condition")?;

                Some(condition)
            };

            let update = if self.cursor.check(&TokenKind::RightParen)? {
                None
            } else {
                Some(self.parse_expression()?)
            };

            self.cursor
                .consume(&TokenKind::RightParen, "expected ')' after for clauses")?;

            let body = self.parse_statement_body()?;

            return Ok(Statement::For {
                initializer,
                condition,
                update,
                body,
            });
        }

        let initializer = if self.cursor.consume_optional(&TokenKind::Semicolon)? {
            None
        } else {
            let expression = self.parse_expression()?;

            self.cursor
                .consume(&TokenKind::Semicolon, "expected ';' after for initializer")?;

            Some(Box::new(Statement::Expression(expression)))
        };

        let condition = if self.cursor.consume_optional(&TokenKind::Semicolon)? {
            None
        } else {
            let condition = self.parse_expression()?;

            self.cursor
                .consume(&TokenKind::Semicolon, "expected ';' after for condition")?;

            Some(condition)
        };

        let update = if self.cursor.check(&TokenKind::RightParen)? {
            None
        } else {
            Some(self.parse_expression()?)
        };

        self.cursor
            .consume(&TokenKind::RightParen, "expected ')' after for clauses")?;

        let body = self.parse_statement_body()?;

        Ok(Statement::For {
            initializer,
            condition,
            update,
            body,
        })
    }

    fn parse_while_statement(&mut self) -> Result<Statement, JsError> {
        self.cursor
            .consume(&TokenKind::LeftParen, "expected '(' after while")?;

        let condition = self.parse_expression()?;

        self.cursor
            .consume(&TokenKind::RightParen, "expected ')' after while condition")?;

        let body = self.parse_statement_body()?;

        Ok(Statement::While { condition, body })
    }

    fn parse_if_statement(&mut self) -> Result<Statement, JsError> {
        self.cursor
            .consume(&TokenKind::LeftParen, "expected '(' after if")?;

        let condition = self.parse_expression()?;

        self.cursor
            .consume(&TokenKind::RightParen, "expected ')' after if condition")?;

        let then_body = self.parse_statement_body()?;

        let else_body = if self.cursor.consume_optional(&TokenKind::Else)? {
            self.parse_statement_body()?
        } else {
            Vec::new()
        };

        Ok(Statement::If {
            condition,
            then_body,
            else_body,
        })
    }

    fn parse_statement_body(&mut self) -> Result<Vec<Statement>, JsError> {
        if !self.cursor.consume_optional(&TokenKind::LeftBrace)? {
            return Ok(vec![self.parse_statement()?]);
        }

        let mut body = Vec::new();

        while !self.cursor.check(&TokenKind::RightBrace)? {
            if self.cursor.check(&TokenKind::EndOfFile)? {
                return Err(JsError::syntax(
                    self.cursor.peek()?.offset,
                    "unterminated statement block",
                ));
            }

            body.push(self.parse_statement()?);
        }

        self.cursor.advance()?;

        Ok(body)
    }

    fn parse_declaration(&mut self) -> Result<Statement, JsError> {
        let (name, offset) = self.consume_identifier("expected a variable name")?;

        let value = if self.cursor.consume_optional(&TokenKind::Assign)? {
            self.parse_expression()?
        } else {
            Expression::Literal(Value::Undefined)
        };

        self.cursor.consume_optional(&TokenKind::Semicolon)?;

        if name.is_empty() {
            return Err(JsError::syntax(offset, "variable name cannot be empty"));
        }

        Ok(Statement::Declaration { name, value })
    }

    fn parse_try_statement(&mut self) -> Result<Statement, JsError> {
        let body = self.parse_required_block("expected '{' after try")?;

        let catch = if self.consume_contextual_keyword("catch")? {
            let binding = if self.cursor.consume_optional(&TokenKind::LeftParen)? {
                let (name, _) = self.consume_identifier("expected a catch binding")?;

                self.cursor
                    .consume(&TokenKind::RightParen, "expected ')' after catch binding")?;

                Some(name)
            } else {
                None
            };

            let body = self.parse_required_block("expected '{' after catch")?;

            Some(CatchClause { binding, body })
        } else {
            None
        };

        let finally = if self.consume_contextual_keyword("finally")? {
            Some(self.parse_required_block("expected '{' after finally")?)
        } else {
            None
        };

        if catch.is_none() && finally.is_none() {
            return Err(JsError::syntax(
                self.cursor.peek()?.offset,
                "expected catch or finally after try block",
            ));
        }

        Ok(Statement::Try {
            body,
            catch,
            finally,
        })
    }

    fn parse_required_block(&mut self, message: &str) -> Result<Vec<Statement>, JsError> {
        if !self.cursor.check(&&TokenKind::LeftBrace)? {
            return Err(JsError::syntax(self.cursor.peek()?.offset, message));
        }

        self.parse_statement_body()
    }
}

#[cfg(test)]
mod tests {
    use super::Statement;
    use crate::javascript::parser::parse_program;

    #[test]
    fn parses_control_flow_and_for_or() {
        let program = parse_program(
            "while (true) { break; } \
            for (let item of [1]) { continue; }",
        )
        .unwrap();

        assert!(matches!(
            &program[0],
            Statement::While { body, .. }
            if matches!(
                body.as_slice(),
                [Statement::Break]
            )
        ));

        assert!(matches!(
            &program[1],
            Statement::ForOf {
                binding,
                body,
                ..
            } if binding == "item"
            && matches!(
                body.as_slice(),
                [Statement::Continue]
            )
        ));
    }

    #[test]
    fn reports_unterminated_statement_block_at_eof() {
        let input = "if (true) {";
        let error = parse_program(input).unwrap_err();

        assert_eq!(error.offset, Some(input.len()));
        assert_eq!(error.message, "unterminated statement block",);
    }

    #[test]
    fn parses_try_catch_finally() {
        let program = parse_program("try { 1; } catch (error) { 2; } finally { 3; }").unwrap();

        assert!(matches!(
            &program[0],
            Statement::Try {
                body,
                catch: Some(catch),
                finally: Some(finally),
            } if body.len() == 1
                && catch.binding.as_deref() == Some("error")
                && catch.body.len() == 1
                && finally.len() == 1
        ));
    }

    #[test]
    fn parses_optional_catch_binding_and_finally_only() {
        let program = parse_program("try {} catch {} try {} finally {}").unwrap();

        assert!(matches!(
            &program[0],
            Statement::Try {
                catch: Some(catch),
                finally: None,
                ..
            } if catch.binding.is_none()
        ));

        assert!(matches!(
            &program[1],
            Statement::Try {
                catch: None,
                finally: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn rejects_try_without_handler_or_required_blocks() {
        for source in [
            "try {}",
            "try 1; catch {}",
            "try {} catch (e) 1;",
            "try {} finally 1;",
        ] {
            assert!(parse_program(source).is_err(), "source: {source}");
        }
    }

    #[test]
    fn parses_try_catch_header_found_in_wpt_harness() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        assert!(source.contains("try {"));
        assert!(source.contains("} catch(e) {"));

        let program = parse_program("try {} catch (e) {}").unwrap();

        assert!(matches!(
            &program[0],
            Statement::Try {
                catch: Some(catch),
                ..
            } if catch.binding.as_deref() == Some("e")
        ));
    }
}
