//! Fallible token cursor for the JavaScript parser.
//!
//! Division mode is the default. Primary expressions can request RegExp mode.

use super::{JsError, Token, TokenKind};
use crate::javascript::token_stream::TokenStream;

pub(super) struct ParserCursor<'source> {
    stream: TokenStream<'source>,
}

impl<'source> ParserCursor<'source> {
    pub(super) fn new(source: &'source str) -> Self {
        Self {
            stream: TokenStream::new(source),
        }
    }

    pub(super) fn peek(&mut self) -> Result<Token, JsError> {
        self.stream.peek(false).cloned().map_err(Self::lex_error)
    }

    pub(super) fn peek_n(&mut self, offset: usize) -> Result<Option<Token>, JsError> {
        let Some(count) = offset.checked_add(1) else {
            return Ok(None);
        };

        self.stream
            .peek_n(&vec![false; count])
            .map(|token| token.cloned())
            .map_err(Self::lex_error)
    }

    pub(super) fn advance(&mut self) -> Result<Token, JsError> {
        self.stream.advance(false).map_err(Self::lex_error)
    }

    pub(super) fn advance_primary(&mut self) -> Result<Token, JsError> {
        self.stream.advance(true).map_err(Self::lex_error)
    }

    pub(super) fn check(&mut self, expected: &TokenKind) -> Result<bool, JsError> {
        Ok(self.peek()?.kind == *expected)
    }

    pub(super) fn consume_optional(&mut self, expected: &TokenKind) -> Result<bool, JsError> {
        if self.check(expected)? {
            self.advance()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub(super) fn consume(&mut self, expected: &TokenKind, message: &str) -> Result<(), JsError> {
        if self.consume_optional(expected)? {
            Ok(())
        } else {
            Err(JsError::syntax(self.peek()?.offset, message))
        }
    }

    fn lex_error(error: crate::javascript::lexer::LexError) -> JsError {
        JsError::syntax(error.offset, error.message)
    }
}

#[cfg(test)]
mod tests {
    use super::ParserCursor;
    use crate::javascript::TokenKind;

    #[test]
    fn lookahead_keeps_tokens_and_eof_stable() {
        let mut cursor = ParserCursor::new("a => a");

        assert_eq!(cursor.peek_n(1).unwrap().unwrap().kind, TokenKind::Arrow);
        assert_eq!(
            cursor.advance().unwrap().kind,
            TokenKind::Identifier("a".to_owned())
        );
        assert_eq!(cursor.advance().unwrap().kind, TokenKind::Arrow);
        assert_eq!(
            cursor.advance().unwrap().kind,
            TokenKind::Identifier("a".to_owned())
        );
        assert_eq!(cursor.advance().unwrap().kind, TokenKind::EndOfFile);
        assert_eq!(cursor.advance().unwrap().kind, TokenKind::EndOfFile);
    }

    #[test]
    fn propagates_lexer_error_with_byte_offset() {
        let mut cursor = ParserCursor::new("x @");

        assert_eq!(
            cursor.advance().unwrap().kind,
            TokenKind::Identifier("x".to_owned())
        );

        let error = cursor.peek().unwrap_err();

        assert_eq!(error.offset, Some(2));
        assert_eq!(error.message, "unexpected character '@'");
    }
}
