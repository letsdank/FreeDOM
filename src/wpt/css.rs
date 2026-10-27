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
}
