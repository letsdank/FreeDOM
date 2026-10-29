//! Buffered token lookahead with explicit slash-mode selection.
//!
//! <https://tc39.es/ecma262/#sec-ecmascript-language-lexical-grammar>
//!
//! This supports only division and RegExp slash modes, not all lexical goals.

use std::collections::VecDeque;

use super::lexer::{LexError, Lexer, Token, TokenKind};

struct BufferedToken {
    token: Token,
    allow_regexp: bool,
    checkpoint: usize,
}

pub(super) struct TokenStream<'source> {
    lexer: Lexer<'source>,
    buffered: VecDeque<BufferedToken>,
}

impl<'source> TokenStream<'source> {
    pub(super) fn new(source: &'source str) -> Self {
        Self {
            lexer: Lexer::new(source),
            buffered: VecDeque::new(),
        }
    }

    pub(super) fn peek(&mut self, allow_regexp: bool) -> Result<&Token, LexError> {
        Ok(self.peek_n(&[allow_regexp])?.expect("one token must exist"))
    }

    pub(super) fn peek_n<'a>(&'a mut self, modes: &[bool]) -> Result<Option<&'a Token>, LexError> {
        for (index, &allow_regexp) in modes.iter().enumerate() {
            if index > 0 && self.buffered[index - 1].token.kind == TokenKind::EndOfFile {
                return Ok(None);
            }

            let rescan = self.buffered.get(index).is_some_and(|item| {
                item.allow_regexp != allow_regexp
                    && matches!(
                        item.token.kind,
                        TokenKind::Slash | TokenKind::RegExpLiteral { .. }
                    )
            });

            if rescan {
                let checkpoint = self.buffered[index].checkpoint;

                self.buffered.truncate(index);
                self.lexer.restore(checkpoint);
            }

            if self.buffered.len() == index {
                let checkpoint = self.lexer.checkpoint();

                let token = match self.lexer.next_token(allow_regexp) {
                    Ok(token) => token,
                    Err(error) => {
                        self.lexer.restore(checkpoint);
                        return Err(error);
                    }
                };

                self.buffered.push_back(BufferedToken {
                    token,
                    allow_regexp,
                    checkpoint,
                });
            }
        }

        Ok(modes
            .len()
            .checked_sub(1)
            .and_then(|index| self.buffered.get(index))
            .map(|item| &item.token))
    }

    pub(super) fn advance(&mut self, allow_regexp: bool) -> Result<Token, LexError> {
        self.peek(allow_regexp)?;

        Ok(self
            .buffered
            .pop_front()
            .expect("buffered token must exist")
            .token)
    }
}

#[cfg(test)]
mod tests {
    use super::TokenStream;
    use crate::javascript::TokenKind;

    #[test]
    fn peeks_multiple_tokens_without_consuming_them() {
        let mut stream = TokenStream::new("(x) => x");

        assert_eq!(
            stream.peek_n(&[false, false, false]).unwrap().unwrap().kind,
            TokenKind::RightParen
        );

        assert_eq!(
            stream
                .peek_n(&[false, false, false, false])
                .unwrap()
                .unwrap()
                .kind,
            TokenKind::Arrow
        );

        assert_eq!(stream.advance(false).unwrap().kind, TokenKind::LeftParen);
        assert_eq!(
            stream.advance(false).unwrap().kind,
            TokenKind::Identifier("x".to_owned())
        );
        assert_eq!(stream.advance(false).unwrap().kind, TokenKind::RightParen);
    }

    #[test]
    fn rescan_invalidates_later_buffered_tokens() {
        let mut stream = TokenStream::new("f(/a/g, 8 / 2)");

        assert_eq!(
            stream.peek_n(&[false, false, false]).unwrap().unwrap().kind,
            TokenKind::Slash
        );

        assert_eq!(
            stream
                .peek_n(&[false, false, true, false])
                .unwrap()
                .unwrap()
                .kind,
            TokenKind::Comma
        );

        assert_eq!(
            stream
                .peek_n(&[false, false, false, false])
                .unwrap()
                .unwrap()
                .kind,
            TokenKind::Identifier("a".to_owned())
        );

        assert_eq!(
            stream.advance(false).unwrap().kind,
            TokenKind::Identifier("f".to_owned())
        );
        assert_eq!(stream.advance(false).unwrap().kind, TokenKind::LeftParen);
        assert_eq!(
            stream.advance(true).unwrap().kind,
            TokenKind::RegExpLiteral {
                pattern: "a".to_owned(),
                flags: "g".to_owned(),
            }
        );
        assert_eq!(stream.advance(false).unwrap().kind, TokenKind::Comma);
        assert_eq!(stream.advance(false).unwrap().kind, TokenKind::Number(8.0));
        assert_eq!(stream.advance(false).unwrap().kind, TokenKind::Slash);
    }

    #[test]
    fn lookahead_reports_eof_without_creating_extra_tokens() {
        let mut stream = TokenStream::new("x");

        assert!(stream.peek_n(&[]).unwrap().is_none());

        assert_eq!(
            stream.peek_n(&[false, false]).unwrap().unwrap().kind,
            TokenKind::EndOfFile
        );

        assert!(stream.peek_n(&[false, false, false]).unwrap().is_none());

        assert_eq!(
            stream.advance(false).unwrap().kind,
            TokenKind::Identifier("x".to_owned())
        );
        assert_eq!(stream.advance(false).unwrap().kind, TokenKind::EndOfFile);
        assert_eq!(stream.advance(false).unwrap().kind, TokenKind::EndOfFile);
    }

    #[test]
    fn lookahead_error_preserves_preceding_tokens() {
        let mut stream = TokenStream::new("a /oops");

        assert_eq!(
            stream.peek_n(&[false, true]).unwrap_err().message,
            "unterminated regular expression"
        );

        assert_eq!(
            stream.advance(false).unwrap().kind,
            TokenKind::Identifier("a".to_owned())
        );
        assert_eq!(stream.advance(false).unwrap().kind, TokenKind::Slash);
    }
}
