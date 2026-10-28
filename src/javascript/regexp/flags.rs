//! RegExp flag validation and canonical ordering.
//!
//! <https://tc39.es/ecma262/#sec-regexpinitialize>
//! <https://tc39.es/ecma262/#sec-get-regexp.prototype.flags>

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RegExpFlags {
    pub(crate) has_indices: bool,
    pub(crate) global: bool,
    pub(crate) ignore_case: bool,
    pub(crate) multiline: bool,
    pub(crate) dot_all: bool,
    pub(crate) unicode: bool,
    pub(crate) unicode_sets: bool,
    pub(crate) sticky: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RegExpFlagErrorKind {
    Invalid(char),
    Duplicate(char),
    UnicodeModesConflict,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RegExpFlagError {
    /// Byte offset relative to the beginning of the flag string.
    pub(crate) offset: usize,
    pub(crate) kind: RegExpFlagErrorKind,
}

impl RegExpFlags {
    pub(crate) fn parse(input: &str) -> Result<Self, RegExpFlagError> {
        let mut flags = Self::default();

        for (offset, character) in input.char_indices() {
            let slot = match character {
                'd' => &mut flags.has_indices,
                'g' => &mut flags.global,
                'i' => &mut flags.ignore_case,
                'm' => &mut flags.multiline,
                's' => &mut flags.dot_all,
                'u' => &mut flags.unicode,
                'v' => &mut flags.unicode_sets,
                'y' => &mut flags.sticky,

                _ => {
                    return Err(RegExpFlagError {
                        offset,
                        kind: RegExpFlagErrorKind::Invalid(character),
                    });
                }
            };

            if *slot {
                return Err(RegExpFlagError {
                    offset,
                    kind: RegExpFlagErrorKind::Duplicate(character),
                });
            }

            *slot = true;

            if flags.unicode && flags.unicode_sets {
                return Err(RegExpFlagError {
                    offset,
                    kind: RegExpFlagErrorKind::UnicodeModesConflict,
                });
            }
        }

        Ok(flags)
    }

    pub(crate) fn to_canonical_string(self) -> String {
        let mut output = String::new();

        for (enabled, character) in [
            (self.has_indices, 'd'),
            (self.global, 'g'),
            (self.ignore_case, 'i'),
            (self.multiline, 'm'),
            (self.dot_all, 's'),
            (self.unicode, 'u'),
            (self.unicode_sets, 'v'),
            (self.sticky, 'y'),
        ] {
            if enabled {
                output.push(character);
            }
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use super::{RegExpFlagError, RegExpFlagErrorKind, RegExpFlags};

    #[test]
    fn parses_and_orders_all_flags() {
        let flags = RegExpFlags::parse("ysumigd").unwrap();

        assert!(flags.has_indices);
        assert!(flags.global);
        assert!(flags.ignore_case);
        assert!(flags.multiline);
        assert!(flags.dot_all);
        assert!(flags.unicode);
        assert!(!flags.unicode_sets);
        assert!(flags.sticky);
        assert_eq!(flags.to_canonical_string(), "dgimsuy");

        let flags = RegExpFlags::parse("ygvidms").unwrap();

        assert_eq!(flags.to_canonical_string(), "dgimsvy");
        assert_eq!(RegExpFlags::parse("").unwrap().to_canonical_string(), "");
    }

    #[test]
    fn rejects_unknown_duplicate_and_incompatible_flags() {
        for (input, offset, kind) in [
            ("igx", 2, RegExpFlagErrorKind::Invalid('x')),
            ("gG", 1, RegExpFlagErrorKind::Invalid('G')),
            ("gg", 1, RegExpFlagErrorKind::Duplicate('g')),
            ("gii", 2, RegExpFlagErrorKind::Duplicate('i')),
            ("uv", 1, RegExpFlagErrorKind::UnicodeModesConflict),
            ("vu", 1, RegExpFlagErrorKind::UnicodeModesConflict),
            ("gé", 1, RegExpFlagErrorKind::Invalid('é')),
        ] {
            assert_eq!(
                RegExpFlags::parse(input).unwrap_err(),
                RegExpFlagError { offset, kind },
                "input: {input:?}"
            );
        }
    }
}
