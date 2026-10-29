//! Partial parser for ECMAScript RegExp patterns.
//!
//! <https://tc39.es/ecma262/#sec-patterns>
//!
//! Legacy decimal escapes and Unicode modes are not implemented here yet.

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Pattern {
    pub(crate) alternatives: Vec<Vec<Term>>,
}

impl Pattern {
    pub(crate) fn capture_count(&self) -> usize {
        self.alternatives
            .iter()
            .flatten()
            .map(captures_in_term)
            .sum()
    }

    pub(crate) fn named_captures(&self) -> Vec<(&str, usize)> {
        fn collect<'a>(pattern: &'a Pattern, result: &mut Vec<(&'a str, usize)>) {
            fn visit<'a>(term: &'a Term, result: &mut Vec<(&'a str, usize)>) {
                match term {
                    Term::Group {
                        name,
                        capture_index,
                        pattern,
                    } => {
                        if let (Some(name), Some(index)) = (name, capture_index) {
                            result.push((name.as_str(), *index));
                        }

                        collect(pattern, result);
                    }

                    Term::Lookahead { pattern, .. } => {
                        collect(pattern, result);
                    }

                    Term::Repeat { term, .. } => {
                        visit(term, result);
                    }

                    _ => {}
                }
            }

            for term in pattern.alternatives.iter().flatten() {
                visit(term, result);
            }
        }

        let mut result = Vec::new();
        collect(self, &mut result);

        result
    }
}

fn captures_in_term(term: &Term) -> usize {
    match term {
        Term::Group {
            capture_index,
            pattern,
            ..
        } => usize::from(capture_index.is_some()) + pattern.capture_count(),

        Term::Lookahead { pattern, .. } => pattern.capture_count(),

        Term::Repeat { term, .. } => captures_in_term(term),

        _ => 0,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Term {
    Literal(char),
    CodeUnit(u16),
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
        capture_index: Option<usize>,
        name: Option<String>,
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
    Backreference {
        index: usize,

        /// Byte offset of the escape in the pattern source.
        offset: usize,
    },
    NamedBackreference {
        name: String,
        index: Option<usize>,
        offset: usize,
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
    CaptureLimit,
    UnsupportedDecimalEscape,
    UnsupportedGroupName,
    UnsupportedDuplicateGroupName,
    UnsupportedNamedReference,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PatternError {
    /// Byte offset relative to the beginning of the pattern.
    pub(crate) offset: usize,
    pub(crate) kind: PatternErrorKind,
}

pub(crate) fn parse(source: &str) -> Result<Pattern, PatternError> {
    let mut parser = Parser {
        source,
        offset: 0,
        next_capture_index: 1,
        group_names: Vec::new(),
    };

    let mut pattern = parser.disjunction(false, 0)?;

    let names: Vec<(String, usize)> = pattern
        .named_captures()
        .into_iter()
        .map(|(name, index)| (name.to_owned(), index))
        .collect();

    resolve_named_references(&mut pattern, &names)?;
    validate_backreferences(&pattern, parser.next_capture_index - 1)?;

    Ok(pattern)
}

// Named references may precede their group, so resolve them after parsing.
// https://tc39.es/ecma262/#sec-runtime-semantics-compileatom
fn resolve_named_references(
    pattern: &mut Pattern,
    names: &[(String, usize)],
) -> Result<(), PatternError> {
    fn resolve_term(term: &mut Term, names: &[(String, usize)]) -> Result<(), PatternError> {
        match term {
            Term::NamedBackreference {
                name,
                index,
                offset,
            } => {
                let found = names
                    .iter()
                    .find(|(candidate, _)| candidate == name)
                    .map(|(_, value)| *value)
                    .ok_or(PatternError {
                        offset: *offset,
                        kind: PatternErrorKind::UnsupportedNamedReference,
                    })?;

                *index = Some(found);

                Ok(())
            }

            Term::Group { pattern, .. } | Term::Lookahead { pattern, .. } => {
                resolve_named_references(pattern, names)
            }

            Term::Repeat { term, .. } => resolve_term(term, names),

            _ => Ok(()),
        }
    }

    for term in pattern.alternatives.iter_mut().flatten() {
        resolve_term(term, names)?;
    }

    Ok(())
}

// Decimal escapes that do not refer to a capturing group may use legacy
// syntax. That syntax is not supported yet; do not treat it as a backreference.
fn validate_backreferences(pattern: &Pattern, count: usize) -> Result<(), PatternError> {
    fn validate_term(term: &Term, count: usize) -> Result<(), PatternError> {
        match term {
            Term::Backreference { index, offset } if *index > count => Err(PatternError {
                offset: *offset,
                kind: PatternErrorKind::UnsupportedDecimalEscape,
            }),

            Term::Group { pattern, .. } | Term::Lookahead { pattern, .. } => {
                validate_backreferences(pattern, count)
            }

            Term::Repeat { term, .. } => validate_term(term, count),

            _ => Ok(()),
        }
    }

    for term in pattern.alternatives.iter().flatten() {
        validate_term(term, count)?;
    }

    Ok(())
}

struct Parser<'a> {
    source: &'a str,
    offset: usize,
    next_capture_index: usize,
    group_names: Vec<String>,
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
            Capturing { name: Option<String> },
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

                Some('<') if matches!(self.peek(), Some('=') | Some('!')) => {
                    return Err(PatternError {
                        offset: question_offset,
                        kind: PatternErrorKind::Unsupported('?'),
                    });
                }

                Some('<') => {
                    let name = self.group_name(group_start)?;

                    if self.group_names.contains(&name) {
                        return Err(PatternError {
                            offset: group_start,
                            kind: PatternErrorKind::UnsupportedDuplicateGroupName,
                        });
                    }

                    self.group_names.push(name.clone());

                    GroupKind::Capturing { name: Some(name) }
                }

                _ => {
                    return Err(PatternError {
                        offset: question_offset,
                        kind: PatternErrorKind::Unsupported('?'),
                    });
                }
            }
        } else {
            GroupKind::Capturing { name: None }
        };

        // ECMAScript numbers capturing groups by the position of their opening
        // parenthesis, including groups nested inside assertions.
        // https://tc39.es/ecma262/#sec-static-semantics-capturinggroupnumber
        let capture_index = if let GroupKind::Capturing { .. } = &kind {
            let index = self.next_capture_index;

            if index >= u32::MAX as usize {
                return Err(PatternError {
                    offset: group_start,
                    kind: PatternErrorKind::CaptureLimit,
                });
            }

            self.next_capture_index += 1;
            Some(index)
        } else {
            None
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
            GroupKind::Capturing { name } => Ok(Term::Group {
                capture_index,
                name,
                pattern: Box::new(pattern),
            }),

            GroupKind::NonCapturing => Ok(Term::Group {
                capture_index: None,
                name: None,
                pattern: Box::new(pattern),
            }),

            GroupKind::Lookahead { negated } => Ok(Term::Lookahead {
                negated,
                pattern: Box::new(pattern),
            }),
        }
    }

    // This is the ASCII subset of RegExpIdentifierName.
    // https://tc39.es/ecma262/#prod-RegExpIdentifierName
    fn group_name(&mut self, start: usize) -> Result<String, PatternError> {
        let name_start = self.offset;

        let valid_start = self.peek().is_some_and(|character| {
            character.is_ascii_alphabetic() || character == '_' || character == '$'
        });

        if !valid_start {
            return Err(PatternError {
                offset: start,
                kind: PatternErrorKind::UnsupportedGroupName,
            });
        }

        self.advance();

        while self.peek().is_some_and(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '$'
        }) {
            self.advance();
        }

        if self.peek() != Some('>') {
            return Err(PatternError {
                offset: start,
                kind: PatternErrorKind::UnsupportedGroupName,
            });
        }

        let name = self.source[name_start..self.offset].to_owned();
        self.advance();

        Ok(name)
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
            'x' => return self.hex_escape(escape_start, character, 2),
            'u' => return self.hex_escape(escape_start, character, 4),
            'c' => return self.control_escape(escape_start),

            '0' => {
                if self.peek().is_some_and(|next| next.is_ascii_digit()) {
                    return Err(PatternError {
                        offset: escape_start,
                        kind: PatternErrorKind::Unsupported('0'),
                    });
                }

                return Ok(Term::CodeUnit(0));
            }

            '1'..='9' => {
                let mut index = (character as u8 - b'0') as usize;

                while let Some(digit) = self.peek().filter(char::is_ascii_digit) {
                    self.advance();

                    index = index
                        .checked_mul(10)
                        .and_then(|value| value.checked_add((digit as u8 - b'0') as usize))
                        .ok_or(PatternError {
                            offset: escape_start,
                            kind: PatternErrorKind::UnsupportedDecimalEscape,
                        })?;
                }

                return Ok(Term::Backreference {
                    index,
                    offset: escape_start,
                });
            }

            'k' => {
                if self.peek() != Some('<') {
                    return Err(PatternError {
                        offset: escape_start,
                        kind: PatternErrorKind::Unsupported('k'),
                    });
                }

                self.advance();

                let name = self.group_name(escape_start)?;

                return Ok(Term::NamedBackreference {
                    name,
                    index: None,
                    offset: escape_start,
                });
            }

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

    fn hex_escape(&mut self, start: usize, kind: char, count: usize) -> Result<Term, PatternError> {
        let error = || PatternError {
            offset: start,
            kind: PatternErrorKind::Unsupported(kind),
        };

        let bytes = self
            .source
            .as_bytes()
            .get(self.offset..self.offset + count)
            .ok_or_else(error)?;

        let mut value = 0_u16;

        for &byte in bytes {
            let digit = (byte as char).to_digit(16).ok_or_else(error)?;
            value = (value << 4) | digit as u16;
        }

        self.offset += count;

        Ok(Term::CodeUnit(value))
    }

    fn control_escape(&mut self, start: usize) -> Result<Term, PatternError> {
        let Some(letter) = self.peek().filter(char::is_ascii_alphabetic) else {
            return Err(PatternError {
                offset: start,
                kind: PatternErrorKind::Unsupported('c'),
            });
        };

        self.advance();

        Ok(Term::CodeUnit((letter as u16) % 32))
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
            } else if self.peek() == Some('k') {
                return Err(PatternError {
                    offset,
                    kind: PatternErrorKind::Unsupported('k'),
                });
            } else if self
                .peek()
                .is_some_and(|character| matches!(character, '1'..='9'))
            {
                return Err(PatternError {
                    offset,
                    kind: PatternErrorKind::UnsupportedDecimalEscape,
                });
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

            Term::CodeUnit(unit) => Ok(ClassItem::Range {
                start: unit,
                end: unit,
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
                        capture_index: Some(1),
                        name: None,
                        pattern: Box::new(Pattern {
                            alternatives: vec![
                                vec![Term::Literal('a')],
                                vec![Term::Group {
                                    capture_index: None,
                                    name: None,
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

    #[test]
    fn parses_hexadecimal_code_unit_escapes() {
        let pattern = parse(r"\x41\u0042\ud83d").unwrap();

        assert_eq!(
            pattern.alternatives,
            vec![vec![
                Term::CodeUnit(0x0041),
                Term::CodeUnit(0x0042),
                Term::CodeUnit(0xD83D),
            ]],
        );

        for (input, kind) in [
            (r"\x4", 'x'),
            (r"\xZZ", 'x'),
            (r"\u123", 'u'),
            (r"\u{41}", 'u'),
        ] {
            assert_eq!(
                parse(input).unwrap_err().kind,
                PatternErrorKind::Unsupported(kind),
                "input: {input}"
            );
        }
    }

    #[test]
    fn parses_null_and_control_escapes() {
        let pattern = parse(r"\0\cA\ca\cZ").unwrap();

        assert_eq!(
            pattern.alternatives,
            vec![vec![
                Term::CodeUnit(0),
                Term::CodeUnit(1),
                Term::CodeUnit(1),
                Term::CodeUnit(26),
            ]]
        );

        for (input, kind) in [
            (r"\01", '0'),
            (r"\09", '0'),
            (r"\c", 'c'),
            (r"\c7", 'c'),
            (r"\cÉ", 'c'),
        ] {
            assert_eq!(
                parse(input).unwrap_err().kind,
                PatternErrorKind::Unsupported(kind),
                "input: {input:?}"
            );
        }
    }

    #[test]
    fn numbers_captures_by_opening_parenthesis() {
        fn collect(pattern: &Pattern, indices: &mut Vec<usize>) {
            fn visit(term: &Term, indices: &mut Vec<usize>) {
                match term {
                    Term::Group {
                        capture_index,
                        pattern,
                        ..
                    } => {
                        if let Some(index) = capture_index {
                            indices.push(*index);
                        }

                        collect(pattern, indices);
                    }

                    Term::Lookahead { pattern, .. } => {
                        collect(pattern, indices);
                    }

                    Term::Repeat { term, .. } => {
                        visit(term, indices);
                    }

                    _ => {}
                }
            }

            for term in pattern.alternatives.iter().flatten() {
                visit(term, indices);
            }
        }

        let pattern = parse("((a)|(?:b)(?=(c))(?!((d))))|(e)").unwrap();
        let mut indices = Vec::new();

        collect(&pattern, &mut indices);

        assert_eq!(indices, vec![1, 2, 3, 4, 5, 6]);
        assert_eq!(pattern.capture_count(), 6);
        assert_eq!(parse("(?:a)(b)+").unwrap().capture_count(), 1);
        assert_eq!(parse("(?:a)").unwrap().capture_count(), 0);
    }

    #[test]
    fn validates_decimal_backreferences_after_parsing_all_groups() {
        assert!(parse(r"\1(a)").is_ok());
        assert!(parse(r"(a)(b)(c)(d)(e)(f)(g)(h)(i)(j)\10").is_ok());
        assert!(parse(r"(a)(b)(c)(d)(e)(f)(g)(h)(i)\9").is_ok());

        for source in [
            r"\1",
            r"(a)\2",
            r"(a)\12",
            r"[\1]",
            r"\9",
            r"\9999999999999999999999999999999(a)",
        ] {
            let error = parse(source).unwrap_err();

            assert_eq!(
                error.kind,
                PatternErrorKind::UnsupportedDecimalEscape,
                "pattern: {source:?}"
            );
        }
    }

    #[test]
    fn parses_and_numbers_ascii_named_groups() {
        let pattern = parse(r"(?<outer>a(?<inner>b))|(?=(?<look>c))(?<$last_1>d)+").unwrap();

        assert_eq!(pattern.capture_count(), 4);

        assert_eq!(
            pattern.named_captures(),
            vec![("outer", 1), ("inner", 2), ("look", 3), ("$last_1", 4),]
        );

        assert!(parse(r"(?<letter>a)\1").is_ok());
    }

    #[test]
    fn reports_unsupported_group_names_and_duplicates() {
        for source in [r"(?<>a)", r"(?<1a>a)", r"(?<é>a)", r"(?<a-b>a)", r"(?<name"] {
            assert_eq!(
                parse(source).unwrap_err().kind,
                PatternErrorKind::UnsupportedGroupName,
                "{source:?}"
            );
        }

        for source in [r"(?<x>a)(?<x>b)", r"(?<x>a)|(?<x>b)"] {
            assert_eq!(
                parse(source).unwrap_err().kind,
                PatternErrorKind::UnsupportedDuplicateGroupName,
                "{source:?}"
            );
        }

        // Lookbehind is not supported yet.
        assert_eq!(
            parse(r"(?<=a)b").unwrap_err().kind,
            PatternErrorKind::Unsupported('?'),
        );
    }

    #[test]
    fn resolves_named_references_across_the_whole_pattern() {
        let pattern =
            parse(r"\k<later>(?<first>a)(?=(?:\k<later>))(?<later>b)(?:\k<first>)+").unwrap();

        assert_eq!(pattern.named_captures(), vec![("first", 1), ("later", 2)]);

        assert_eq!(
            pattern.alternatives[0][0],
            Term::NamedBackreference {
                name: "later".to_owned(),
                index: Some(2),
                offset: 0,
            }
        );

        for source in [
            r"\k<unknown>",
            r"(?<a>x)\k<b>",
            r"(?<a>x)(?=\k<b>)",
            r"(?<a>x)(?:\k<b>)+",
        ] {
            assert_eq!(
                parse(source).unwrap_err().kind,
                PatternErrorKind::UnsupportedNamedReference,
                "{source:?}"
            );
        }

        assert_eq!(
            parse(r"[\k<x>]").unwrap_err().kind,
            PatternErrorKind::Unsupported('k'),
        );
    }
}
