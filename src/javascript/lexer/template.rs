//! Partial scanner for untagged template literal chunks.
//!
//! <https://tc39.es/ecma262/#sec-template-literal-lexical-components>

use super::{LexError, Lexer, Token, TokenKind};

impl Lexer<'_> {
    pub(in crate::javascript) fn read_template_chunk(
        &mut self,
        offset: usize,
    ) -> Result<Token, LexError> {
        let mut value = String::new();

        loop {
            let Some(ch) = self.advance() else {
                return Err(LexError {
                    offset,
                    message: "unterminated template literal".to_owned(),
                });
            };

            match ch {
                '`' => {
                    return Ok(Token {
                        kind: TokenKind::TemplateChunk { value, tail: true },
                        offset,
                    });
                }

                '$' if self.consume_if('{') => {
                    return Ok(Token {
                        kind: TokenKind::TemplateChunk { value, tail: false },
                        offset,
                    });
                }

                '\r' => {
                    self.consume_if('\n');
                    value.push('\n');
                }

                '\\' => {
                    let Some(escape) = self.advance() else {
                        return Err(LexError {
                            offset,
                            message: "unterminated template escape".to_owned(),
                        });
                    };

                    match escape {
                        'n' => value.push('\n'),
                        'r' => value.push('\r'),
                        't' => value.push('\t'),
                        'b' => value.push('\u{0008}'),
                        'f' => value.push('\u{000C}'),
                        'v' => value.push('\u{000B}'),

                        '0' if !self.peek().is_some_and(|ch| ch.is_ascii_digit()) => {
                            value.push('\0');
                        }

                        '\n' | '\u{2028}' | '\u{2029}' => {}

                        '\r' => {
                            self.consume_if('\n');
                        }

                        'u' => value.push(self.read_unicode_escape(offset)?),

                        '0'..='9' | 'x' => {
                            return Err(LexError {
                                offset,
                                message: "unsupported template escape".to_owned(),
                            });
                        }

                        other => value.push(other),
                    }
                }

                _ => value.push(ch),
            }
        }
    }
}
