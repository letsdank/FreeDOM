//! Internal RegExp state and capture-aware execution.
//!
//! <https://tc39.es/ecma262/#sec-regexpbuiltinexec>
//!
//! This is not yet a JavaScript object or a full exec result.

use super::{
    RegExpFlagError, RegExpFlags,
    matcher::{self, CaptureMatch, Match, MatchError},
    pattern::{self, Pattern, PatternError},
};

#[derive(Debug)]
pub(crate) struct RegExp {
    pattern: Pattern,
    flags: RegExpFlags,
    last_index: usize,
    named_captures: Vec<(String, usize)>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct NamedCapture {
    pub(crate) name: String,
    pub(crate) index: usize,
    pub(crate) span: Option<Match>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ExecResult {
    pub(crate) captures: CaptureMatch,

    /// None means the pattern contains no named groups.
    pub(crate) named_groups: Option<Vec<NamedCapture>>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CompileError {
    Flags(RegExpFlagError),
    Pattern(PatternError),
    UnsupportedFlag(char),
}

impl RegExp {
    pub(crate) fn compile(source: &str, flags: &str) -> Result<Self, CompileError> {
        let flags = RegExpFlags::parse(flags).map_err(CompileError::Flags)?;

        for (enabled, flag) in [(flags.unicode, 'u'), (flags.unicode_sets, 'v')] {
            if enabled {
                return Err(CompileError::UnsupportedFlag(flag));
            }
        }

        let pattern = pattern::parse(source).map_err(CompileError::Pattern)?;

        let named_captures = pattern
            .named_captures()
            .into_iter()
            .map(|(name, index)| (name.to_owned(), index))
            .collect();

        Ok(Self {
            pattern,
            flags,
            last_index: 0,
            named_captures,
        })
    }

    pub(crate) fn last_index(&self) -> usize {
        self.last_index
    }

    pub(crate) fn uses_last_index(&self) -> bool {
        self.flags.global || self.flags.sticky
    }

    pub(crate) fn set_last_index(&mut self, value: usize) {
        self.last_index = value;
    }

    pub(crate) fn exec(&mut self, input: &[u16]) -> Result<Option<Match>, MatchError> {
        Ok(self.exec_captures(input)?.map(|result| result.span))
    }

    // This is the built-in matching path only; RegExpExec overrides are not
    // supported by the JavaScript runtime yet.
    // https://tc39.es/ecma262/#sec-regexp.prototype.test
    pub(crate) fn test(&mut self, input: &[u16]) -> Result<bool, MatchError> {
        Ok(self.exec(input)?.is_some())
    }

    pub(crate) fn exec_captures(
        &mut self,
        input: &[u16],
    ) -> Result<Option<CaptureMatch>, MatchError> {
        let stateful = self.flags.global || self.flags.sticky;
        let start = if stateful { self.last_index } else { 0 };
        let result = matcher::find_captures_utf16(&self.pattern, input, start, self.flags)?;

        if stateful {
            self.last_index = result.as_ref().map_or(0, |matched| matched.span.end);
        }

        Ok(result)
    }

    // RegExpBuiltinExec creates named group even if none of them matched.
    // https://tc39.es/ecma262/#sec-regexpbuiltinexec
    pub(crate) fn exec_result(&mut self, input: &[u16]) -> Result<Option<ExecResult>, MatchError> {
        let Some(captures) = self.exec_captures(input)? else {
            return Ok(None);
        };

        let named_groups = if self.named_captures.is_empty() {
            None
        } else {
            Some(
                self.named_captures
                    .iter()
                    .map(|(name, index)| NamedCapture {
                        name: name.clone(),
                        index: *index,
                        span: captures.groups[*index],
                    })
                    .collect(),
            )
        };

        Ok(Some(ExecResult {
            captures,
            named_groups,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{CompileError, NamedCapture, RegExp};
    use crate::javascript::regexp::matcher::{Match, MatchError};

    fn units(input: &str) -> Vec<u16> {
        input.encode_utf16().collect()
    }

    #[test]
    fn global_exec_updates_and_resets_last_index() {
        let mut re = RegExp::compile("a", "g").unwrap();
        let input = units("aba");

        assert_eq!(re.exec(&input).unwrap(), Some(Match { start: 0, end: 1 }),);
        assert_eq!(re.last_index, 1);

        assert_eq!(re.exec(&input).unwrap(), Some(Match { start: 2, end: 3 }),);
        assert_eq!(re.last_index(), 3);

        assert_eq!(re.exec(&input).unwrap(), None);
        assert_eq!(re.last_index(), 0);
    }

    #[test]
    fn sticky_starts_exactly_at_last_index() {
        let mut re = RegExp::compile("b", "y").unwrap();
        let input = units("ab");

        assert_eq!(re.exec(&input).unwrap(), None);

        re.set_last_index(1);

        assert_eq!(re.exec(&input).unwrap(), Some(Match { start: 1, end: 2 }),);
        assert_eq!(re.last_index(), 2);

        assert_eq!(re.exec(&input).unwrap(), None);
        assert_eq!(re.last_index(), 0);
    }

    #[test]
    fn stateless_exec_ignores_and_preserves_last_index() {
        let mut re = RegExp::compile("a", "").unwrap();
        re.set_last_index(42);

        assert_eq!(
            re.exec(&units("a")).unwrap(),
            Some(Match { start: 0, end: 1 })
        );
        assert_eq!(re.last_index(), 42);

        assert_eq!(re.exec(&units("b")).unwrap(), None);
        assert_eq!(re.last_index(), 42);
    }

    #[test]
    fn zero_length_global_match_does_not_advance_last_index() {
        let mut re = RegExp::compile("a*", "g").unwrap();
        re.set_last_index(1);

        for _ in 0..2 {
            assert_eq!(
                re.exec(&units("b")).unwrap(),
                Some(Match { start: 1, end: 1 }),
            );

            assert_eq!(re.last_index(), 1);
        }
    }

    #[test]
    fn uses_utf16_indices_and_rejects_unimplemented_modes() {
        let mut re = RegExp::compile(".", "g").unwrap();
        let input = units("😀");

        assert_eq!(re.exec(&input).unwrap(), Some(Match { start: 0, end: 1 }),);
        assert_eq!(re.exec(&input).unwrap(), Some(Match { start: 1, end: 2 }),);
        assert_eq!(re.last_index(), 2);

        assert!(matches!(
            RegExp::compile(".", "u"),
            Err(CompileError::UnsupportedFlag('u'))
        ));

        assert!(matches!(
            RegExp::compile(".", "gg"),
            Err(CompileError::Flags(_))
        ));
    }

    #[test]
    fn sticky_still_applies_when_global_is_also_set() {
        let mut re = RegExp::compile("b", "gy").unwrap();

        assert_eq!(re.exec(&units("ab")).unwrap(), None);

        re.set_last_index(1);

        assert_eq!(
            re.exec(&units("ab")).unwrap(),
            Some(Match { start: 1, end: 2 })
        );

        assert_eq!(re.last_index(), 2);
    }

    #[test]
    fn internal_match_errors_do_not_reset_last_index() {
        let mut re = RegExp::compile(&"(a|a)".repeat(18), "g").unwrap();
        let input = units(&format!("x{}", "a".repeat(18)));

        re.set_last_index(1);

        assert_eq!(re.exec(&input), Err(MatchError::StepLimit));
        assert_eq!(re.last_index(), 1);
    }

    #[test]
    fn checks_last_index_cases_from_test262() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/test262/test/built-ins/RegExp/prototype/exec");

        let global_source = std::fs::read_to_string(directory.join("failure-g-lastindex-reset.js"))
            .expect("Test262 fixture must exist");

        assert!(global_source.contains("var r = /a/g;"));
        assert!(global_source.contains("reset(42);"));

        let mut global = RegExp::compile("a", "g").unwrap();
        global.set_last_index(42);

        assert_eq!(global.exec(&units("abc")).unwrap(), None);
        assert_eq!(global.last_index(), 0);

        let sticky_source = std::fs::read_to_string(directory.join("y-fail-lastindex.js"))
            .expect("Test262 fixture must exist");

        assert!(sticky_source.contains("var r = /c/y;"));
        assert!(sticky_source.contains("r.lastIndex = 1;"));

        let mut sticky = RegExp::compile("c", "y").unwrap();
        sticky.set_last_index(1);

        assert_eq!(sticky.exec(&units("abc")).unwrap(), None);
        assert_eq!(sticky.last_index(), 0);
    }

    #[test]
    fn capture_exec_updates_and_resets_global_last_index() {
        let mut re = RegExp::compile("(a)(b)?", "g").unwrap();
        let input = units("a ab");

        let first = re.exec_captures(&input).unwrap().unwrap();

        assert_eq!(
            first.groups,
            vec![
                Some(Match { start: 0, end: 1 }),
                Some(Match { start: 0, end: 1 }),
                None,
            ]
        );
        assert_eq!(re.last_index(), 1);

        let second = re.exec_captures(&input).unwrap().unwrap();

        assert_eq!(
            second.groups,
            vec![
                Some(Match { start: 2, end: 4 }),
                Some(Match { start: 2, end: 3 }),
                Some(Match { start: 3, end: 4 }),
            ]
        );
        assert_eq!(re.last_index(), 4);

        assert_eq!(re.exec_captures(&input).unwrap(), None);
        assert_eq!(re.last_index(), 0);
    }

    #[test]
    fn capture_exec_preserves_unpaired_utf16_code_units() {
        let mut re = RegExp::compile(r"(\ud800)(.)?", "").unwrap();

        re.set_last_index(42);

        let input = [0x0061, 0xd800, 0x0062];
        let result = re.exec_captures(&input).unwrap().unwrap();

        assert_eq!(result.span, Match { start: 1, end: 3 });

        assert_eq!(
            result.groups,
            vec![
                Some(Match { start: 1, end: 3 }),
                Some(Match { start: 1, end: 2 }),
                Some(Match { start: 2, end: 3 }),
            ]
        );

        let captured = result.groups[1].unwrap();

        assert_eq!(&input[captured.start..captured.end], &[0xd800],);

        // Without g or y, execution preserves lastIndex.
        assert_eq!(re.last_index(), 42);
    }

    #[test]
    fn capture_exec_keeps_last_index_on_match_error() {
        let mut re = RegExp::compile(&"(a|a)".repeat(18), "g").unwrap();

        let input = units(&format!("x{}", "a".repeat(18)));

        re.set_last_index(1);

        assert_eq!(re.exec_captures(&input), Err(MatchError::StepLimit));
        assert_eq!(re.last_index(), 1);
    }

    #[test]
    fn exec_result_distinguishes_absent_unmatched_and_empty_groups() {
        let mut no_names = RegExp::compile("(a)", "").unwrap();

        let result = no_names.exec_result(&units("a")).unwrap().unwrap();

        assert_eq!(result.named_groups, None);
        assert_eq!(result.captures.groups[1], Some(Match { start: 0, end: 1 }));

        let mut re = RegExp::compile("(?<first>a)?(b)(?<last>c*)", "").unwrap();
        let result = re.exec_result(&units("b")).unwrap().unwrap();

        assert_eq!(result.captures.span, Match { start: 0, end: 1 });

        assert_eq!(
            result.named_groups,
            Some(vec![
                NamedCapture {
                    name: "first".into(),
                    index: 1,
                    span: None,
                },
                NamedCapture {
                    name: "last".into(),
                    index: 3,
                    span: Some(Match { start: 1, end: 1 })
                }
            ])
        );
    }

    #[test]
    fn exec_result_preserves_utf16_and_global_state() {
        let mut re = RegExp::compile(r"(?<unit>\ud800)", "g").unwrap();
        let input = [0xd800, 0x0061, 0xd800];

        let first = re.exec_result(&input).unwrap().unwrap();
        let group = &first.named_groups.as_ref().unwrap()[0];
        let span = group.span.unwrap();

        assert_eq!(&input[span.start..span.end], &[0xd800]);
        assert_eq!(re.last_index(), 1);

        let second = re.exec_result(&input).unwrap().unwrap();

        assert_eq!(
            second.named_groups.unwrap()[0].span,
            Some(Match { start: 2, end: 3 }),
        );
        assert_eq!(re.last_index(), 3);

        assert_eq!(re.exec_result(&input).unwrap(), None);
        assert_eq!(re.last_index(), 0);
    }

    #[test]
    fn named_groups_in_unmatched_alternatices_are_present() {
        let mut re = RegExp::compile(r"(?<left>a)|(?<right>b)", "").unwrap();

        let result = re.exec_result(&units("b")).unwrap().unwrap();

        assert_eq!(
            result.named_groups,
            Some(vec![
                NamedCapture {
                    name: "left".into(),
                    index: 1,
                    span: None,
                },
                NamedCapture {
                    name: "right".into(),
                    index: 2,
                    span: Some(Match { start: 0, end: 1 })
                }
            ])
        );
    }

    #[test]
    fn test_uses_the_same_global_last_index_as_exec() {
        let mut re = RegExp::compile("(?<letter>a)", "g").unwrap();
        let input = units("aba");

        assert_eq!(re.test(&input), Ok(true));
        assert_eq!(re.last_index(), 1);

        assert_eq!(re.exec(&input), Ok(Some(Match { start: 2, end: 3 })));
        assert_eq!(re.last_index(), 3);

        assert_eq!(re.test(&input), Ok(false));
        assert_eq!(re.last_index(), 0);
    }

    #[test]
    fn test_respects_sticky_and_stateless_modes() {
        let mut sticky = RegExp::compile("b", "y").unwrap();
        sticky.set_last_index(1);

        assert_eq!(sticky.test(&units("ab")), Ok(true));
        assert_eq!(sticky.last_index(), 2);

        assert_eq!(sticky.test(&units("ab")), Ok(false));
        assert_eq!(sticky.last_index(), 0);

        let mut plain = RegExp::compile("a", "").unwrap();
        plain.set_last_index(42);

        assert_eq!(plain.test(&units("a")), Ok(true));
        assert_eq!(plain.test(&units("b")), Ok(false));
        assert_eq!(plain.last_index(), 42);
    }

    #[test]
    fn test_keeps_empty_global_match_position() {
        let mut re = RegExp::compile("a*", "g").unwrap();
        re.set_last_index(1);

        for _ in 0..2 {
            assert_eq!(re.test(&units("b")), Ok(true));
            assert_eq!(re.last_index(), 1);
        }
    }

    #[test]
    fn test_accepts_utf16_code_units_and_propagates_errors() {
        let mut re = RegExp::compile(r"\ud800", "g").unwrap();

        assert_eq!(re.test(&[0xd800]), Ok(true));
        assert_eq!(re.last_index(), 1);

        let mut expensive = RegExp::compile(&"(a|a)".repeat(18), "g").unwrap();
        expensive.set_last_index(1);

        let input = units(&format!("x{}", "a".repeat(18)));

        assert_eq!(expensive.test(&input), Err(MatchError::StepLimit),);
        assert_eq!(expensive.last_index(), 1);
    }
}
