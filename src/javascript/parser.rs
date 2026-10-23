use std::collections::HashMap;

use super::{
    BinaryOperator, Expression, JsError, Statement, Token, TokenKind, UserFunction, Value, tokenize,
};

pub(super) fn parse_program(source: &str) -> Result<Vec<Statement>, JsError> {
    let tokens = tokenize(source).map_err(|error| JsError {
        offset: Some(error.offset),
        message: error.message,
    })?;

    Parser::new(tokens).parse_program()
}

pub fn parse_json(source: &str) -> Result<Value, JsError> {
    let tokens = tokenize(source).map_err(|error| JsError {
        offset: Some(error.offset),
        message: error.message,
    })?;

    JsonParser::new(tokens).parse()
}

struct JsonParser {
    tokens: Vec<Token>,
    position: usize,
}

impl JsonParser {
    fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            position: 0,
        }
    }

    fn parse(&mut self) -> Result<Value, JsError> {
        let value = self.parse_value()?;

        if !self.check(&TokenKind::EndOfFile) {
            return Err(JsError::syntax(
                self.peek().offset,
                "unexpected token after JSON value",
            ));
        }

        Ok(value)
    }

    fn parse_value(&mut self) -> Result<Value, JsError> {
        let token = self.advance();

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

        if self.consume_optional(&TokenKind::RightBracket) {
            return Ok(Value::Array(values));
        }

        loop {
            values.push(self.parse_value()?);

            if self.consume_optional(&TokenKind::Comma) {
                if self.check(&TokenKind::RightBracket) {
                    return Err(JsError::syntax(
                        self.peek().offset,
                        "trailing comma is not valid JSON",
                    ));
                }

                continue;
            }

            self.consume(
                &TokenKind::RightBracket,
                "expected ',' or ']' after JSON array item",
            )?;

            return Ok(Value::Array(values));
        }
    }

    fn parse_object(&mut self) -> Result<Value, JsError> {
        let mut values = HashMap::new();

        if self.consume_optional(&TokenKind::RightBrace) {
            return Ok(Value::Object(values));
        }

        loop {
            let token = self.advance();

            let TokenKind::String(key) = token.kind else {
                return Err(JsError::syntax(
                    token.offset,
                    "JSON object keys must be strings",
                ));
            };

            self.consume(&TokenKind::Colon, "expected ':' after JSON object key")?;

            let value = self.parse_value()?;
            values.insert(key, value);

            if self.consume_optional(&TokenKind::Comma) {
                if self.check(&TokenKind::RightBrace) {
                    return Err(JsError::syntax(
                        self.peek().offset,
                        "trailing comma is not valid JSON",
                    ));
                }

                continue;
            }

            self.consume(
                &TokenKind::RightBrace,
                "expected ',' or '}' after JSON object property",
            )?;

            return Ok(Value::Object(values));
        }
    }

    fn consume(&mut self, expected: &TokenKind, message: &str) -> Result<(), JsError> {
        if self.check(expected) {
            self.advance();
            Ok(())
        } else {
            Err(JsError::syntax(self.peek().offset, message))
        }
    }

    fn consume_optional(&mut self, expected: &TokenKind) -> bool {
        if self.check(expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn check(&self, expected: &TokenKind) -> bool {
        self.peek().kind == *expected
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.position]
    }

    fn advance(&mut self) -> Token {
        let token = self.tokens[self.position].clone();

        if !matches!(token.kind, TokenKind::EndOfFile) {
            self.position += 1;
        }

        token
    }
}

struct Parser {
    tokens: Vec<Token>,
    position: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            position: 0,
        }
    }

    fn parse_program(&mut self) -> Result<Vec<Statement>, JsError> {
        let mut statements = Vec::new();

        while !self.check(&TokenKind::EndOfFile) {
            statements.push(self.parse_statement()?);
        }

        Ok(statements)
    }

    fn parse_statement(&mut self) -> Result<Statement, JsError> {
        if self.consume_optional(&TokenKind::For) {
            return self.parse_for_statement();
        }

        if self.consume_optional(&TokenKind::Break) {
            self.consume_optional(&TokenKind::Semicolon);
            return Ok(Statement::Break);
        }

        if self.consume_optional(&TokenKind::Continue) {
            self.consume_optional(&TokenKind::Semicolon);
            return Ok(Statement::Continue);
        }

        if self.consume_optional(&TokenKind::If) {
            return self.parse_if_statement();
        }

        if self.consume_optional(&TokenKind::While) {
            return self.parse_while_statement();
        }

        if self.consume_optional(&TokenKind::Return) {
            let value = self.parse_expression()?;
            self.consume_optional(&TokenKind::Semicolon);

            return Ok(Statement::Return(value));
        }

        if matches!(
            self.peek().kind,
            TokenKind::Let | TokenKind::Const | TokenKind::Var
        ) {
            self.advance();
            return self.parse_declaration();
        }

        let expression = self.parse_expression()?;
        self.consume_optional(&TokenKind::Semicolon);

        Ok(Statement::Expression(expression))
    }

    fn parse_for_statement(&mut self) -> Result<Statement, JsError> {
        self.consume(&TokenKind::LeftParen, "expected '(' after for")?;

        if matches!(
            self.peek().kind,
            TokenKind::Let | TokenKind::Const | TokenKind::Var
        ) {
            self.advance();

            let (name, offset) = self.consume_identifier("expected a variable name")?;

            if self.consume_contextual_keyword("of") {
                if name.is_empty() {
                    return Err(JsError::syntax(offset, "variable name cannot be empty"));
                }

                let iterable = self.parse_expression()?;

                self.consume(&TokenKind::RightParen, "expected ')' after for-of iterable")?;

                let body = self.parse_statement_body()?;

                return Ok(Statement::ForOf {
                    binding: name,
                    iterable,
                    body,
                });
            }

            let value = if self.consume_optional(&TokenKind::Assign) {
                self.parse_expression()?
            } else {
                Expression::Literal(Value::Undefined)
            };

            self.consume(&TokenKind::Semicolon, "expected ';' after for declaration")?;

            let initializer = Some(Box::new(Statement::Declaration { name, value }));

            let condition = if self.consume_optional(&TokenKind::Semicolon) {
                None
            } else {
                let condition = self.parse_expression()?;

                self.consume(&TokenKind::Semicolon, "expected ';' after for condition")?;

                Some(condition)
            };

            let update = if self.check(&TokenKind::RightParen) {
                None
            } else {
                Some(self.parse_expression()?)
            };

            self.consume(&TokenKind::RightParen, "expected ')' after for clauses")?;

            let body = self.parse_statement_body()?;

            return Ok(Statement::For {
                initializer,
                condition,
                update,
                body,
            });
        }

        let initializer = if self.consume_optional(&TokenKind::Semicolon) {
            None
        } else {
            let expression = self.parse_expression()?;

            self.consume(&TokenKind::Semicolon, "expected ';' after for initializer")?;

            Some(Box::new(Statement::Expression(expression)))
        };

        let condition = if self.consume_optional(&TokenKind::Semicolon) {
            None
        } else {
            let condition = self.parse_expression()?;

            self.consume(&TokenKind::Semicolon, "expected ';' after for condition")?;

            Some(condition)
        };

        let update = if self.check(&TokenKind::RightParen) {
            None
        } else {
            Some(self.parse_expression()?)
        };

        self.consume(&TokenKind::RightParen, "expected ')' after for clauses")?;

        let body = self.parse_statement_body()?;

        Ok(Statement::For {
            initializer,
            condition,
            update,
            body,
        })
    }

    fn parse_while_statement(&mut self) -> Result<Statement, JsError> {
        self.consume(&TokenKind::LeftParen, "expected '(' after while")?;

        let condition = self.parse_expression()?;

        self.consume(&TokenKind::RightParen, "expected ')' after while condition")?;

        let body = self.parse_statement_body()?;

        Ok(Statement::While { condition, body })
    }

    fn parse_if_statement(&mut self) -> Result<Statement, JsError> {
        self.consume(&TokenKind::LeftParen, "expected '(' after if")?;

        let condition = self.parse_expression()?;

        self.consume(&TokenKind::RightParen, "expected ')' after if condition")?;

        let then_body = self.parse_statement_body()?;

        let else_body = if self.consume_optional(&TokenKind::Else) {
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
        if !self.consume_optional(&TokenKind::LeftBrace) {
            return Ok(vec![self.parse_statement()?]);
        }

        let mut body = Vec::new();

        while !self.check(&TokenKind::RightBrace) {
            if self.check(&TokenKind::EndOfFile) {
                return Err(JsError::syntax(
                    self.peek().offset,
                    "unterminated statement block",
                ));
            }

            body.push(self.parse_statement()?);
        }

        self.advance();

        Ok(body)
    }

    fn parse_declaration(&mut self) -> Result<Statement, JsError> {
        let (name, offset) = self.consume_identifier("expected a variable name")?;

        let value = if self.consume_optional(&TokenKind::Assign) {
            self.parse_expression()?
        } else {
            Expression::Literal(Value::Undefined)
        };

        self.consume_optional(&TokenKind::Semicolon);

        if name.is_empty() {
            return Err(JsError::syntax(offset, "variable name cannot be empty"));
        }

        Ok(Statement::Declaration { name, value })
    }

    fn parse_expression(&mut self) -> Result<Expression, JsError> {
        self.parse_assignment_expression()
    }

    fn parse_assignment_expression(&mut self) -> Result<Expression, JsError> {
        if let TokenKind::Identifier(parameter) = self.peek().kind.clone() {
            let is_arrow = self
                .tokens
                .get(self.position + 1)
                .is_some_and(|token| matches!(token.kind, TokenKind::Arrow));

            if is_arrow {
                self.advance();
                self.advance();

                return Ok(Expression::Function(UserFunction {
                    parameters: vec![parameter],
                    body: self.parse_function_body()?,
                }));
            }
        }

        let expression = self.parse_logical_or_expression()?;

        if self.consume_optional(&TokenKind::Assign) {
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

        while self.consume_optional(&TokenKind::Or) {
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

        while self.consume_optional(&TokenKind::And) {
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
            let operator = if self.consume_optional(&TokenKind::StrictEqual) {
                BinaryOperator::StrictEqual
            } else if self.consume_optional(&TokenKind::StrictNotEqual) {
                BinaryOperator::StrictNotEqual
            } else if self.consume_optional(&TokenKind::Equal) {
                BinaryOperator::Equal
            } else if self.consume_optional(&TokenKind::NotEqual) {
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
            let operator = if self.consume_optional(&TokenKind::LessEqual) {
                BinaryOperator::LessEqual
            } else if self.consume_optional(&TokenKind::GreaterEqual) {
                BinaryOperator::GreaterEqual
            } else if self.consume_optional(&TokenKind::Instanceof) {
                BinaryOperator::Instanceof
            } else if self.consume_optional(&TokenKind::Less) {
                BinaryOperator::Less
            } else if self.consume_optional(&TokenKind::Greater) {
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
            let operator = if self.consume_optional(&TokenKind::Plus) {
                BinaryOperator::Add
            } else if self.consume_optional(&TokenKind::Minus) {
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
            let operator = if self.consume_optional(&TokenKind::Star) {
                BinaryOperator::Multiply
            } else if self.consume_optional(&TokenKind::Slash) {
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
        if self.consume_optional(&TokenKind::PlusPlus) {
            return Ok(Expression::Update {
                target: Box::new(self.parse_unary_expression()?),
                delta: 1.0,
                prefix: true,
            });
        }

        if self.consume_optional(&TokenKind::MinusMinus) {
            return Ok(Expression::Update {
                target: Box::new(self.parse_unary_expression()?),
                delta: -1.0,
                prefix: true,
            });
        }

        if self.consume_optional(&TokenKind::Not) {
            return Ok(Expression::Not(Box::new(self.parse_unary_expression()?)));
        }

        self.parse_postfix_expression()
    }

    fn parse_postfix_expression(&mut self) -> Result<Expression, JsError> {
        let mut expression = self.parse_primary()?;

        loop {
            if self.consume_optional(&TokenKind::Dot) {
                let (property, _) =
                    self.consume_identifier("expected a property name after '.'")?;

                expression = Expression::Member {
                    object: Box::new(expression),
                    property,
                };

                continue;
            }

            if self.consume_optional(&TokenKind::LeftBracket) {
                let index = self.parse_expression()?;

                self.consume(
                    &TokenKind::RightBracket,
                    "expected ']' after index expression",
                )?;

                expression = Expression::Index {
                    object: Box::new(expression),
                    index: Box::new(index),
                };

                continue;
            }

            if self.consume_optional(&TokenKind::LeftParen) {
                let arguments = self.parse_arguments()?;

                expression = Expression::Call {
                    callee: Box::new(expression),
                    arguments,
                };

                continue;
            }

            if self.consume_optional(&TokenKind::PlusPlus) {
                expression = Expression::Update {
                    target: Box::new(expression),
                    delta: 1.0,
                    prefix: false,
                };

                continue;
            }

            if self.consume_optional(&TokenKind::MinusMinus) {
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
        let token = self.advance();

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
                if self.consume_optional(&TokenKind::RightParen) {
                    if self.consume_optional(&TokenKind::Arrow) {
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

                if let TokenKind::Identifier(parameter) = self.peek().kind.clone() {
                    let has_closing_parenthesis = self
                        .tokens
                        .get(self.position + 1)
                        .is_some_and(|token| matches!(token.kind, TokenKind::RightParen));

                    let has_arrow = self
                        .tokens
                        .get(self.position + 2)
                        .is_some_and(|token| matches!(token.kind, TokenKind::Arrow));

                    if has_closing_parenthesis && has_arrow {
                        self.advance();
                        self.advance();
                        self.advance();

                        return Ok(Expression::Function(UserFunction {
                            parameters: vec![parameter],
                            body: self.parse_function_body()?,
                        }));
                    }
                }

                let expression = self.parse_expression()?;
                self.consume(&TokenKind::RightParen, "expected ')' after expression")?;

                Ok(expression)
            }

            TokenKind::LeftBracket => self.parse_array_literal(),

            _ => Err(JsError::syntax(token.offset, "expected an expression")),
        }
    }

    fn parse_array_literal(&mut self) -> Result<Expression, JsError> {
        let mut values = Vec::new();

        if self.consume_optional(&TokenKind::RightBracket) {
            return Ok(Expression::Array(values));
        }

        loop {
            values.push(self.parse_expression()?);

            if self.consume_optional(&TokenKind::Comma) {
                if self.consume_optional(&TokenKind::RightBracket) {
                    return Ok(Expression::Array(values));
                }

                continue;
            }

            self.consume(
                &TokenKind::RightBracket,
                "expected ',' or ']' after array element",
            )?;

            return Ok(Expression::Array(values));
        }
    }

    fn parse_function_expression(&mut self) -> Result<Expression, JsError> {
        self.consume(&TokenKind::LeftParen, "expected '(' after function keyword")?;

        let parameters = self.parse_function_parameters()?;
        let body = self.parse_function_body()?;

        Ok(Expression::Function(UserFunction { parameters, body }))
    }

    fn parse_function_parameters(&mut self) -> Result<Vec<String>, JsError> {
        let mut parameters = Vec::new();

        if self.consume_optional(&TokenKind::RightParen) {
            return Ok(parameters);
        }

        loop {
            let (parameter, _) = self.consume_identifier("expected a function identifier")?;

            parameters.push(parameter);

            if self.consume_optional(&TokenKind::Comma) {
                continue;
            }

            self.consume(
                &TokenKind::RightParen,
                "expected ',' or ')' after function parameter",
            )?;

            return Ok(parameters);
        }
    }

    fn parse_function_body(&mut self) -> Result<Vec<Statement>, JsError> {
        self.consume(&TokenKind::LeftBrace, "expected '{' before function body")?;

        let mut body = Vec::new();

        while !self.check(&TokenKind::RightBrace) {
            if self.check(&TokenKind::EndOfFile) {
                return Err(JsError::syntax(
                    self.peek().offset,
                    "unterminated function body",
                ));
            }

            body.push(self.parse_statement()?);
        }

        self.advance();

        Ok(body)
    }

    fn parse_new_expression(&mut self, offset: usize) -> Result<Expression, JsError> {
        let (constructor, _) = self.consume_identifier("expected a constructor after 'new'")?;

        self.consume(&TokenKind::LeftParen, "expected '(' after constructor name")?;

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

        if self.consume_optional(&TokenKind::RightParen) {
            return Ok(arguments);
        }

        loop {
            arguments.push(self.parse_expression()?);

            if self.consume_optional(&TokenKind::Comma) {
                if self.consume_optional(&TokenKind::RightParen) {
                    return Ok(arguments);
                }

                continue;
            }

            self.consume(&TokenKind::RightParen, "expected ',' or ')' after argument")?;

            return Ok(arguments);
        }
    }

    fn consume_identifier(&mut self, message: &str) -> Result<(String, usize), JsError> {
        let token = self.advance();

        match token.kind {
            TokenKind::Identifier(name) => Ok((name, token.offset)),
            _ => Err(JsError::syntax(token.offset, message)),
        }
    }

    fn consume_contextual_keyword(&mut self, keyword: &str) -> bool {
        match &self.peek().kind {
            TokenKind::Identifier(identifier) if identifier == keyword => {
                self.advance();
                true
            }
            _ => false,
        }
    }

    fn consume(&mut self, expected: &TokenKind, message: &str) -> Result<(), JsError> {
        if self.check(expected) {
            self.advance();
            Ok(())
        } else {
            Err(JsError::syntax(self.peek().offset, message))
        }
    }

    fn consume_optional(&mut self, expected: &TokenKind) -> bool {
        if self.check(expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn check(&self, expected: &TokenKind) -> bool {
        self.peek().kind == *expected
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.position]
    }

    fn advance(&mut self) -> Token {
        let token = self.tokens[self.position].clone();

        if !matches!(token.kind, TokenKind::EndOfFile) {
            self.position += 1;
        }

        token
    }
}
