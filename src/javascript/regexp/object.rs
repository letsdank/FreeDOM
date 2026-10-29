//! Internal RegExp state and span-only execution.
//!
//! <https://tc39.es/ecma262/#sec-regexpbuiltinexec>
//!
//! This is not yet a JavaScript object or a full exec result.

use super::{
    RegExpFlagError, RegExpFlags,
    matcher::{self, Match, MatchError},
    pattern::{self, Pattern, PatternError},
};

#[derive(Debug)]
pub(crate) struct RegExp {
    pattern: Pattern,
    flags: RegExpFlags,
    last_index: usize,
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

        Ok(Self {
            pattern,
            flags,
            last_index: 0,
        })
    }

    pub(crate) fn last_index(&self) -> usize {
        self.last_index
    }

    pub(crate) fn set_last_index(&mut self, value: usize) {
        self.last_index = value;
    }

    pub(crate) fn exec(&mut self, input: &[u16]) -> Result<Option<Match>, MatchError> {
        let stateful = self.flags.global || self.flags.sticky;

        let start = if stateful { self.last_index } else { 0 };

        let result = matcher::find_utf16(&self.pattern, input, start, self.flags)?;

        if stateful {
            self.last_index = result.map_or(0, |matched| matched.end);
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::{CompileError, RegExp};
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
}
