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

    #[test]
    fn wpt_font_size_em_parent_subset() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/css/CSS2/fonts/font-size-rule-004.xht",
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

        let parent = Element {
            tag: "div",
            id: None,
            classes: &[],
            inline_style: None,
        };

        let parent_style = compute_style(&parent, TextStyle::initial(), &rules);

        assert_eq!(parent_style.font_size, 18.0);

        for id in ["span1", "span2"] {
            let element = Element {
                tag: "span",
                id: Some(id),
                classes: &[],
                inline_style: None,
            };

            let style = compute_style(&element, parent_style.clone(), &rules);

            assert_eq!(style.font_size, 27.0, "element: {id}");
        }
    }

    #[test]
    fn wpt_important_vs_inline_font_size_subset() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/css/css-cascade/important-vs-inline-002.html"
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
        let classes = vec!["outer".to_owned()];

        for inline in ["font-size: 24px", "font-size: 36px"] {
            let element = Element {
                tag: "p",
                id: Some("el"),
                classes: &classes,
                inline_style: Some(inline),
            };

            let style = compute_style(&element, TextStyle::initial(), &rules);

            assert_eq!(style.font_size, 18.0);
        }
    }

    #[test]
    fn wpt_invalid_hex_color_subset() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/css/css-color/parsing/color-invalid-hex-color.html"
        );

        let source = std::fs::read_to_string(path).expect("WPT CSS fixture must exist");
        let mut count = 0;

        for line in source.lines().map(str::trim) {
            if !line.starts_with("[\"#") {
                continue;
            }

            let json = line.strip_suffix(',').unwrap_or(line);

            let record = crate::javascript::parse_json(json).expect("WPT case must parse");

            let fields = record.as_array().expect("WPT case must be an array");

            let value = fields[0]
                .as_string()
                .expect("WPT case must contain a color string");

            let inline_style = format!("color: green; color: {value}");

            let element = Element {
                tag: "p",
                id: None,
                classes: &[],
                inline_style: Some(&inline_style),
            };

            let style = compute_style(&element, TextStyle::initial(), &[]);

            assert_eq!(style.color, 0x00_00_80_00, "invalid color: {value:?}");

            count += 1;
        }

        assert_eq!(count, 10, "expected ten WPT cases");
    }

    #[test]
    fn wpt_legacy_rgb_integer_clamping_subset() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/css/css-color/parsing/color-valid-rgb.html"
        );

        let source = std::fs::read_to_string(path).expect("WPT CSS fixture must exist");
        let mut count = 0;

        for line in source.lines().map(str::trim) {
            if !line.starts_with("[\"rgb(") {
                continue;
            }

            let json = line.strip_suffix(',').unwrap_or(line);
            let record = crate::javascript::parse_json(json).expect("WPT case must parse");
            let fields = record.as_array().expect("WPT case must be an array");

            let input = fields[0]
                .as_string()
                .expect("WPT case must contain an input string");

            if !matches!(input, "rgb(500, 0, 0)" | "rgb(-500, 64, 128)") {
                continue;
            }

            let expected = fields[1]
                .as_string()
                .expect("WPT case must contain an expected string");

            let compute_color = |value: &str| {
                let inline_style = format!("color: {value}");

                let element = Element {
                    tag: "p",
                    id: None,
                    classes: &[],
                    inline_style: Some(&inline_style),
                };

                compute_style(&element, TextStyle::initial(), &[]).color
            };

            let expected_color = compute_color(expected);

            assert_ne!(
                expected_color,
                TextStyle::initial().color,
                "unsupported WPT expectation: {expected}"
            );

            assert_eq!(compute_color(input), expected_color, "input: {input}");

            count += 1;
        }

        assert_eq!(count, 2, "expected two legacy integer RGB WPT cases");
    }
}
