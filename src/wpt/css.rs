//! Targeted checks based on WPT CSS fixtures.
//!
//! These checks do not yet execute WPT testharness.js or implement CSSOM.

#[cfg(test)]
mod tests {
    use crate::{
        css::{Element, TextStyle, build_author_rules, compute_style},
        html::{Document, Node},
    };

    #[test]
    fn wpt_trailing_braces_style_subset() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/css/css-syntax/trailing-braces.html",
        );

        let source = std::fs::read_to_string(path).expect("WPT CSS fixture must exist");
        let document = Document::parse(&source);

        let style_element = document
            .first_element("style")
            .expect("WPT fixture must contain a style element");

        let stylesheet = style_element
            .children
            .iter()
            .filter_map(|node| match node {
                Node::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();

        let rules = build_author_rules(&[stylesheet]);

        assert_eq!(rules.len(), 1, "expected one supported CSS rule");

        let element = Element {
            tag: "div",
            id: Some("target1"),
            classes: &[],
            inline_style: None,
        };

        let style = compute_style(&element, TextStyle::initial(), &rules);

        assert_eq!(
            style.color, 0x00_00_80_00,
            "invalid declarations must not replace green"
        );
    }

    #[test]
    fn wpt_input_preprocessing_null_subset() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/css/css-syntax/input-preprocessing.html",
        );

        let source = std::fs::read_to_string(path).expect("WPT CSS fixture must exist");
        let mut count = 0;

        for line in source.lines().map(str::trim) {
            let Some(arguments) = line
                .strip_prefix("testParsing(")
                .and_then(|line| line.strip_suffix(");"))
            else {
                continue;
            };

            let Some((input, expected)) = arguments.split_once(", ") else {
                continue;
            };

            if !input.contains(r"\x00") {
                continue;
            }

            // Convert the JavaScript escape used by this fixture to JSON syntax.
            let input_json = input.replace(r"\x00", r"\u0000");

            let input = crate::javascript::parse_json(&input_json)
                .expect("WPT input must be a supported string");

            let expected = crate::javascript::parse_json(expected)
                .expect("WPT expectation must be a supported string");

            assert_eq!(
                crate::css::preprocess_input(input.as_string().unwrap()),
                expected.as_string().unwrap()
            );

            let stylesheet = format!("{} {{ color: green }}", input.as_string().unwrap());
            let rules = build_author_rules(&[stylesheet]);

            assert_eq!(rules.len(), 1, "input: {:?}", input.as_string().unwrap());

            let element = Element {
                tag: expected.as_string().unwrap(),
                id: None,
                classes: &[],
                inline_style: None,
            };

            let style = compute_style(&element, TextStyle::initial(), &rules);

            assert_eq!(style.color, 0x00_00_80_00);

            count += 1;
        }

        assert_eq!(count, 5, "expected five WPT NUL cases");
    }

    #[test]
    fn wpt_bad_selector_rule_subset() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/css/CSS2/syntax/bad-selector-001.xht",
        );

        let source = std::fs::read_to_string(path).expect("WPT CSS fixture must exist");

        let document = Document::parse(&source);

        let style_element = document
            .first_element("style")
            .expect("WPT fixture must contain a style element");

        let stylesheet = style_element
            .children
            .iter()
            .filter_map(|node| match node {
                Node::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();

        let rules = build_author_rules(&[stylesheet]);

        assert!(
            rules.is_empty(),
            "the invalid selector list must discard the rule"
        );
    }

    #[test]
    fn wpt_element_vs_universal_specificity_subset() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/css/CSS2/cascade/specificity-007.xht"
        );

        let source = std::fs::read_to_string(path).expect("WPT CSS fixture must exist");
        let document = Document::parse(&source);

        let style_element = document
            .first_element("style")
            .expect("WPT fixture must contain a style element");

        let stylesheet = style_element
            .children
            .iter()
            .filter_map(|node| match node {
                Node::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();

        let rules = build_author_rules(&[stylesheet]);

        assert_eq!(rules.len(), 3);

        let div = Element {
            tag: "div",
            id: None,
            classes: &[],
            inline_style: None,
        };

        let div_style = compute_style(&div, TextStyle::initial(), &rules);

        assert_eq!(div_style.color, 0x00_00_80_00);

        // Verify that the universal selector was applied to another element.
        let span = Element {
            tag: "span",
            id: None,
            classes: &[],
            inline_style: None,
        };

        let span_style = compute_style(&span, TextStyle::initial(), &rules);

        assert_eq!(span_style.color, 0x00_FF_00_00);
    }
}
