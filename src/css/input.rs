//! CSS input preprocessing.
//!
//! Specification: <https://www.w3.org/TR/css-syntax-3/#input-preprocessing>
//!
//! Rust strings contain only Unicode scalar values, so surrogate code points
//! must be handled by the layer that decodes non-UTF-8 input.

pub(crate) fn preprocess_input(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let mut characters = source.chars().peekable();

    while let Some(character) = characters.next() {
        match character {
            '\r' => {
                if characters.peek() == Some(&'\n') {
                    characters.next();
                }

                output.push('\n');
            }

            '\u{000C}' => output.push('\n'),
            '\0' => output.push('\u{FFFD}'),

            _ => output.push(character),
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::preprocess_input;

    #[test]
    fn normalizes_newlines_and_nulls() {
        assert_eq!(
            preprocess_input("a\r\nb\rc\u{000C}d\0é"),
            "a\nb\nc\nd\u{FFFD}é"
        );
    }
}
