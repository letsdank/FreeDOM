use std::{error::Error, fmt};

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    Identifier(String),
    String(String),
    Number(f64),

    RegExpLiteral { pattern: String, flags: String },

    Let,
    Const,
    Var,
    New,
    True,
    False,
    Null,
    Typeof,

    LeftParen,
    RightParen,
    LeftBrace,
    RightBrace,
    LeftBracket,
    RightBracket,

    Dot,
    Comma,
    Semicolon,
    Colon,
    Question,

    Plus,
    Minus,
    Star,
    Slash,

    Assign,
    Equal,
    StrictEqual,
    Not,
    NotEqual,
    StrictNotEqual,

    Less,
    LessEqual,
    Greater,
    GreaterEqual,

    Arrow,
    EndOfFile,

    Function,
    Instanceof,
    Return,

    If,
    Else,

    And,
    Or,
    While,
    For,
    Break,
    Continue,
    PlusPlus,
    MinusMinus,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub offset: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LexError {
    pub offset: usize,
    pub message: String,
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.message, self.offset)
    }
}

impl Error for LexError {}

pub fn tokenize(source: &str) -> Result<Vec<Token>, LexError> {
    let mut stream = TokenStream::new(source);
    let mut tokens = Vec::new();

    loop {
        let token = stream.advance(false)?;
        let at_end = token.kind == TokenKind::EndOfFile;

        tokens.push(token);

        if at_end {
            return Ok(tokens);
        }
    }
}

/// A one-token lookahead stream with explicit slash-mode selection.
///
/// Changing the mode rescans a buffered slash without consuming it.
/// Other ECMAScript lexical goals are not implemented yet.
pub(super) struct TokenStream<'source> {
    lexer: Lexer<'source>,
    buffered: Option<(Token, bool)>,
}

impl<'source> TokenStream<'source> {
    pub(super) fn new(source: &'source str) -> Self {
        Self {
            lexer: Lexer::new(source),
            buffered: None,
        }
    }

    pub(super) fn peek(&mut self, allow_regexp: bool) -> Result<&Token, LexError> {
        let rescan = self.buffered.as_ref().is_some_and(|(token, old_mode)| {
            *old_mode != allow_regexp
                && matches!(
                    token.kind,
                    TokenKind::Slash | TokenKind::RegExpLiteral { .. }
                )
        });

        if rescan {
            let (token, _) = self.buffered.take().expect("buffered token must exist");

            self.lexer.index = token.offset;
        }

        if self.buffered.is_none() {
            let checkpoint = self.lexer.index;

            let token = match self.lexer.next_token(allow_regexp) {
                Ok(token) => token,
                Err(error) => {
                    self.lexer.index = checkpoint;
                    return Err(error);
                }
            };

            self.buffered = Some((token, allow_regexp));
        }

        Ok(&self.buffered.as_ref().expect("buffered token must exist").0)
    }

    pub(super) fn advance(&mut self, allow_regexp: bool) -> Result<Token, LexError> {
        self.peek(allow_regexp)?;

        Ok(self.buffered.take().expect("buffered token must exist").0)
    }
}

pub(super) struct Lexer<'source> {
    source: &'source str,
    index: usize,
    tokens: Vec<Token>,
}

impl<'source> Lexer<'source> {
    pub(super) fn new(source: &'source str) -> Self {
        Self {
            source,
            index: 0,
            tokens: Vec::new(),
        }
    }

    // Selects the slash mode for exactly one token. Other lexical goals are
    // not implemented yet.
    pub(super) fn next_token(&mut self, allow_regexp: bool) -> Result<Token, LexError> {
        let found = if allow_regexp {
            self.scan_next_with_regexp(true)?
        } else {
            self.scan_next()?
        };

        if found {
            Ok(self.tokens.pop().expect("scanning a token must append it"))
        } else {
            Ok(Token {
                kind: TokenKind::EndOfFile,
                offset: self.index,
            })
        }
    }

    fn scan_next(&mut self) -> Result<bool, LexError> {
        self.scan_next_with_regexp(false)
    }

    // The caller selects whether a slash can start a RegExp literal.
    // This is not yet the complete ECMAScript lexical-goal mechanism.
    fn scan_next_with_regexp(&mut self, allow_regexp: bool) -> Result<bool, LexError> {
        while let Some(character) = self.advance() {
            let offset = self.index - character.len_utf8();

            match character {
                character if character.is_ascii_whitespace() => continue,

                '/' if self.consume_if('/') => {
                    self.skip_line_comment();
                    continue;
                }

                '/' if self.consume_if('*') => {
                    self.skip_block_comment(offset)?;
                    continue;
                }

                '"' | '\'' => self.read_string(character, offset)?,

                character if character.is_ascii_digit() => self.read_number(offset)?,

                character if is_identifier_start(character) => self.read_identifier(offset),

                '(' => self.push(TokenKind::LeftParen, offset),
                ')' => self.push(TokenKind::RightParen, offset),
                '{' => self.push(TokenKind::LeftBrace, offset),
                '}' => self.push(TokenKind::RightBrace, offset),
                '[' => self.push(TokenKind::LeftBracket, offset),
                ']' => self.push(TokenKind::RightBracket, offset),
                '.' => self.push(TokenKind::Dot, offset),
                ',' => self.push(TokenKind::Comma, offset),
                ';' => self.push(TokenKind::Semicolon, offset),
                ':' => self.push(TokenKind::Colon, offset),
                '?' => self.push(TokenKind::Question, offset),

                '+' if self.consume_if('+') => self.push(TokenKind::PlusPlus, offset),
                '-' if self.consume_if('-') => self.push(TokenKind::MinusMinus, offset),
                '+' => self.push(TokenKind::Plus, offset),
                '-' => self.push(TokenKind::Minus, offset),
                '*' => self.push(TokenKind::Star, offset),

                '/' if allow_regexp => {
                    let literal = super::regexp::scan_literal(self.source, offset)?;

                    let pattern = literal.pattern.to_owned();
                    let flags = literal.flags.to_owned();

                    self.index = literal.end;
                    self.push(TokenKind::RegExpLiteral { pattern, flags }, offset);
                }

                '/' => self.push(TokenKind::Slash, offset),

                '=' => {
                    let kind = if self.consume_if('=') {
                        if self.consume_if('=') {
                            TokenKind::StrictEqual
                        } else {
                            TokenKind::Equal
                        }
                    } else if self.consume_if('>') {
                        TokenKind::Arrow
                    } else {
                        TokenKind::Assign
                    };

                    self.push(kind, offset);
                }

                '!' => {
                    let kind = if self.consume_if('=') {
                        if self.consume_if('=') {
                            TokenKind::StrictNotEqual
                        } else {
                            TokenKind::NotEqual
                        }
                    } else {
                        TokenKind::Not
                    };

                    self.push(kind, offset);
                }

                '<' if self.consume_if('=') => self.push(TokenKind::LessEqual, offset),
                '<' => self.push(TokenKind::Less, offset),

                '>' if self.consume_if('=') => self.push(TokenKind::GreaterEqual, offset),
                '>' => self.push(TokenKind::Greater, offset),

                '&' if self.consume_if('&') => self.push(TokenKind::And, offset),
                '|' if self.consume_if('|') => self.push(TokenKind::Or, offset),

                unexpected => {
                    return Err(LexError {
                        offset,
                        message: format!("unexpected character {unexpected:?}"),
                    });
                }
            }

            return Ok(true);
        }

        Ok(false)
    }

    fn read_string(&mut self, quote: char, offset: usize) -> Result<(), LexError> {
        let mut value = String::new();

        loop {
            let Some(character) = self.advance() else {
                return Err(LexError {
                    offset,
                    message: "unterminated string literal".to_owned(),
                });
            };

            if character == quote {
                break;
            }

            if matches!(character, '\n' | '\r') {
                return Err(LexError {
                    offset,
                    message: "newline in string literal".to_owned(),
                });
            }

            if character != '\\' {
                value.push(character);
                continue;
            }

            let Some(escape) = self.advance() else {
                return Err(LexError {
                    offset,
                    message: "unterminated escape sequence".to_owned(),
                });
            };

            match escape {
                '"' => value.push('"'),
                '\'' => value.push('\''),
                '\\' => value.push('\\'),
                '/' => value.push('/'),
                'b' => value.push('\u{0008}'),
                'f' => value.push('\u{000C}'),
                'n' => value.push('\n'),
                'r' => value.push('\r'),
                't' => value.push('\t'),
                'u' => value.push(self.read_unicode_escape(offset)?),

                unknown => {
                    return Err(LexError {
                        offset,
                        message: format!("unsupported escape sequence \\{unknown}"),
                    });
                }
            }
        }

        self.push(TokenKind::String(value), offset);

        Ok(())
    }

    fn read_unicode_escape(&mut self, offset: usize) -> Result<char, LexError> {
        let mut value = 0_u32;

        for _ in 0..4 {
            let Some(character) = self.advance() else {
                return Err(LexError {
                    offset,
                    message: "incomplete Unicode escape".to_owned(),
                });
            };

            let Some(digit) = character.to_digit(16) else {
                return Err(LexError {
                    offset,
                    message: "invalid Unicode escape".to_owned(),
                });
            };

            value = value * 16 + digit;
        }

        char::from_u32(value).ok_or_else(|| LexError {
            offset,
            message: "invalid Unicode scalar value".to_owned(),
        })
    }

    fn read_number(&mut self, offset: usize) -> Result<(), LexError> {
        while self
            .peek()
            .is_some_and(|character| character.is_ascii_digit())
        {
            self.advance();
        }

        if self.peek() == Some('.') {
            self.advance();

            while self
                .peek()
                .is_some_and(|character| character.is_ascii_digit())
            {
                self.advance();
            }
        }

        if matches!(self.peek(), Some('e' | 'E')) {
            let exponent_start = self.index;
            self.advance();

            if matches!(self.peek(), Some('+' | '-')) {
                self.advance();
            }

            if self
                .peek()
                .is_some_and(|character| character.is_ascii_digit())
            {
                while self
                    .peek()
                    .is_some_and(|character| character.is_ascii_digit())
                {
                    self.advance();
                }
            } else {
                self.index = exponent_start;
            }
        }

        let number = self.source[offset..self.index]
            .parse::<f64>()
            .map_err(|_| LexError {
                offset,
                message: "invalid number literal".to_owned(),
            })?;

        self.push(TokenKind::Number(number), offset);

        Ok(())
    }

    fn read_identifier(&mut self, offset: usize) {
        while self.peek().is_some_and(is_identifier_continue) {
            self.advance();
        }

        let identifier = &self.source[offset..self.index];

        let kind = match identifier {
            "let" => TokenKind::Let,
            "const" => TokenKind::Const,
            "var" => TokenKind::Var,
            "new" => TokenKind::New,
            "true" => TokenKind::True,
            "false" => TokenKind::False,
            "null" => TokenKind::Null,
            "function" => TokenKind::Function,
            "instanceof" => TokenKind::Instanceof,
            "return" => TokenKind::Return,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "while" => TokenKind::While,
            "for" => TokenKind::For,
            "break" => TokenKind::Break,
            "continue" => TokenKind::Continue,
            "typeof" => TokenKind::Typeof,
            _ => TokenKind::Identifier(identifier.to_owned()),
        };

        self.push(kind, offset);
    }

    fn skip_line_comment(&mut self) {
        while self.peek().is_some_and(|character| character != '\n') {
            self.advance();
        }
    }

    fn skip_block_comment(&mut self, offset: usize) -> Result<(), LexError> {
        loop {
            let Some(character) = self.advance() else {
                return Err(LexError {
                    offset,
                    message: "unterminated block comment".to_owned(),
                });
            };

            if character == '*' && self.consume_if('/') {
                return Ok(());
            }
        }
    }

    fn advance(&mut self) -> Option<char> {
        let character = self.peek()?;
        self.index += character.len_utf8();
        Some(character)
    }

    fn consume_if(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<char> {
        self.source[self.index..].chars().next()
    }

    fn push(&mut self, kind: TokenKind, offset: usize) {
        self.tokens.push(Token { kind, offset });
    }
}

fn is_identifier_start(character: char) -> bool {
    character.is_ascii_alphabetic() || matches!(character, '_' | '$')
}

fn is_identifier_continue(character: char) -> bool {
    is_identifier_start(character) || character.is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use super::{Lexer, TokenKind, TokenStream, tokenize};

    #[test]
    fn scans_one_token_at_a_time_and_skips_comments() {
        let mut lexer = Lexer::new("  /* comment */ let x = 8 / 2; // end");
        let mut counts = Vec::new();

        while lexer.scan_next().unwrap() {
            counts.push(lexer.tokens.len());
        }

        assert_eq!(counts, (1..=7).collect::<Vec<_>>());
        assert_eq!(lexer.tokens[4].kind, TokenKind::Slash);
        assert!(!lexer.scan_next().unwrap());
    }

    #[test]
    fn incremental_and_batch_scanning_agree() {
        let source = "let x = 'é'; // comment\nx = x + 'a';";
        let expected = tokenize(source).unwrap();

        let mut lexer = Lexer::new(source);

        while lexer.scan_next().unwrap() {}

        assert_eq!(lexer.tokens, expected[..expected.len() - 1]);
        assert_eq!(lexer.index, source.len());
    }

    #[test]
    fn scans_wpt_harness_prefix_before_first_regexp_literal() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        let start = source
            .find(r"/^\(\)\s*=>\s*(?:{(.*)}\s*|(.*))$/")
            .expect("WPT RegExp must exist");

        let prefix = &source[..start];
        let expected = tokenize(prefix).unwrap();

        let mut lexer = Lexer::new(prefix);

        while lexer.scan_next().unwrap() {}

        assert_eq!(lexer.tokens, expected[..expected.len() - 1]);
        assert_eq!(lexer.index, start);
    }

    #[test]
    fn reports_unterminated_comment_while_scanning() {
        let mut lexer = Lexer::new("let /*");

        assert!(lexer.scan_next().unwrap());

        let error = lexer.scan_next().unwrap_err();

        assert_eq!(error.offset, 4);
        assert_eq!(error.message, "unterminated block comment");
    }

    #[test]
    fn slash_mode_distinguishes_division_ang_regexp() {
        let mut division = Lexer::new("/a/g");

        assert!(division.scan_next_with_regexp(false).unwrap());
        assert_eq!(division.tokens[0].kind, TokenKind::Slash);

        let mut regexp = Lexer::new("/a/g");

        assert!(regexp.scan_next_with_regexp(true).unwrap());
        assert_eq!(
            regexp.tokens[0].kind,
            TokenKind::RegExpLiteral {
                pattern: "a".to_owned(),
                flags: "g".to_owned(),
            }
        );
    }

    #[test]
    fn scans_unicode_regexp_after_comment_and_preserves_following_token() {
        let source = r"/* note */ /é[\/]x/gi.test";
        let mut lexer = Lexer::new(source);

        assert!(lexer.scan_next_with_regexp(true).unwrap());

        assert_eq!(lexer.tokens[0].offset, source.find("/é").unwrap());
        assert_eq!(
            lexer.tokens[0].kind,
            TokenKind::RegExpLiteral {
                pattern: r"é[\/]x".to_owned(),
                flags: "gi".to_owned(),
            }
        );

        assert!(lexer.scan_next().unwrap());
        assert_eq!(lexer.tokens[1].kind, TokenKind::Dot);
    }

    #[test]
    fn reports_unterminated_regexp_when_enabled() {
        let mut lexer = Lexer::new("/abc");

        let error = lexer.scan_next_with_regexp(true).unwrap_err();

        assert_eq!(error.offset, 0);
        assert_eq!(error.message, "unterminated regular expression");
    }

    #[test]
    fn scans_first_wpt_harness_regexp_after_its_prefix() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        let fragment = r"/^\(\)\s*=>\s*(?:{(.*)}\s*|(.*))$/";
        let start = source.find(fragment).expect("WPT RegExp must exist");

        let mut lexer = Lexer::new(source);

        while lexer.index < start {
            assert!(lexer.scan_next().unwrap());
        }

        assert_eq!(lexer.index, start);
        assert!(lexer.scan_next_with_regexp(true).unwrap());

        let token = lexer.tokens.last().unwrap();

        assert_eq!(token.offset, start);
        assert_eq!(
            token.kind,
            TokenKind::RegExpLiteral {
                pattern: fragment[1..fragment.len() - 1].to_owned(),
                flags: String::new(),
            }
        );

        assert!(lexer.scan_next().unwrap());
        assert_eq!(lexer.tokens.last().unwrap().kind, TokenKind::RightParen);
    }

    #[test]
    fn next_token_accepts_a_slash_mode_for_each_token() {
        let mut lexer = Lexer::new(" /é/gi / 2");

        let regexp = lexer.next_token(true).unwrap();

        assert_eq!(regexp.offset, 1);
        assert_eq!(
            regexp.kind,
            TokenKind::RegExpLiteral {
                pattern: "é".to_owned(),
                flags: "gi".to_owned(),
            }
        );

        let divide = lexer.next_token(false).unwrap();

        assert_eq!(divide.offset, 8);
        assert_eq!(divide.kind, TokenKind::Slash);
        assert_eq!(
            lexer.next_token(false).unwrap().kind,
            TokenKind::Number(2.0)
        );
    }

    #[test]
    fn next_token_keeps_eof_stable_and_preserves_errors() {
        let mut lexer = Lexer::new("é");
        let error = lexer.next_token(false).unwrap_err();

        assert_eq!(error.offset, 0);

        let mut empty = Lexer::new(" /* ok */ ");
        let eof = empty.next_token(false).unwrap();

        assert_eq!(eof.kind, TokenKind::EndOfFile);
        assert_eq!(eof.offset, " /* ok */ ".len());
        assert_eq!(empty.next_token(true).unwrap(), eof);

        let mut invalid = Lexer::new("/oops");
        let error = invalid.next_token(true).unwrap_err();

        assert_eq!(error.message, "unterminated regular expression");
    }

    #[test]
    fn next_token_reads_wpt_regexp_literal_and_suffix() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        let fragment = r"/^\(\)\s*=>\s*(?:{(.*)}\s*|(.*))$/";
        let start = source.find(fragment).expect("WPT RegExp must exist");

        let mut lexer = Lexer::new(&source[start..]);
        let token = lexer.next_token(true).unwrap();

        // The lexer receives a slice, so its offsets are relative to that slice.
        assert_eq!(token.offset, 0);
        assert_eq!(
            token.kind,
            TokenKind::RegExpLiteral {
                pattern: fragment[1..fragment.len() - 1].to_owned(),
                flags: String::new(),
            }
        );

        assert_eq!(lexer.next_token(false).unwrap().kind, TokenKind::RightParen);
    }

    #[test]
    fn stream_rescans_lookahead_when_slash_mode_changes() {
        let mut stream = TokenStream::new(" /*comment*/ /é/gi / 2");
        let first_offset = " /*comment*/ ".len();

        assert_eq!(stream.peek(false).unwrap().kind, TokenKind::Slash);

        let regexp = stream.peek(true).unwrap().clone();

        assert_eq!(regexp.offset, first_offset);
        assert_eq!(
            regexp.kind,
            TokenKind::RegExpLiteral {
                pattern: "é".to_owned(),
                flags: "gi".to_owned(),
            }
        );

        assert_eq!(stream.peek(false).unwrap().kind, TokenKind::Slash);
        assert_eq!(stream.advance(true).unwrap(), regexp);
        assert_eq!(stream.advance(false).unwrap().kind, TokenKind::Slash);
        assert_eq!(stream.advance(false).unwrap().kind, TokenKind::Number(2.0));
    }

    #[test]
    fn stream_recovers_position_after_failed_regexp_preview() {
        let mut stream = TokenStream::new("  /abc");

        let error = stream.peek(true).unwrap_err();
        assert_eq!(error.message, "unterminated regular expression");

        let slash = stream.advance(false).unwrap();

        assert_eq!(slash.offset, 2);
        assert_eq!(slash.kind, TokenKind::Slash);
        assert_eq!(
            stream.advance(false).unwrap().kind,
            TokenKind::Identifier("abc".to_owned())
        );
    }

    #[test]
    fn stream_keeps_non_slash_lookahead_and_eof_stable() {
        let mut stream = TokenStream::new("x");

        let token = stream.peek(false).unwrap().clone();

        assert_eq!(stream.peek(true).unwrap(), &token);
        assert_eq!(stream.advance(true).unwrap(), token);

        let end = stream.peek(false).unwrap().clone();

        assert_eq!(end.kind, TokenKind::EndOfFile);
        assert_eq!(end.offset, 1);
        assert_eq!(stream.advance(true).unwrap(), end);
        assert_eq!(stream.advance(false).unwrap(), end);
    }

    #[test]
    fn stream_rescans_wpt_harness_regexp() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        let fragment = r"/^\(\)\s*=>\s*(?:{(.*)}\s*|(.*))$/";
        let start = source.find(fragment).expect("WPT RegExp must exist");

        let mut stream = TokenStream::new(&source[start..]);

        assert_eq!(stream.peek(false).unwrap().kind, TokenKind::Slash);

        let token = stream.advance(true).unwrap();

        assert_eq!(token.offset, 0);
        assert_eq!(
            token.kind,
            TokenKind::RegExpLiteral {
                pattern: fragment[1..fragment.len() - 1].to_owned(),
                flags: String::new(),
            }
        );

        assert_eq!(stream.advance(false).unwrap().kind, TokenKind::RightParen);
    }
}
