use super::{
    BinaryOperator, Expression, JsError, Statement, Token, TokenKind, UserFunction, Value, tokenize,
};

use super::token_cursor::TokenCursor;

#[path = "parser/statement.rs"]
mod statement;

pub(super) fn parse_program(source: &str) -> Result<Vec<Statement>, JsError> {
    let tokens = tokenize(source).map_err(|error| JsError {
        offset: Some(error.offset),
        message: error.message,
    })?;

    Parser::new(tokens).parse_program()
}

struct Parser {
    cursor: TokenCursor,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self {
            cursor: TokenCursor::new(tokens),
        }
    }

    fn parse_program(&mut self) -> Result<Vec<Statement>, JsError> {
        let mut statements = Vec::new();

        while !self.cursor.check(&TokenKind::EndOfFile) {
            statements.push(self.parse_statement()?);
        }

        Ok(statements)
    }

    fn parse_expression(&mut self) -> Result<Expression, JsError> {
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

    fn parse_primary(&mut self) -> Result<Expression, JsError> {
        let token = self.cursor.advance();

        match token.kind {
            TokenKind::String(value) => Ok(Expression::Literal(Value::String(value))),
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
                if self.cursor.consume_optional(&TokenKind::RightParen) {
                    if self.cursor.consume_optional(&TokenKind::Arrow) {
                        return Ok(Expression::Function(UserFunction {
                            parameters: Vec::new(),
                            body: self.parse_function_body()?,
                        }));
                    }

                    return Err(JsError::syntax(
                        token.offset,
                        "expected an expression inside parentheses",
                    ));
                }

                if let TokenKind::Identifier(parameter) = self.cursor.peek().kind.clone() {
                    let has_closing_parenthesis = self
                        .cursor
                        .peek_n(1)
                        .is_some_and(|token| matches!(token.kind, TokenKind::RightParen));

                    let has_arrow = self
                        .cursor
                        .peek_n(2)
                        .is_some_and(|token| matches!(token.kind, TokenKind::Arrow));

                    if has_closing_parenthesis && has_arrow {
                        self.cursor.advance();
                        self.cursor.advance();
                        self.cursor.advance();

                        return Ok(Expression::Function(UserFunction {
                            parameters: vec![parameter],
                            body: self.parse_function_body()?,
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

        if self.cursor.consume_optional(&TokenKind::RightBracket) {
            return Ok(Expression::Array(values));
        }

        loop {
            values.push(self.parse_expression()?);

            if self.cursor.consume_optional(&TokenKind::Comma) {
                if self.cursor.consume_optional(&TokenKind::RightBracket) {
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

        if self.cursor.consume_optional(&TokenKind::RightBrace) {
            return Ok(Expression::Object(properties));
        }

        loop {
            let token = self.cursor.advance();

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

            if self.cursor.consume_optional(&TokenKind::Comma) {
                if self.cursor.consume_optional(&TokenKind::RightBrace) {
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

    fn parse_function_expression(&mut self) -> Result<Expression, JsError> {
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

    fn parse_function_body(&mut self) -> Result<Vec<Statement>, JsError> {
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

    fn parse_new_expression(&mut self, offset: usize) -> Result<Expression, JsError> {
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

    fn parse_arguments(&mut self) -> Result<Vec<Expression>, JsError> {
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

    fn consume_identifier(&mut self, message: &str) -> Result<(String, usize), JsError> {
        let token = self.cursor.advance();

        match token.kind {
            TokenKind::Identifier(name) => Ok((name, token.offset)),
            _ => Err(JsError::syntax(token.offset, message)),
        }
    }

    fn consume_contextual_keyword(&mut self, keyword: &str) -> bool {
        match &self.cursor.peek().kind {
            TokenKind::Identifier(identifier) if identifier == keyword => {
                self.cursor.advance();
                true
            }
            _ => false,
        }
    }
}
