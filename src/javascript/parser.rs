use super::{
    BinaryOperator, Expression, JsError, Statement, Token, TokenKind, UserFunction, Value, tokenize,
};

use super::token_cursor::TokenCursor;

#[path = "parser/statement.rs"]
mod statement;

#[path = "parser/primary.rs"]
mod primary;

#[path = "parser/function.rs"]
mod function;

#[path = "parser/expression.rs"]
mod expression;

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
