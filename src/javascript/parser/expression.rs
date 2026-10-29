//! Partial parser for JavaScript expressions and operator precedence.
//!
//! <https://tc39.es/ecma262/#sec-ecmascript-language-expressions>
//!
//! This module preserves the interpreter's existing grammar subset.

use super::{BinaryOperator, Expression, JsError, Parser, TokenKind, UserFunction};

impl Parser {
    pub(super) fn parse_expression(&mut self) -> Result<Expression, JsError> {
        self.parse_assignment_expression()
    }

    fn parse_conditional_expression(&mut self) -> Result<Expression, JsError> {
        let condition = self.parse_logical_or_expression()?;

        if !self.cursor.consume_optional(&TokenKind::Question) {
            return Ok(condition);
        }

        let consequent = self.parse_assignment_expression()?;

        self.cursor.consume(
            &TokenKind::Colon,
            "expected ':' after conditional expression",
        )?;

        let alternate = self.parse_assignment_expression()?;

        Ok(Expression::Conditional {
            condition: Box::new(condition),
            consequent: Box::new(consequent),
            alternate: Box::new(alternate),
        })
    }

    fn parse_assignment_expression(&mut self) -> Result<Expression, JsError> {
        if let TokenKind::Identifier(parameter) = self.cursor.peek().kind.clone() {
            let is_arrow = self
                .cursor
                .peek_n(1)
                .is_some_and(|token| matches!(token.kind, TokenKind::Arrow));

            if is_arrow {
                self.cursor.advance();
                self.cursor.advance();

                return Ok(Expression::Function(UserFunction {
                    parameters: vec![parameter],
                    body: self.parse_function_body()?,
                }));
            }
        }

        let expression = self.parse_conditional_expression()?;

        if self.cursor.consume_optional(&TokenKind::Assign) {
            let value = self.parse_assignment_expression()?;

            return Ok(Expression::Assignment {
                target: Box::new(expression),
                value: Box::new(value),
            });
        }

        Ok(expression)
    }

    fn parse_logical_or_expression(&mut self) -> Result<Expression, JsError> {
        let mut expression = self.parse_logical_and_expression()?;

        while self.cursor.consume_optional(&TokenKind::Or) {
            let right = self.parse_logical_and_expression()?;

            expression = Expression::Binary {
                left: Box::new(expression),
                operator: BinaryOperator::Or,
                right: Box::new(right),
            };
        }

        Ok(expression)
    }

    fn parse_logical_and_expression(&mut self) -> Result<Expression, JsError> {
        let mut expression = self.parse_equality_expression()?;

        while self.cursor.consume_optional(&TokenKind::And) {
            let right = self.parse_equality_expression()?;

            expression = Expression::Binary {
                left: Box::new(expression),
                operator: BinaryOperator::And,
                right: Box::new(right),
            };
        }

        Ok(expression)
    }

    fn parse_equality_expression(&mut self) -> Result<Expression, JsError> {
        let mut expression = self.parse_comparison_expression()?;

        loop {
            let operator = if self.cursor.consume_optional(&TokenKind::StrictEqual) {
                BinaryOperator::StrictEqual
            } else if self.cursor.consume_optional(&TokenKind::StrictNotEqual) {
                BinaryOperator::StrictNotEqual
            } else if self.cursor.consume_optional(&TokenKind::Equal) {
                BinaryOperator::Equal
            } else if self.cursor.consume_optional(&TokenKind::NotEqual) {
                BinaryOperator::NotEqual
            } else {
                break;
            };

            let right = self.parse_comparison_expression()?;

            expression = Expression::Binary {
                left: Box::new(expression),
                operator,
                right: Box::new(right),
            };
        }

        Ok(expression)
    }

    fn parse_comparison_expression(&mut self) -> Result<Expression, JsError> {
        let mut expression = self.parse_term_expression()?;

        loop {
            let operator = if self.cursor.consume_optional(&TokenKind::LessEqual) {
                BinaryOperator::LessEqual
            } else if self.cursor.consume_optional(&TokenKind::GreaterEqual) {
                BinaryOperator::GreaterEqual
            } else if self.cursor.consume_optional(&TokenKind::Instanceof) {
                BinaryOperator::Instanceof
            } else if self.cursor.consume_optional(&TokenKind::Less) {
                BinaryOperator::Less
            } else if self.cursor.consume_optional(&TokenKind::Greater) {
                BinaryOperator::Greater
            } else {
                break;
            };

            let right = self.parse_term_expression()?;

            expression = Expression::Binary {
                left: Box::new(expression),
                operator,
                right: Box::new(right),
            };
        }

        Ok(expression)
    }

    fn parse_term_expression(&mut self) -> Result<Expression, JsError> {
        let mut expression = self.parse_factor_expression()?;

        loop {
            let operator = if self.cursor.consume_optional(&TokenKind::Plus) {
                BinaryOperator::Add
            } else if self.cursor.consume_optional(&TokenKind::Minus) {
                BinaryOperator::Subtract
            } else {
                break;
            };

            let right = self.parse_factor_expression()?;

            expression = Expression::Binary {
                left: Box::new(expression),
                operator,
                right: Box::new(right),
            };
        }

        Ok(expression)
    }

    fn parse_factor_expression(&mut self) -> Result<Expression, JsError> {
        let mut expression = self.parse_unary_expression()?;

        loop {
            let operator = if self.cursor.consume_optional(&TokenKind::Star) {
                BinaryOperator::Multiply
            } else if self.cursor.consume_optional(&TokenKind::Slash) {
                BinaryOperator::Divide
            } else {
                break;
            };

            let right = self.parse_unary_expression()?;

            expression = Expression::Binary {
                left: Box::new(expression),
                operator,
                right: Box::new(right),
            };
        }

        Ok(expression)
    }

    fn parse_unary_expression(&mut self) -> Result<Expression, JsError> {
        if self.cursor.consume_optional(&TokenKind::PlusPlus) {
            return Ok(Expression::Update {
                target: Box::new(self.parse_unary_expression()?),
                delta: 1.0,
                prefix: true,
            });
        }

        if self.cursor.consume_optional(&TokenKind::MinusMinus) {
            return Ok(Expression::Update {
                target: Box::new(self.parse_unary_expression()?),
                delta: -1.0,
                prefix: true,
            });
        }

        if self.cursor.consume_optional(&TokenKind::Typeof) {
            return Ok(Expression::TypeOf(Box::new(self.parse_unary_expression()?)));
        }

        if self.cursor.consume_optional(&TokenKind::Not) {
            return Ok(Expression::Not(Box::new(self.parse_unary_expression()?)));
        }

        self.parse_postfix_expression()
    }

    fn parse_postfix_expression(&mut self) -> Result<Expression, JsError> {
        let mut expression = self.parse_primary()?;

        loop {
            if self.cursor.consume_optional(&TokenKind::Dot) {
                let (property, _) =
                    self.consume_identifier("expected a property name after '.'")?;

                expression = Expression::Member {
                    object: Box::new(expression),
                    property,
                };

                continue;
            }

            if self.cursor.consume_optional(&TokenKind::LeftBracket) {
                let index = self.parse_expression()?;

                self.cursor.consume(
                    &TokenKind::RightBracket,
                    "expected ']' after index expression",
                )?;

                expression = Expression::Index {
                    object: Box::new(expression),
                    index: Box::new(index),
                };

                continue;
            }

            if self.cursor.consume_optional(&TokenKind::LeftParen) {
                let arguments = self.parse_arguments()?;

                expression = Expression::Call {
                    callee: Box::new(expression),
                    arguments,
                };

                continue;
            }

            if self.cursor.consume_optional(&TokenKind::PlusPlus) {
                expression = Expression::Update {
                    target: Box::new(expression),
                    delta: 1.0,
                    prefix: false,
                };

                continue;
            }

            if self.cursor.consume_optional(&TokenKind::MinusMinus) {
                expression = Expression::Update {
                    target: Box::new(expression),
                    delta: -1.0,
                    prefix: false,
                };

                continue;
            }

            break;
        }

        Ok(expression)
    }

    pub(super) fn parse_new_expression(&mut self, offset: usize) -> Result<Expression, JsError> {
        let (constructor, _) = self.consume_identifier("expected a constructor after 'new'")?;

        self.cursor
            .consume(&TokenKind::LeftParen, "expected '(' after constructor name")?;

        let arguments = self.parse_arguments()?;

        if constructor.is_empty() {
            return Err(JsError::syntax(offset, "constructor name cannot be empty"));
        }

        Ok(Expression::New {
            constructor,
            arguments,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::parser::parse_program;
    use crate::javascript::{BinaryOperator, Expression, Statement};

    #[test]
    fn multiplication_has_higher_precedence_than_addition() {
        let program = parse_program("1 + 2 * 3;").unwrap();

        let [
            Statement::Expression(Expression::Binary {
                operator: BinaryOperator::Add,
                right,
                ..
            }),
        ] = program.as_slice()
        else {
            panic!("expected an addition expression");
        };

        assert!(matches!(
            right.as_ref(),
            Expression::Binary {
                operator: BinaryOperator::Multiply,
                ..
            }
        ));
    }

    #[test]
    fn assignment_is_right_associative() {
        let program = parse_program("a = b = 1;").unwrap();

        let [Statement::Expression(Expression::Assignment { target, value })] = program.as_slice()
        else {
            panic!("expected an assignment expression");
        };

        assert!(matches!(
            target.as_ref(),
            Expression::Identifier(name) if name == "a"
        ));

        assert!(matches!(
            value.as_ref(),
            Expression::Assignment { target, .. }
                if matches!(
                    target.as_ref(),
                    Expression::Identifier(name) if name == "b"
                )
        ));
    }

    #[test]
    fn parses_chained_member_index_and_call() {
        let program = parse_program("object.method(1)[0];").unwrap();

        let [Statement::Expression(Expression::Index { object, .. })] = program.as_slice() else {
            panic!("expected an indexed expression");
        };

        assert!(matches!(
            object.as_ref(),
            Expression::Call { callee, arguments }
                if arguments.len() == 1
                    && matches!(
                        callee.as_ref(),
                        Expression::Member{ property, .. } if property == "method"
                    )
        ));
    }
}
