//! Cursor over tokens already produced by the JavaScript lexer.
//!
//! This does not choose lexical goals or validate JSON syntax.

use super::{JsError, Token, TokenKind};

pub(super) struct TokenCursor {
    tokens: Vec<Token>,
    position: usize,
}

impl TokenCursor {
    pub(super) fn new(tokens: Vec<Token>) -> Self {
        debug_assert!(matches!(
            tokens.last().map(|token| &token.kind),
            Some(TokenKind::EndOfFile)
        ));

        Self {
            tokens,
            position: 0,
        }
    }

    pub(super) fn peek(&self) -> &Token {
        &self.tokens[self.position]
    }

    pub(super) fn peek_n(&self, offset: usize) -> Option<&Token> {
        self.position
            .checked_add(offset)
            .and_then(|index| self.tokens.get(index))
    }

    pub(super) fn advance(&mut self) -> Token {
        let token = self.tokens[self.position].clone();

        if !matches!(token.kind, TokenKind::EndOfFile) {
            self.position += 1;
        }

        token
    }

    pub(super) fn check(&self, expected: &TokenKind) -> bool {
        self.peek().kind == *expected
    }

    pub(super) fn consume_optional(&mut self, expected: &TokenKind) -> bool {
        if self.check(expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    pub(super) fn consume(&mut self, expected: &TokenKind, message: &str) -> Result<(), JsError> {
        if self.check(expected) {
            self.advance();
            Ok(())
        } else {
            Err(JsError::syntax(self.peek().offset, message))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{TokenCursor, TokenKind};
    use crate::javascript::tokenize;

    #[test]
    fn lookahead_and_eof_do_not_advance_the_cursor() {
        let mut cursor = TokenCursor::new(tokenize("a").unwrap());

        assert!(matches!(cursor.peek().kind, TokenKind::Identifier(_)));

        assert!(matches!(
            cursor.peek_n(1).unwrap().kind,
            TokenKind::EndOfFile
        ));

        assert!(cursor.peek_n(2).is_none());

        assert!(matches!(cursor.advance().kind, TokenKind::Identifier(_)));
        assert!(matches!(cursor.advance().kind, TokenKind::EndOfFile));
        assert!(matches!(cursor.advance().kind, TokenKind::EndOfFile));
    }

    #[test]
    fn failed_consume_reports_offset_and_does_not_advance() {
        let mut cursor = TokenCursor::new(tokenize("  x").unwrap());

        let error = cursor
            .consume(&TokenKind::LeftParen, "expected opening parenthesis")
            .unwrap_err();

        assert_eq!(error.offset, Some(2));
        assert_eq!(error.message, "expected opening parenthesis");

        assert!(!cursor.consume_optional(&TokenKind::LeftParen));
        assert!(matches!(cursor.peek().kind, TokenKind::Identifier(_)));
    }
}
