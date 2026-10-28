//! Partial parser for ECMAScript RegExp patterns.
//!
//! <https://tc39.es/ecma262/#sec-patterns>
//!
//! Matching, quantifiers, character classes, backreferences, and Unicode
//! modes are not implemented here yet.

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Pattern {
    pub(crate) alternatives: Vec<Vec<Term>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Term {
    Literal(char),
    Any,
    Start,
    End,
    Group {
        capturing: bool,
        pattern: Box<Pattern>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PatternErrorKind {
    UnmatchedClosingParenthesis,
    UnterminatedGroup,
    DanglingEscape,
    Unsupported(char),
    NestingLimit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PatternError {
    /// Byte offset relative to the beginning of the pattern.
    pub(crate) offset: usize,
    pub(crate) kind: PatternErrorKind,
}

pub(crate) fn parse(source: &str) -> Result<Pattern, PatternError> {
    let mut parser = Parser { source, offset: 0 };

    parser.disjunction(false, 0)
}

struct Parser<'a> {
    source: &'a str,
    offset: usize,
}

impl Parser<'_> {
    fn disjunction(&mut self, in_group: bool, depth: usize) -> Result<Pattern, PatternError> {
        let mut alternatives = vec![Vec::new()];

        while let Some(character) = self.peek() {
            if character == ')' {
                if in_group {
                    break;
                }

                return Err(self.error(PatternErrorKind::UnmatchedClosingParenthesis));
            }

            if character == '|' {
                self.advance();
                alternatives.push(Vec::new());
                continue;
            }

            let term = self.term(depth)?;

            alternatives.last_mut().unwrap().push(term);
        }

        Ok(Pattern { alternatives })
    }

    fn term(&mut self, depth: usize) -> Result<Term, PatternError> {
        let offset = self.offset;
        let character = self.advance().unwrap();

        match character {
            '.' => Ok(Term::Any),
            '^' => Ok(Term::Start),
            '$' => Ok(Term::End),
            '(' => self.group(offset, depth),
            '\\' => self.escape(offset),

            '*' | '+' | '?' | '[' | ']' | '{' | '}' => Err(PatternError {
                offset,
                kind: PatternErrorKind::Unsupported(character),
            }),

            _ => Ok(Term::Literal(character)),
        }
    }

    fn group(&mut self, group_start: usize, depth: usize) -> Result<Term, PatternError> {
        const MAX_GROUP_DEPTH: usize = 128;

        if depth >= MAX_GROUP_DEPTH {
            return Err(PatternError {
                offset: group_start,
                kind: PatternErrorKind::NestingLimit,
            });
        }

        let capturing = if self.peek() == Some('?') {
            let question_offset = self.offset;
            self.advance();

            if self.peek() != Some(':') {
                return Err(PatternError {
                    offset: question_offset,
                    kind: PatternErrorKind::Unsupported('?'),
                });
            }

            self.advance();

            false
        } else {
            true
        };

        let pattern = self.disjunction(true, depth + 1)?;

        if self.peek() != Some(')') {
            return Err(PatternError {
                offset: group_start,
                kind: PatternErrorKind::UnterminatedGroup,
            });
        }

        self.advance();

        Ok(Term::Group {
            capturing,
            pattern: Box::new(pattern),
        })
    }

    fn escape(&mut self, escape_start: usize) -> Result<Term, PatternError> {
        let Some(character) = self.advance() else {
            return Err(PatternError {
                offset: escape_start,
                kind: PatternErrorKind::DanglingEscape,
            });
        };

        let value = match character {
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            'f' => '\u{000C}',
            'v' => '\u{000B}',

            '^' | '$' | '\\' | '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|'
            | '/' => character,

            _ => {
                return Err(PatternError {
                    offset: escape_start,
                    kind: PatternErrorKind::Unsupported(character),
                });
            }
        };

        Ok(Term::Literal(value))
    }

    fn peek(&self) -> Option<char> {
        self.source[self.offset..].chars().next()
    }

    fn advance(&mut self) -> Option<char> {
        let character = self.peek()?;
        self.offset += character.len_utf8();

        Some(character)
    }

    fn error(&self, kind: PatternErrorKind) -> PatternError {
        PatternError {
            offset: self.offset,
            kind,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Pattern, PatternErrorKind, Term, parse};

    #[test]
    fn parses_nested_groups_alternatives_and_anchors() {
        let pattern = parse(r"^(a|(?:b\.))$").unwrap();

        assert_eq!(
            pattern,
            Pattern {
                alternatives: vec![vec![
                    Term::Start,
                    Term::Group {
                        capturing: true,
                        pattern: Box::new(Pattern {
                            alternatives: vec![
                                vec![Term::Literal('a')],
                                vec![Term::Group {
                                    capturing: false,
                                    pattern: Box::new(Pattern {
                                        alternatives: vec![vec![
                                            Term::Literal('b'),
                                            Term::Literal('.')
                                        ]],
                                    })
                                }]
                            ]
                        })
                    },
                    Term::End,
                ]]
            }
        );
    }

    #[test]
    fn handles_empty_alternatives_and_unicode_offsets() {
        assert_eq!(
            parse("|a|").unwrap().alternatives,
            vec![vec![], vec![Term::Literal('a')], vec![]],
        );

        assert_eq!(parse("é)").unwrap_err().offset, 2);
        assert_eq!(parse(".").unwrap().alternatives, vec![vec![Term::Any]]);
    }

    #[test]
    fn distinguishes_unsupported_features_from_syntax_errors() {
        assert_eq!(
            parse(r"\d").unwrap_err().kind,
            PatternErrorKind::Unsupported('d'),
        );

        assert_eq!(
            parse("a+").unwrap_err().kind,
            PatternErrorKind::Unsupported('+'),
        );

        assert_eq!(
            parse("(?=a)").unwrap_err().kind,
            PatternErrorKind::Unsupported('?'),
        );

        assert_eq!(
            parse("(a").unwrap_err().kind,
            PatternErrorKind::UnterminatedGroup,
        );

        assert_eq!(
            parse("a)").unwrap_err().kind,
            PatternErrorKind::UnmatchedClosingParenthesis,
        );

        assert_eq!(
            parse("\\").unwrap_err().kind,
            PatternErrorKind::DanglingEscape,
        );
    }

    #[test]
    fn limits_group_nesting() {
        let input = format!("{}{}", "(".repeat(129), ")".repeat(129));

        assert_eq!(
            parse(&input).unwrap_err().kind,
            PatternErrorKind::NestingLimit,
        );
    }
}
