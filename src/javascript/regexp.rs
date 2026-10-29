//! Scanning JavaScript regular expression literals.
//!
//! Lexical grammar:
//! <https://tc39.es/ecma262/#sec-literals-regular-expression-literals>
//!
//! The literal scanner does not parse patterns. Flag scanning is currently
//! limited to ASCII identifier characters; validation is a separate step.

#[path = "regexp/flags.rs"]
mod flags;

#[path = "regexp/case.rs"]
mod case;

#[path = "regexp/pattern.rs"]
pub(crate) mod pattern;

#[path = "regexp/matcher.rs"]
pub(crate) mod matcher;

pub(crate) use flags::{RegExpFlagError, RegExpFlags};

use super::lexer::LexError;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RegExpLiteral<'a> {
    pub(crate) pattern: &'a str,
    pub(crate) flags: &'a str,
    pub(crate) end: usize,
}

impl RegExpLiteral<'_> {
    pub(crate) fn parse_flags(&self) -> Result<RegExpFlags, RegExpFlagError> {
        RegExpFlags::parse(self.flags)
    }

    pub(crate) fn parse_pattern(&self) -> Result<pattern::Pattern, pattern::PatternError> {
        pattern::parse(self.pattern)
    }
}

pub(crate) fn scan_literal(source: &str, start: usize) -> Result<RegExpLiteral<'_>, LexError> {
    let Some(rest) = source.get(start..) else {
        return Err(error(start, "invalid regular expression start"));
    };

    if !rest.starts_with('/') {
        return Err(error(start, "expected '/'"));
    }

    let pattern_start = start + 1;
    let pattern_source = &source[pattern_start..];

    if pattern_source.starts_with('/') || pattern_source.starts_with('*') {
        return Err(error(start, "expected regular expression, found comment"));
    }

    let mut in_class = false;
    let mut escaped = false;

    for (relative_offset, character) in pattern_source.char_indices() {
        let offset = pattern_start + relative_offset;

        if matches!(character, '\n' | '\r' | '\u{2028}' | '\u{2029}') {
            return Err(error(offset, "line terminator in regular expression"));
        }

        if escaped {
            escaped = false;
            continue;
        }

        match character {
            '\\' => escaped = true,
            '[' if !in_class => in_class = true,
            ']' if in_class => in_class = false,

            '/' if !in_class => {
                let flags_start = offset + 1;
                let mut end = flags_start;

                while let Some(character) = source[end..].chars().next() {
                    if character.is_ascii_alphanumeric() || matches!(character, '_' | '$') {
                        end += character.len_utf8();
                    } else {
                        break;
                    }
                }

                return Ok(RegExpLiteral {
                    pattern: &source[pattern_start..offset],
                    flags: &source[flags_start..end],
                    end,
                });
            }

            _ => {}
        }
    }

    Err(error(start, "unterminated regular expression"))
}

fn error(offset: usize, message: &str) -> LexError {
    LexError {
        offset,
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::scan_literal;

    #[test]
    fn scans_escapes_character_classes_and_ascii_flags() {
        let source = r#"let re = /a\/b[/-]c/gi; 8 / 2"#;
        let start = source.find("/a").unwrap();

        let literal = scan_literal(source, start).unwrap();

        assert_eq!(literal.pattern, r"a\/b[/-]c");
        assert_eq!(literal.flags, "gi");
        assert_eq!(literal.parse_flags().unwrap().to_canonical_string(), "gi");
        assert_eq!(&source[literal.end..], "; 8 / 2");
    }

    #[test]
    fn uses_byte_offsets_for_unicode() {
        let source = "α /é/gi;";
        let start = source.find('/').unwrap();

        let literal = scan_literal(source, start).unwrap();

        assert_eq!(literal.pattern, "é");
        assert_eq!(literal.flags, "gi");
        assert_eq!(&source[literal.end..], ";");

        // Byte 1 is inside the UTF-8 representation of α.
        assert!(scan_literal(source, 1).is_err());
    }

    #[test]
    fn rejects_unterminated_literals_and_line_terminators() {
        for source in [
            "/abc",
            "/[abc/",
            "/abc\\",
            "/a\nb/",
            "/a\\\nb/",
            "/a\u{2028}b/",
        ] {
            assert!(scan_literal(source, 0).is_err(), "source: {source:?}");
        }

        for source in ["// comment", "/* comment */", "abc"] {
            assert!(scan_literal(source, 0).is_err(), "source: {source:?}");
        }
    }

    #[test]
    fn scanning_and_flag_validation_are_separate() {
        let literal = scan_literal("/a/gg;", 0).unwrap();

        assert_eq!(literal.flags, "gg");
        assert!(literal.parse_flags().is_err());
    }
}
