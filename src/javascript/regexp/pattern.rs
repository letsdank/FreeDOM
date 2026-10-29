//! Partial parser for ECMAScript RegExp patterns.
//!
//! <https://tc39.es/ecma262/#sec-patterns>
//!
//! Quantifiers, backreferences, and Unicode modes are not implemented here yet.
//! Bracket classes currently supports BMP literals, ranges, and predefined classes.

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
    WordBoundary {
        negated: bool,
    },
    CharacterClass {
        class: PredefinedClass,
        negated: bool,
    },
    BracketClass {
        negated: bool,
        items: Vec<ClassItem>,
    },
    Group {
        capturing: bool,
        pattern: Box<Pattern>,
    },
    Repeat {
        term: Box<Term>,
        min: usize,
        max: Option<usize>,
        greedy: bool,
    },
    Lookahead {
        negated: bool,
        pattern: Box<Pattern>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ClassItem {
    Range {
        start: u16,
        end: u16,
    },
    Predefined {
        class: PredefinedClass,
        negated: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PredefinedClass {
    Digit,
    Word,
    Space,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PatternErrorKind {
    UnmatchedClosingParenthesis,
    UnterminatedGroup,
    DanglingEscape,
    Unsupported(char),
    NestingLimit,
    UnterminatedCharacterClass,
    InvalidCharacterClassRange,
    InvalidQuantifierRange,
    QuantifierTooLarge,
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
            let term = self.quantity(term)?;

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
            '[' => self.bracket_class(offset),

            '*' | '+' | '?' | ']' | '{' | '}' => Err(PatternError {
                offset,
                kind: PatternErrorKind::Unsupported(character),
            }),

            _ => Ok(Term::Literal(character)),
        }
    }

    fn quantity(&mut self, term: Term) -> Result<Term, PatternError> {
        let (min, max, end) = match self.peek() {
            Some('*') => (0, None, self.offset + 1),
            Some('+') => (1, None, self.offset + 1),
            Some('?') => (0, Some(1), self.offset + 1),

            Some('{') => match self.counted_quantifier()? {
                Some(value) => value,
                None => return Ok(term),
            },

            _ => return Ok(term),
        };

        if matches!(
            term,
            Term::Start | Term::End | Term::WordBoundary { .. } | Term::Lookahead { .. }
        ) {
            return Err(PatternError {
                offset: self.offset,
                kind: PatternErrorKind::Unsupported(self.peek().unwrap()),
            });
        }

        if let Term::Literal(character) = &term {
            if character.len_utf16() == 2 {
                return Err(PatternError {
                    offset: self.offset,
                    kind: PatternErrorKind::Unsupported(*character),
                });
            }
        }

        self.offset = end;

        let greedy = if self.peek() == Some('?') {
            self.advance();
            false
        } else {
            true
        };

        Ok(Term::Repeat {
            term: Box::new(term),
            min,
            max,
            greedy,
        })
    }

    fn counted_quantifier(&self) -> Result<Option<(usize, Option<usize>, usize)>, PatternError> {
        let bytes = self.source.as_bytes();
        let mut cursor = self.offset + 1;

        let min_start = cursor;

        while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }

        if cursor == min_start {
            return Ok(None);
        }

        let min_end = cursor;
        let mut max_range = None;
        let mut open_ended = false;

        if bytes.get(cursor) == Some(&b',') {
            cursor += 1;

            let max_start = cursor;

            while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
                cursor += 1;
            }

            if cursor == max_start {
                open_ended = true;
            } else {
                max_range = Some((max_start, cursor));
            }
        }

        if bytes.get(cursor) != Some(&b'}') {
            return Ok(None);
        }

        let too_large = || PatternError {
            offset: self.offset,
            kind: PatternErrorKind::QuantifierTooLarge,
        };

        let min = self.source[min_start..min_end]
            .parse::<usize>()
            .map_err(|_| too_large())?;

        let max = if open_ended {
            None
        } else if let Some((start, end)) = max_range {
            Some(
                self.source[start..end]
                    .parse::<usize>()
                    .map_err(|_| too_large())?,
            )
        } else {
            Some(min)
        };

        if max.is_some_and(|max| min > max) {
            return Err(PatternError {
                offset: self.offset,
                kind: PatternErrorKind::InvalidQuantifierRange,
            });
        }

        Ok(Some((min, max, cursor + 1)))
    }

    fn group(&mut self, group_start: usize, depth: usize) -> Result<Term, PatternError> {
        const MAX_GROUP_DEPTH: usize = 128;

        enum GroupKind {
            Capturing,
            NonCapturing,
            Lookahead { negated: bool },
        }

        if depth >= MAX_GROUP_DEPTH {
            return Err(PatternError {
                offset: group_start,
                kind: PatternErrorKind::NestingLimit,
            });
        }

        let kind = if self.peek() == Some('?') {
            let question_offset = self.offset;
            self.advance();

            match self.advance() {
                Some(':') => GroupKind::NonCapturing,
                Some('=') => GroupKind::Lookahead { negated: false },
                Some('!') => GroupKind::Lookahead { negated: true },

                _ => {
                    return Err(PatternError {
                        offset: question_offset,
                        kind: PatternErrorKind::Unsupported('?'),
                    });
                }
            }
        } else {
            GroupKind::Capturing
        };

        let pattern = self.disjunction(true, depth + 1)?;

        if self.peek() != Some(')') {
            return Err(PatternError {
                offset: group_start,
                kind: PatternErrorKind::UnterminatedGroup,
            });
        }

        self.advance();

        match kind {
            GroupKind::Capturing => Ok(Term::Group {
                capturing: true,
                pattern: Box::new(pattern),
            }),

            GroupKind::NonCapturing => Ok(Term::Group {
                capturing: false,
                pattern: Box::new(pattern),
            }),

            GroupKind::Lookahead { negated } => Ok(Term::Lookahead {
                negated,
                pattern: Box::new(pattern),
            }),
        }
    }

    fn escape(&mut self, escape_start: usize) -> Result<Term, PatternError> {
        let Some(character) = self.advance() else {
            return Err(PatternError {
                offset: escape_start,
                kind: PatternErrorKind::DanglingEscape,
            });
        };

        match character {
            'b' => return Ok(Term::WordBoundary { negated: false }),
            'B' => return Ok(Term::WordBoundary { negated: true }),
            _ => {}
        }

        let class = match character {
            'd' => Some((PredefinedClass::Digit, false)),
            'D' => Some((PredefinedClass::Digit, true)),
            'w' => Some((PredefinedClass::Word, false)),
            'W' => Some((PredefinedClass::Word, true)),
            's' => Some((PredefinedClass::Space, false)),
            'S' => Some((PredefinedClass::Space, true)),
            _ => None,
        };

        if let Some((class, negated)) = class {
            return Ok(Term::CharacterClass { class, negated });
        }

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

    fn bracket_class(&mut self, class_start: usize) -> Result<Term, PatternError> {
        let negated = if self.peek() == Some('^') {
            self.advance();
            true
        } else {
            false
        };

        let mut items = Vec::new();

        loop {
            match self.peek() {
                None => {
                    return Err(PatternError {
                        offset: class_start,
                        kind: PatternErrorKind::UnterminatedCharacterClass,
                    });
                }

                Some(']') => {
                    self.advance();

                    return Ok(Term::BracketClass { negated, items });
                }

                _ => {}
            }

            let first = self.class_atom()?;

            if self.peek() == Some('-') && !self.source[self.offset + 1..].starts_with(']') {
                let dash_offset = self.offset;
                self.advance();

                if self.peek().is_none() {
                    return Err(PatternError {
                        offset: class_start,
                        kind: PatternErrorKind::UnterminatedCharacterClass,
                    });
                }

                let second = self.class_atom()?;

                match (first, second) {
                    (
                        ClassItem::Range {
                            start,
                            end: first_end,
                        },
                        ClassItem::Range {
                            start: second_start,
                            end,
                        },
                    ) if start == first_end && second_start == end => {
                        if start > end {
                            return Err(PatternError {
                                offset: dash_offset,
                                kind: PatternErrorKind::InvalidCharacterClassRange,
                            });
                        }

                        items.push(ClassItem::Range { start, end });
                    }

                    _ => {
                        return Err(PatternError {
                            offset: dash_offset,
                            kind: PatternErrorKind::Unsupported('-'),
                        });
                    }
                }
            } else {
                items.push(first);
            }
        }
    }

    fn class_atom(&mut self) -> Result<ClassItem, PatternError> {
        let offset = self.offset;
        let character = self.advance().unwrap();

        let term = if character == '\\' {
            if self.peek() == Some('b') {
                self.advance();
                Term::Literal('\u{0008}')
            } else if self.peek() == Some('-') {
                self.advance();
                Term::Literal('-')
            } else {
                self.escape(offset)?
            }
        } else {
            Term::Literal(character)
        };

        match term {
            Term::Literal(character) if (character as u32) <= 0xFFFF => {
                let unit = character as u16;

                Ok(ClassItem::Range {
                    start: unit,
                    end: unit,
                })
            }

            Term::Literal(character) => Err(PatternError {
                offset,
                kind: PatternErrorKind::Unsupported(character),
            }),

            Term::CharacterClass { class, negated } => Ok(ClassItem::Predefined { class, negated }),

            Term::WordBoundary { .. } => Err(PatternError {
                offset,
                kind: PatternErrorKind::Unsupported('B'),
            }),

            _ => unreachable!(),
        }
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
            parse(r"\p").unwrap_err().kind,
            PatternErrorKind::Unsupported('p'),
        );

        assert_eq!(
            parse("+a").unwrap_err().kind,
            PatternErrorKind::Unsupported('+'),
        );

        assert_eq!(
            parse("(?<=a)").unwrap_err().kind,
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

    #[test]
    fn validates_bracket_class_ranges_and_termination() {
        assert_eq!(
            parse("[z-a]").unwrap_err().kind,
            PatternErrorKind::InvalidCharacterClassRange,
        );

        assert_eq!(
            parse("[abc").unwrap_err().kind,
            PatternErrorKind::UnterminatedCharacterClass,
        );

        assert_eq!(
            parse(r"[\d-a]").unwrap_err().kind,
            PatternErrorKind::Unsupported('-')
        );

        assert_eq!(
            parse("[😀]").unwrap_err().kind,
            PatternErrorKind::Unsupported('😀'),
        );
    }

    #[test]
    fn defers_quantified_astral_literals() {
        assert_eq!(
            parse("😀*").unwrap_err().kind,
            PatternErrorKind::Unsupported('😀'),
        );

        assert!(parse("(?:😀)*").is_ok());
    }

    #[test]
    fn validates_counted_quantifiers() {
        assert_eq!(
            parse("a{4,3}").unwrap_err().kind,
            PatternErrorKind::InvalidQuantifierRange,
        );

        assert_eq!(
            parse("a{99999999999999999999999999999999}")
                .unwrap_err()
                .kind,
            PatternErrorKind::QuantifierTooLarge,
        );

        for input in ["a{}", "a{,2}", "a{1,2", "a{1,2,3}"] {
            assert_eq!(
                parse(input).unwrap_err().kind,
                PatternErrorKind::Unsupported('{'),
                "input: {input}",
            );
        }

        assert!(parse("a{1}").is_ok());
        assert!(parse("a{2,}").is_ok());
        assert!(parse("a{2,4}?").is_ok());
    }

    #[test]
    fn word_boundaries_are_not_quantifiable() {
        assert_eq!(
            parse(r"\b+").unwrap_err().kind,
            PatternErrorKind::Unsupported('+'),
        );

        assert_eq!(
            parse(r"\B{2}").unwrap_err().kind,
            PatternErrorKind::Unsupported('{'),
        );

        assert_eq!(
            parse(r"[\B]").unwrap_err().kind,
            PatternErrorKind::Unsupported('B'),
        );
    }

    #[test]
    fn lookahead_is_not_quantified_in_this_subset() {
        assert_eq!(
            parse("(?=a)*").unwrap_err().kind,
            PatternErrorKind::Unsupported('*'),
        );

        assert_eq!(
            parse("(?!a){2}").unwrap_err().kind,
            PatternErrorKind::Unsupported('{'),
        );

        assert_eq!(
            parse("(?=a").unwrap_err().kind,
            PatternErrorKind::UnterminatedGroup,
        );
    }
}
