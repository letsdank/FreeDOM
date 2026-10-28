//! Incremental compatibility checks against the upstream WPT test harness.
//!
//! These checks do not yet run testharness.js.

#[cfg(test)]
mod tests {
    use crate::javascript::{TokenKind, tokenize};

    #[test]
    fn tokenizes_comparisons_from_upstream_testharness() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js",
        );

        let source = std::fs::read_to_string(path).expect("WPT testharness.js must exist");

        for (fragment, expected) in [
            ("w != w.parent", TokenKind::NotEqual),
            ("w !== self", TokenKind::StrictNotEqual),
            ("e[prop] == required_props[prop]", TokenKind::Equal),
            (
                r#"event.data.type === "getmessages""#,
                TokenKind::StrictEqual,
            ),
        ] {
            let start = source.find(fragment).expect("WPT fragment must exist");

            let fragment_source = &source[start..start + fragment.len()];
            let tokens = tokenize(fragment_source).unwrap();

            assert!(
                tokens.iter().any(|token| token.kind == expected),
                "fragment: {fragment}"
            );
        }
    }

    #[test]
    fn scans_regular_expression_literals_from_upstream_testharness() {
        use crate::javascript::regexp::scan_literal;

        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js",
        );

        let source = std::fs::read_to_string(path).expect("WPT testharness.js must exist");

        for (fragment, expected_pattern, following) in [
            (
                r"/^\(\)\s*=>\s*(?:{(.*)}\s*|(.*))$/",
                r"^\(\)\s*=>\s*(?:{(.*)}\s*|(.*))$",
                ");",
            ),
            (
                r"/[\u000A\u000D\u2028\u2029]/",
                r"[\u000A\u000D\u2028\u2029]",
                ".test",
            ),
        ] {
            let start = source.find(fragment).expect("WPT fragment must exist");

            let literal = scan_literal(&source, start).unwrap();

            assert_eq!(literal.pattern, expected_pattern);
            assert_eq!(literal.flags, "");
            assert!(
                source[literal.end..].starts_with(following),
                "fragment: {fragment}"
            );
        }
    }

    #[test]
    fn validates_regular_expression_flags_from_upstream_testharness() {
        use crate::javascript::regexp::scan_literal;

        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js",
        );

        let source = std::fs::read_to_string(path).expect("WPT testharness.js must exist");

        for fragment in [r"/\n/g", r"/\s/g"] {
            let start = source.find(fragment).expect("WPT fragment must exist");
            let literal = scan_literal(&source, start).unwrap();
            let flags = literal.parse_flags().unwrap();

            assert!(flags.global);
            assert_eq!(flags.to_canonical_string(), "g");
            assert_eq!(&source[start..literal.end], fragment);
        }
    }

    #[test]
    fn parses_newline_regexp_from_upstream_testharness() {
        use crate::javascript::regexp::{pattern::Term, scan_literal};

        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js",
        );

        let source = std::fs::read_to_string(path).expect("WPT testharness.js must exist");
        let start = source.find(r"/\n/g").expect("WPT fragment must exist");
        let literal = scan_literal(&source, start).unwrap();
        let pattern = literal.parse_pattern().unwrap();
        let flags = literal.parse_flags().unwrap();

        assert_eq!(pattern.alternatives, vec![vec![Term::Literal('\n')]]);
        assert!(flags.global);
    }
}
