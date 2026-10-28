//! Partial RegExp matcher returning the span of the first match.
//!
//! <https://tc39.es/ecma262/#sec-runtime-semantics-compilepattern>
//!
//! Offsets are UTF-16 code units. Captures, lastIndex, and flags i/u/v are not
//! implemented here. The caller is responsible for global iteration.

use super::{
    RegExpFlags,
    pattern::{Pattern, PredefinedClass, Term},
};

const MAX_STEPS: usize = 200_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Match {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MatchError {
    UnsupportedFlag(char),
    StepLimit,
}

pub(crate) fn find(
    pattern: &Pattern,
    input: &str,
    from: usize,
    flags: RegExpFlags,
) -> Result<Option<Match>, MatchError> {
    for (enabled, flag) in [
        (flags.ignore_case, 'i'),
        (flags.unicode, 'u'),
        (flags.unicode_sets, 'v'),
    ] {
        if enabled {
            return Err(MatchError::UnsupportedFlag(flag));
        }
    }

    let units: Vec<u16> = input.encode_utf16().collect();

    if from > units.len() {
        return Ok(None);
    }

    let mut budget = MAX_STEPS;

    let last_start = if flags.sticky { from } else { units.len() };

    for start in from..=last_start {
        let ends = match_pattern(pattern, &units, start, flags, &mut budget)?;

        if let Some(end) = ends.first() {
            return Ok(Some(Match { start, end: *end }));
        }
    }

    Ok(None)
}

fn match_pattern(
    pattern: &Pattern,
    input: &[u16],
    start: usize,
    flags: RegExpFlags,
    budget: &mut usize,
) -> Result<Vec<usize>, MatchError> {
    let mut matches = Vec::new();

    for alternative in &pattern.alternatives {
        step(budget)?;

        let mut positions = vec![start];

        for term in alternative {
            let mut next = Vec::new();

            for position in positions {
                step(budget)?;

                match term {
                    Term::Literal(character) => {
                        let mut buffer = [0; 2];
                        let encoded = character.encode_utf16(&mut buffer);

                        if input
                            .get(position..)
                            .is_some_and(|rest| rest.starts_with(encoded))
                        {
                            next.push(position + encoded.len());
                        }
                    }

                    Term::Any => {
                        if input
                            .get(position)
                            .is_some_and(|unit| flags.dot_all || !is_line_terminator(*unit))
                        {
                            next.push(position + 1);
                        }
                    }

                    Term::CharacterClass { class, negated } => {
                        if let Some(&unit) = input.get(position) {
                            if class_matches(*class, unit) != *negated {
                                next.push(position + 1);
                            }
                        }
                    }

                    Term::Start => {
                        if position == 0
                            || (flags.multiline && is_line_terminator(input[position - 1]))
                        {
                            next.push(position);
                        }
                    }

                    Term::End => {
                        if position == input.len()
                            || (flags.multiline
                                && input
                                    .get(position)
                                    .is_some_and(|unit| is_line_terminator(*unit)))
                        {
                            next.push(position);
                        }
                    }

                    Term::Group { pattern, .. } => {
                        next.extend(match_pattern(pattern, input, position, flags, budget)?);
                    }
                }
            }

            positions = next;

            if positions.is_empty() {
                break;
            }
        }

        matches.extend(positions);
    }

    Ok(matches)
}

fn step(budget: &mut usize) -> Result<(), MatchError> {
    *budget = budget.checked_sub(1).ok_or(MatchError::StepLimit)?;

    Ok(())
}

fn is_line_terminator(unit: u16) -> bool {
    matches!(unit, 0x000A | 0x000D | 0x2028 | 0x2029)
}

// https://tc39.es/ecma262/#sec-runtime-semantics-compilecharacterclassescape
fn class_matches(class: PredefinedClass, unit: u16) -> bool {
    match class {
        PredefinedClass::Digit => {
            matches!(unit, 0x30..=0x39)
        }

        PredefinedClass::Word => {
            matches!(
                unit,
                0x30..=0x39|0x41..=0x5A|0x5F|0x61..=0x7A
            )
        }

        PredefinedClass::Space => {
            matches!(
                unit,
                0x0009..=0x000D
                | 0x0020
                | 0x00A0
                | 0x1680
                | 0x2000..=0x200A
                | 0x2028
                | 0x2029
                | 0x202F
                | 0x205F
                | 0x3000
                | 0xFEFF
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Match, MatchError, find};
    use crate::javascript::regexp::{RegExpFlags, pattern};

    fn search(
        pattern: &str,
        input: &str,
        from: usize,
        flags: &str,
    ) -> Result<Option<Match>, MatchError> {
        find(
            &pattern::parse(pattern).unwrap(),
            input,
            from,
            RegExpFlags::parse(flags).unwrap(),
        )
    }

    #[test]
    fn finds_literals_and_respects_utf16_offsets() {
        assert_eq!(
            search("b", "😀ab", 0, ""),
            Ok(Some(Match { start: 3, end: 4 })),
        );

        assert_eq!(
            search(".", "😀", 0, ""),
            Ok(Some(Match { start: 0, end: 1 })),
        );

        assert_eq!(
            search("😀", "😀", 0, ""),
            Ok(Some(Match { start: 0, end: 2 })),
        );

        assert_eq!(search("x", "abc", 0, ""), Ok(None));
        assert_eq!(search("a", "a", 2, ""), Ok(None));
    }

    #[test]
    fn respects_alternative_order_and_group_backtracking() {
        assert_eq!(
            search("a|ab", "ab", 0, ""),
            Ok(Some(Match { start: 0, end: 1 })),
        );

        assert_eq!(
            search("(a|ab)c", "abc", 0, ""),
            Ok(Some(Match { start: 0, end: 3 })),
        );

        assert_eq!(
            search("(ab|a)b", "ab", 0, ""),
            Ok(Some(Match { start: 0, end: 2 })),
        );

        assert_eq!(
            search("|a", "a", 0, ""),
            Ok(Some(Match { start: 0, end: 0 })),
        );
    }

    #[test]
    fn handles_anchors_dot_and_sticky() {
        assert_eq!(
            search("^b$", "a\nb\nc", 0, "m"),
            Ok(Some(Match { start: 2, end: 3 })),
        );

        assert_eq!(search("a.b", "a\nb", 0, ""), Ok(None));

        assert_eq!(
            search("a.b", "a\nb", 0, "s"),
            Ok(Some(Match { start: 0, end: 3 })),
        );

        assert_eq!(search("b", "ab", 0, "y"), Ok(None));

        assert_eq!(
            search("b", "ab", 1, "y"),
            Ok(Some(Match { start: 1, end: 2 }))
        );
    }

    #[test]
    fn rejects_modes_not_implemented_yet() {
        assert_eq!(
            search("a", "A", 0, "i"),
            Err(MatchError::UnsupportedFlag('i')),
        );

        assert_eq!(
            search(".", "😀", 0, "u"),
            Err(MatchError::UnsupportedFlag('u')),
        );

        assert_eq!(
            search(".", "x", 0, "v"),
            Err(MatchError::UnsupportedFlag('v')),
        );
    }

    #[test]
    fn limits_combinatorial_work() {
        let pattern = "(a|a)".repeat(18);
        let input = "a".repeat(18);

        assert_eq!(search(&pattern, &input, 0, ""), Err(MatchError::StepLimit),);
    }

    #[test]
    fn matches_predefined_character_classes() {
        for (pattern, input, expected) in [
            (r"^\d$", "5", true),
            (r"^\d$", "٥", false),
            (r"^\D$", "x", true),
            (r"^\D$", "5", false),
            (r"^\w$", "_", true),
            (r"^\w$", "é", false),
            (r"^\W$", "é", true),
            (r"^\W$", "A", false),
            (r"^\s$", "\u{FEFF}", true),
            (r"^\s$", "\u{0085}", false),
            (r"^\S$", "\u{0085}", true),
            (r"^\S$", "\n", false),
            (r"^\D$", "", false),
        ] {
            let result = search(pattern, input, 0, "").unwrap();

            assert_eq!(
                result.is_some(),
                expected,
                "pattern: {pattern:?}, input: {input:?}"
            );
        }
    }

    #[test]
    fn matches_ecmascript_whitespace_and_line_terminators() {
        for unit in [
            0x0009, 0x000A, 0x000B, 0x000C, 0x000D, 0x0020, 0x00A0, 0x1680, 0x2000, 0x2001, 0x2002,
            0x2003, 0x2004, 0x2005, 0x2006, 0x2007, 0x2008, 0x2009, 0x200A, 0x2028, 0x2029, 0x202F,
            0x205F, 0x3000, 0xFEFF,
        ] {
            let input = char::from_u32(unit).unwrap().to_string();
            let result = search(r"^\s$", &input, 0, "").unwrap();

            assert!(result.is_some(), "character: U+{unit:04X}");
        }
    }
}
