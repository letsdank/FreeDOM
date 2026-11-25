//! Partial CSS comment scanning.
//!
//! Specification: <https://www.w3.org/TR/css-syntax-3/#consume-comments>
//!
//! This is not a complete tokenizer and does not preserve token boundaries.

pub(super) fn remove_css_comments(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let mut characters = source.chars().peekable();
    let mut quote = None;

    while let Some(character) = characters.next() {
        if character == '\\' {
            output.push(character);

            if let Some(next) = characters.next() {
                output.push(next);
            }

            continue;
        }

        if let Some(delimiter) = quote {
            output.push(character);

            if character == delimiter {
                quote = None;
            }

            continue;
        }

        if matches!(character, '\'' | '"') {
            quote = Some(character);
            output.push(character);
        } else if character == '/' && characters.peek() == Some(&'*') {
            characters.next();

            while let Some(next) = characters.next() {
                if next == '*' && characters.peek() == Some(&'/') {
                    characters.next();
                    break;
                }
            }
        } else {
            output.push(character);
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::remove_css_comments;

    #[test]
    fn preserves_comment_markers_inside_strings() {
        assert_eq!(
            remove_css_comments(r#"/* content */p { font-family: 'A/*B*/C'; }"#),
            "p { font-family: 'A/*B*/C'; }"
        );

        assert_eq!(
            remove_css_comments(r#"x: 'a\'/*b*/c';/* end */"#),
            r#"x: 'a\'/*b*/c';"#
        );
    }

    #[test]
    fn discards_unclosed_comments() {
        assert_eq!(remove_css_comments("é/* unfinished"), "é");
    }
}
