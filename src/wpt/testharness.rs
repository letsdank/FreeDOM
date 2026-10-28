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
}
