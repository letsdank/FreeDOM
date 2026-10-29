//! Partial RegExp matcher returning the span of the first match.
//!
//! <https://tc39.es/ecma262/#sec-runtime-semantics-compilepattern>
//!
//! Offsets are UTF-16 code units. Flags u/v are not implemented here.
//! The caller is responsible for global iteration.

use super::case::{code_units_equal, range_contains};
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CaptureMatch {
    pub(crate) span: Match,

    /// Index 0 is the whole match; groups start at index 1.
    pub(crate) groups: Vec<Option<Match>>,
}

#[derive(Clone)]
struct State {
    position: usize,
    groups: Vec<Option<Match>>,
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
    let units: Vec<u16> = input.encode_utf16().collect();

    find_utf16(pattern, &units, from, flags)
}

pub(crate) fn find_utf16(
    pattern: &Pattern,
    units: &[u16],
    from: usize,
    flags: RegExpFlags,
) -> Result<Option<Match>, MatchError> {
    Ok(find_captures_utf16(pattern, units, from, flags)?.map(|result| result.span))
}

pub(crate) fn find_captures_utf16(
    pattern: &Pattern,
    units: &[u16],
    from: usize,
    flags: RegExpFlags,
) -> Result<Option<CaptureMatch>, MatchError> {
    for (enabled, flag) in [(flags.unicode, 'u'), (flags.unicode_sets, 'v')] {
        if enabled {
            return Err(MatchError::UnsupportedFlag(flag));
        }
    }

    if from > units.len() {
        return Ok(None);
    }

    let mut budget = MAX_STEPS;
    let last_start = if flags.sticky { from } else { units.len() };

    for start in from..=last_start {
        let state = State {
            position: start,
            groups: vec![None; pattern.capture_count() + 1],
        };

        let matches = match_pattern(pattern, units, state, flags, &mut budget)?;

        if let Some(mut state) = matches.into_iter().next() {
            let span = Match {
                start,
                end: state.position,
            };

            state.groups[0] = Some(span);

            return Ok(Some(CaptureMatch {
                span,
                groups: state.groups,
            }));
        }
    }

    Ok(None)
}

fn match_pattern(
    pattern: &Pattern,
    input: &[u16],
    start: State,
    flags: RegExpFlags,
    budget: &mut usize,
) -> Result<Vec<State>, MatchError> {
    let mut matches = Vec::new();

    for alternative in &pattern.alternatives {
        step(budget)?;

        let mut positions = vec![start.clone()];

        for term in alternative {
            let mut next = Vec::new();

            for state in positions {
                step(budget)?;

                let position = state.position;

                let mut push_position = |new_position| {
                    let mut next_state = state.clone();
                    next_state.position = new_position;
                    next.push(next_state);
                };

                match term {
                    Term::Literal(character) => {
                        let mut buffer = [0; 2];
                        let encoded = character.encode_utf16(&mut buffer);

                        if input.get(position..).is_some_and(|rest| {
                            rest.get(..encoded.len()).is_some_and(|candidate| {
                                candidate.iter().zip(encoded.iter()).all(|(&left, &right)| {
                                    code_units_equal(left, right, flags.ignore_case)
                                })
                            })
                        }) {
                            push_position(position + encoded.len());
                        }
                    }

                    Term::CodeUnit(unit) => {
                        if input.get(position).is_some_and(|&actual| {
                            code_units_equal(actual, *unit, flags.ignore_case)
                        }) {
                            push_position(position + 1);
                        }
                    }

                    Term::Any => {
                        if input
                            .get(position)
                            .is_some_and(|unit| flags.dot_all || !is_line_terminator(*unit))
                        {
                            push_position(position + 1);
                        }
                    }

                    Term::CharacterClass { class, negated } => {
                        if let Some(&unit) = input.get(position) {
                            if class_matches(*class, unit) != *negated {
                                push_position(position + 1);
                            }
                        }
                    }

                    Term::BracketClass { negated, items } => {
                        if let Some(&unit) = input.get(position) {
                            let included = items.iter().any(|item| match item {
                                ClassItem::Range { start, end } => {
                                    range_contains(*start, *end, unit, flags.ignore_case)
                                }

                                ClassItem::Predefined { class, negated } => {
                                    class_matches(*class, unit) != *negated
                                }
                            });

                            if included != *negated {
                                push_position(position + 1);
                            }
                        }
                    }

                    Term::Start => {
                        if position == 0
                            || (flags.multiline && is_line_terminator(input[position - 1]))
                        {
                            push_position(position);
                        }
                    }

                    Term::End => {
                        if position == input.len()
                            || (flags.multiline
                                && input
                                    .get(position)
                                    .is_some_and(|unit| is_line_terminator(*unit)))
                        {
                            push_position(position);
                        }
                    }

                    Term::Group {
                        capture_index,
                        pattern,
                        ..
                    } => {
                        let results = match_pattern(pattern, input, state, flags, budget)?;

                        for mut result in results {
                            if let Some(index) = capture_index {
                                result.groups[*index] = Some(Match {
                                    start: position,
                                    end: result.position,
                                });
                            }

                            next.push(result);
                        }
                    }

                    Term::Repeat {
                        term,
                        min,
                        max,
                        greedy,
                    } => {
                        next.extend(match_repeat(
                            term, *min, *max, *greedy, input, state, flags, budget,
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
                            push_position(position);
                        }
                    }

                    Term::Lookahead { negated, pattern } => {
                        let found = match_pattern(pattern, input, state.clone(), flags, budget)?;

                        if *negated {
                            if found.is_empty() {
                                next.push(state);
                            }
                        } else if let Some(mut result) = found.into_iter().next() {
                            result.position = position;
                            next.push(result);
                        }
                    }

                    Term::Backreference { index, .. } => {
                        if let Some(capture) = state.groups[*index] {
                            let captured = &input[capture.start..capture.end];

                            let candidate = input
                                .get(position..)
                                .and_then(|rest| rest.get(..captured.len()));

                            if let Some(candidate) = candidate {
                                let equal =
                                    captured.iter().zip(candidate).all(|(&left, &right)| {
                                        code_units_equal(left, right, flags.ignore_case)
                                    });

                                if equal {
                                    push_position(position + captured.len());
                                }
                            }
                        } else {
                            // A valid backreference to an unmatched group matches an empty string.
                            push_position(position);
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
    start: State,
    flags: RegExpFlags,
    budget: &mut usize,
) -> Result<Vec<State>, MatchError> {
    enum Frame {
        Visit { state: State, count: usize },
        Emit(State),
    }

    let atom = Pattern {
        alternatives: vec![vec![term.clone()]],
    };

    let mut stack = vec![Frame::Visit {
        state: start,
        count: 0,
    }];

    let mut matches = Vec::new();

    while let Some(frame) = stack.pop() {
        step(budget)?;

        let (state, count) = match frame {
            Frame::Emit(state) => {
                matches.push(state);
                continue;
            }

            Frame::Visit { state, count } => (state, count),
        };

        if count >= min {
            if greedy {
                stack.push(Frame::Emit(state.clone()));
            } else {
                matches.push(state.clone());
            }
        }

        if max.is_some_and(|limit| count >= limit) {
            continue;
        }

        let position = state.position;
        let mut next_state = state;

        clear_captures(term, &mut next_state.groups);

        let ends = match_pattern(&atom, input, next_state, flags, budget)?;

        for end in ends.into_iter().rev() {
            // An empty match must not repeat indefinitely.
            if end.position == position && count >= min {
                continue;
            }

            stack.push(Frame::Visit {
                state: end,
                count: count + 1,
            });
        }
    }

    Ok(matches)
}

// RepeatMatcher clears the atom's captures before each repetition.
// https://tc39.es/ecma262/#sec-repeatmatcher
fn clear_captures(term: &Term, groups: &mut [Option<Match>]) {
    match term {
        Term::Group {
            capture_index,
            pattern,
            ..
        } => {
            if let Some(index) = capture_index {
                groups[*index] = None;
            }

            for inner in pattern.alternatives.iter().flatten() {
                clear_captures(inner, groups);
            }
        }

        Term::Lookahead { pattern, .. } => {
            for inner in pattern.alternatives.iter().flatten() {
                clear_captures(inner, groups);
            }
        }

        Term::Repeat { term, .. } => {
            clear_captures(term, groups);
        }

        _ => {}
    }
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
    use super::{Match, MatchError, find, find_captures_utf16};
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

    #[test]
    fn matches_positive_and_negative_lookahead() {
        for (pattern, input, expected) in [
            ("a(?=b)", "ab", Some((0, 1))),
            ("a(?=b)b", "ab", Some((0, 2))),
            ("a(?!b)", "ac", Some((0, 1))),
            ("a(?!b)", "ab", None),
            ("(?=a|b)b", "b", Some((0, 1))),
            ("(?!a|b)c", "c", Some((0, 1))),
            ("(?=)", "", Some((0, 0))),
            ("(?!)", "", None),
        ] {
            let result = search(pattern, input, 0, "")
                .unwrap()
                .map(|matched| (matched.start, matched.end));

            assert_eq!(result, expected, "pattern: {pattern:?}, input: {input:?}");
        }
    }

    #[test]
    fn matches_lookahead_spans_from_test262_fixtures() {
        use crate::javascript::regexp::scan_literal;

        for (file, fragment, input, expected) in [
            ("S15.10.2.8_A1_T1.js", r"/(?=(a+))/", "baaabac", (1, 1)),
            (
                "S15.10.2.8_A2_T2.js",
                r"/Java(?!Script)([A-Z]\w*)/",
                "using of JavaBeans technology",
                (9, 18),
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

    #[test]
    fn matches_hex_and_surrogate_code_units() {
        for (pattern, input, expected) in [
            (r"^\x41\u0042$", "AB", Some((0, 2))),
            (r"\ud83d", "😀", Some((0, 1))),
            (r"\ude00", "😀", Some((1, 2))),
            (r"\ud83d\ude00", "😀", Some((0, 2))),
            (r"[\ud800-\udbff]+", "😀", Some((0, 1))),
            (r"\u0041{2}", "AA", Some((0, 2))),
        ] {
            let result = search(pattern, input, 0, "")
                .unwrap()
                .map(|matched| (matched.start, matched.end));

            assert_eq!(result, expected, "pattern: {pattern:?}, input: {input:?}");
        }
    }

    #[test]
    fn matches_ascii_ignore_case_literals_and_ranges() {
        for (pattern, input, expected) in [
            ("abc", "xABc", Some((1, 4))),
            (r"\x61", "A", Some((0, 1))),
            ("[a-z]+", "ABC", Some((0, 3))),
            ("[^a-z]", "A", None),
            ("[A-Z]", "q", Some((0, 1))),
            ("[A-z]", "_", Some((0, 1))),
            (r"\babc\b", "ABC!", Some((0, 3))),
            ("(?=ab)ab", "AB", Some((0, 2))),
        ] {
            let result = search(pattern, input, 0, "i")
                .unwrap()
                .map(|matched| (matched.start, matched.end));

            assert_eq!(result, expected, "pattern: {pattern:?}, input: {input:?}");
        }

        assert_eq!(
            search("a", "aé", 0, "i"),
            Ok(Some(Match { start: 0, end: 1 }))
        );

        assert_eq!(search("é", "e", 0, "i"), Ok(None),);
        assert_eq!(search("a", "A", 0, ""), Ok(None));
    }

    #[test]
    fn matches_ascii_ignore_case_from_test262_fixture() {
        use crate::javascript::regexp::scan_literal;

        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/test262/test/built-ins/RegExp/prototype/exec/",
            "S15.10.6.2_A1_T15.js"
        );

        let source = std::fs::read_to_string(path).expect("Test262 fixture must exist");

        assert!(source.contains(r#"var __expected = ["ls"];"#));

        let start = source.find("/LS/i").expect("Test262 regexp must exist");
        let literal = scan_literal(&source, start).unwrap();
        let pattern = literal.parse_pattern().unwrap();
        let flags = literal.parse_flags().unwrap();

        let result = find(&pattern, "false", 0, flags).unwrap();

        assert_eq!(result, Some(Match { start: 2, end: 4 }));
    }

    #[test]
    fn matches_legacy_ignore_case_in_unicode_ranges() {
        for (pattern, input, expected) in [
            ("é", "É", true),
            ("σ", "ς", true),
            ("[à-ö]", "Ä", true),
            ("[^à-ö]", "Ä", false),
            ("ſ", "S", false),
            ("K", "K", false),
            ("ß", "SS", false),
            (r"^\w$", "ſ", false),
        ] {
            let result = search(pattern, input, 0, "i").unwrap();

            assert_eq!(
                result.is_some(),
                expected,
                "pattern: {pattern:?}, input: {input:?}"
            );
        }
    }

    #[test]
    fn matches_null_and_control_escapes() {
        for (pattern, input, expected) in [
            (r"\0", "a\0b", Some((1, 2))),
            (r"\cA", "\u{0001}", Some((0, 1))),
            (r"\ca", "\u{0001}", Some((0, 1))),
            (r"[\0\cZ]", "\u{001A}", Some((0, 1))),
            (r"[\cA-\cC]", "\u{0002}", Some((0, 1))),
            (r"\cA", "A", None),
        ] {
            let result = search(pattern, input, 0, "")
                .unwrap()
                .map(|matched| (matched.start, matched.end));

            assert_eq!(result, expected, "pattern: {pattern:?}, input: {input:?}");
        }
    }

    #[test]
    fn matches_control_letters_from_test262_fixtures() {
        use crate::javascript::regexp::{RegExpFlags, matcher::find_utf16};

        for (file, first, last) in [
            ("S15.10.2.10_A2.1_T1.js", b'A', b'Z'),
            ("S15.10.2.10_A2.1_T2.js", b'a', b'z'),
        ] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("testdata/test262/test/built-ins/RegExp")
                .join(file);

            let fixture_source = std::fs::read_to_string(path).expect("Test262 fixture must exist");

            assert!(
                fixture_source.contains("String.fromCharCode(alpha % 32)"),
                "fixture: {file}"
            );

            for byte in first..=last {
                let pattern_source = format!(r"\c{}", byte as char);
                let pattern = pattern::parse(&pattern_source).unwrap();
                let input = [u16::from(byte % 32)];

                let result = find_utf16(&pattern, &input, 0, RegExpFlags::default()).unwrap();

                assert_eq!(
                    result,
                    Some(Match { start: 0, end: 1 }),
                    "fixture: {file}, pattern: {pattern_source}"
                );
            }
        }
    }

    #[test]
    fn records_captures_and_restores_them_during_backtracking() {
        let cases: &[(&str, &str, &[Option<(usize, usize)>])] = &[
            (
                "(a|(ab))c",
                "abc",
                &[Some((0, 3)), Some((0, 2)), Some((0, 2))],
            ),
            ("(a)?b", "b", &[Some((0, 1)), None]),
            ("(a*)b", "b", &[Some((0, 1)), Some((0, 0))]),
            ("(a(b)?)+", "aba", &[Some((0, 3)), Some((2, 3)), None]),
            ("(?=(a+))a", "aaa", &[Some((0, 1)), Some((0, 3))]),
            ("(?!(a))b", "b", &[Some((0, 1)), None]),
            ("(😀)", "a😀", &[Some((1, 3)), Some((1, 3))]),
        ];

        for &(pattern, input, expected) in cases {
            let parsed = pattern::parse(&pattern).unwrap();
            let units: Vec<u16> = input.encode_utf16().collect();
            let flags = RegExpFlags::parse("").unwrap();

            let result = find_captures_utf16(&parsed, &units, 0, flags)
                .unwrap()
                .unwrap();

            let actual: Vec<_> = result
                .groups
                .iter()
                .map(|capture| capture.map(|capture| (capture.start, capture.end)))
                .collect();

            assert_eq!(&actual, expected, "pattern: {pattern:?}");
        }
    }

    #[test]
    fn matches_decimal_backreferences() {
        let cases = [
            (r"(ab)\1", "abab", "", Some((0, 4))),
            (r"(a|ab)\1c", "ababc", "", Some((0, 5))),
            (r"(a)?b\1", "b", "", Some((0, 1))),
            (r"\1(a)", "a", "", Some((0, 1))),
            (r"(a)\1", "aA", "i", Some((0, 2))),
            (r"(a)\1", "aA", "", None),
            (
                r"(a)(b)(c)(d)(e)(f)(g)(h)(i)(j)\10",
                "abcdefghijj",
                "",
                Some((0, 11)),
            ),
            (r"(😀)\1", "😀😀", "", Some((0, 4))),
        ];

        for (pattern, input, flags, expected) in cases {
            let result = search(pattern, input, 0, flags)
                .unwrap()
                .map(|matched| (matched.start, matched.end));

            assert_eq!(result, expected, "pattern: {pattern:?}");
        }
    }

    #[test]
    fn named_groups_remain_usable_as_numbered_groups() {
        assert_eq!(
            search(r"(?<letter>a)(b)\1", "aba", 0, ""),
            Ok(Some(Match { start: 0, end: 3 }))
        );
    }
}
