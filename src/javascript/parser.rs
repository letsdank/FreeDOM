use super::{
    BinaryOperator, Expression, JsError, Statement, Token, TokenKind, UserFunction, Value,
};

use cursor::ParserCursor;

#[path = "parser/cursor.rs"]
mod cursor;

#[path = "parser/statement.rs"]
mod statement;

#[path = "parser/primary.rs"]
mod primary;

#[path = "parser/function.rs"]
mod function;

#[path = "parser/expression.rs"]
mod expression;

pub(super) fn parse_program(source: &str) -> Result<Vec<Statement>, JsError> {
    Parser::new(source).parse_program()
}

struct Parser<'source> {
    cursor: ParserCursor<'source>,
}

impl<'source> Parser<'source> {
    fn new(source: &'source str) -> Self {
        Self {
            cursor: ParserCursor::new(source),
        }
    }

    fn parse_program(&mut self) -> Result<Vec<Statement>, JsError> {
        let mut statements = Vec::new();

        while !self.cursor.check(&TokenKind::EndOfFile)? {
            statements.push(self.parse_statement()?);
        }

        Ok(statements)
    }

    fn consume_identifier(&mut self, message: &str) -> Result<(String, usize), JsError> {
        let token = self.cursor.advance()?;

        match token.kind {
            TokenKind::Identifier(name) => Ok((name, token.offset)),
            _ => Err(JsError::syntax(token.offset, message)),
        }
    }

    fn consume_contextual_keyword(&mut self, keyword: &str) -> Result<bool, JsError> {
        match &self.cursor.peek()?.kind {
            TokenKind::Identifier(identifier) if identifier == keyword => {
                self.cursor.advance()?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}
