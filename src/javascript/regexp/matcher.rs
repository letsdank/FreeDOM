//! Partial RegExp matcher returning the span of the first match.
//!
//! <https://tc39.es/ecma262/#sec-runtime-semantics-compilepattern>
//!
//! Offsets are UTF-16 code units. Captures, lastIndex, and flags i/u/v are not
//! implemented here. The caller is responsible for global iteration.

use super::{
    RegExpFlags,
    pattern::{ClassItem, Pattern, PredefinedClass, Term},
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

                    Term::BracketClass { negated, items } => {
                        if let Some(&unit) = input.get(position) {
                            let included = items.iter().any(|item| match item {
                                ClassItem::Range { start, end } => (*start..=*end).contains(&unit),

                                ClassItem::Predefined { class, negated } => {
                                    class_matches(*class, unit) != *negated
                                }
                            });

                            if included != *negated {
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

                    Term::Repeat {
                        term,
                        min,
                        max,
                        greedy,
                    } => {
                        next.extend(match_repeat(
                            term, *min, *max, *greedy, input, position, flags, budget,
                        )?);
                    }

                    Term::WordBoundary { negated } => {
                        let left_word = position
                            .checked_sub(1)
                            .and_then(|index| input.get(index))
                            .is_some_and(|&unit| class_matches(PredefinedClass::Word, unit));

                        let right_word = input
                            .get(position)
                            .is_some_and(|&unit| class_matches(PredefinedClass::Word, unit));

                        let boundary = left_word != right_word;

                        if boundary != *negated {
                            next.push(position);
                        }
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

fn match_repeat(
    term: &Term,
    min: usize,
    max: Option<usize>,
    greedy: bool,
    input: &[u16],
    start: usize,
    flags: RegExpFlags,
    budget: &mut usize,
) -> Result<Vec<usize>, MatchError> {
    enum Frame {
        Visit { position: usize, count: usize },
        Emit(usize),
    }

    let atom = Pattern {
        alternatives: vec![vec![term.clone()]],
    };

    let mut stack = vec![Frame::Visit {
        position: start,
        count: 0,
    }];

    let mut matches = Vec::new();

    while let Some(frame) = stack.pop() {
        step(budget)?;

        let (position, count) = match frame {
            Frame::Emit(position) => {
                matches.push(position);
                continue;
            }

            Frame::Visit { position, count } => (position, count),
        };

        if count >= min {
            if greedy {
                stack.push(Frame::Emit(position));
            } else {
                matches.push(position);
            }
        }

        if max.is_some_and(|limit| count >= limit) {
            continue;
        }

        let ends = match_pattern(&atom, input, position, flags, budget)?;

        for end in ends.into_iter().rev() {
            // An empty match must not repeat indefinitely.
            if end == position && count >= min {
                continue;
            }

            stack.push(Frame::Visit {
                position: end,
                count: count + 1,
            });
        }
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

    #[test]
    fn matches_bracket_classes_and_ranges() {
        for (pattern, input, expected) in [
            (r"^[a-z]$", "m", true),
            (r"^[a-z]$", "M", false),
            (r"^[^0-9]$", "x", true),
            (r"^[^0-9]$", "3", false),
            (r"^[\s_]$", "\n", true),
            (r"^[\s_]$", "_", true),
            (r"^[\D]$", "x", true),
            (r"^[\D]$", "7", false),
            (r"^[-a]$", "-", true),
            (r"^[a-]$", "-", true),
            (r"^[\b]$", "\u{0008}", true),
            (r"^[\]]$", "]", true),
            (r"^[]$", "x", false),
            (r"^[^]$", "\n", true),
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
    fn matches_greedy_and_lazy_quantifiers() {
        for (pattern, input, expected) in [
            ("a*", "aaab", Some((0, 3))),
            ("a*?", "aaab", Some((0, 0))),
            ("a+", "aaab", Some((0, 3))),
            ("a+?", "aaab", Some((0, 1))),
            ("a?", "aaab", Some((0, 1))),
            ("a??", "aaab", Some((0, 0))),
            ("a*b", "aaab", Some((0, 4))),
            ("a*ab", "aaab", Some((0, 4))),
            ("a*?b", "aaab", Some((0, 4))),
            ("(ab|a)+b", "aab", Some((0, 3))),
            ("a+", "bbb", None),
            ("(a|)+b", "b", Some((0, 1))),
            ("()+", "", Some((0, 0))),
        ] {
            let result = search(pattern, input, 0, "")
                .unwrap()
                .map(|matched| (matched.start, matched.end));

            assert_eq!(result, expected, "pattern: {pattern:?}, input: {input:?}");
        }
    }

    #[test]
    fn matches_counted_quantifiers() {
        for (pattern, input, expected) in [
            ("a{2}", "aaaa", Some((0, 2))),
            ("a{0}", "aaa", Some((0, 0))),
            ("a{2,}", "aaaa", Some((0, 4))),
            ("a{2,3}", "aaaa", Some((0, 3))),
            ("a{2,3}?", "aaaa", Some((0, 2))),
            ("a{2,3}?b", "aaab", Some((0, 4))),
            ("a{2,3}ab", "aaaab", Some((0, 5))),
            ("(?:ab){2}", "abab", Some((0, 4))),
            ("(?:a|){2}", "a", Some((0, 1))),
            ("a{2}", "a", None),
        ] {
            let result = search(pattern, input, 0, "")
                .unwrap()
                .map(|matched| (matched.start, matched.end));

            assert_eq!(result, expected, "pattern: {pattern:?}, input: {input:?}");
        }
    }

    #[test]
    fn matches_counted_quantifier_from_test262_fixture() {
        use crate::javascript::regexp::scan_literal;

        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/test262/test/built-ins/RegExp/S15.10.2.5_A1_T1.js",
        );

        let source = std::fs::read_to_string(path).expect("Test262 fixture must exist");

        assert!(source.contains(r#"var __executed = /a[a-z]{2,4}/.exec("abcdefghi");"#));
        assert!(source.contains(r#"var __expected = ["abcde"];"#));

        let start = source
            .find("/a[a-z]{2,4}")
            .expect("Test262 regexp must exist");

        let literal = scan_literal(&source, start).unwrap();
        let pattern = literal.parse_pattern().unwrap();
        let flags = literal.parse_flags().unwrap();

        let matched = find(&pattern, "abcdefghi", 0, flags).unwrap();

        assert_eq!(matched, Some(Match { start: 0, end: 5 }),);
    }

    #[test]
    fn matches_word_boundaries_without_consuming_characters() {
        for (pattern, input, expected) in [
            (r"\bcat\b", "a cat!", Some((2, 5))),
            (r"\bcat\b", "concatenate", None),
            (r"\Bcat\B", "scats", Some((1, 4))),
            (r"\B", "", Some((0, 0))),
            (r"\b", "", None),
            (r"\b", "éa", Some((1, 1))),
            (r"\b_\b", "_", Some((0, 1))),
            (r"^[\b]$", "\u{0008}", Some((0, 1))),
        ] {
            let result = search(pattern, input, 0, "")
                .unwrap()
                .map(|matched| (matched.start, matched.end));

            assert_eq!(result, expected, "pattern: {pattern:?}, input: {input:?}");
        }
    }

    #[test]
    fn matches_word_boundaries_from_test262_fixtures() {
        use crate::javascript::regexp::scan_literal;

        for (file, fragment, input, expected) in [
            (
                "S15.10.2.6_A3_T2.js",
                r"/ot\b/",
                "pilot\nsoviet robot\topenoffice",
                (3, 5),
            ),
            (
                "S15.10.2.6_A4_T1.js",
                r"/\Bevil\B/",
                "devils arise\tfor\nevil",
                (1, 5),
            ),
        ] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("testdata/test262/test/built-ins/RegExp")
                .join(file);

            let source = std::fs::read_to_string(path).expect("Test262 fixture must exist");
            let start = source.find(fragment).expect("Test262 regexp must exist");
            let literal = scan_literal(&source, start).unwrap();
            let pattern = literal.parse_pattern().unwrap();
            let flags = literal.parse_flags().unwrap();

            let result = find(&pattern, input, 0, flags).unwrap();

            assert_eq!(
                result,
                Some(Match {
                    start: expected.0,
                    end: expected.1,
                }),
                "fixture: {file}",
            );
        }
    }
}
