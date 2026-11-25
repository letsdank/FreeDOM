//! Unescaped CSS identifier validation.
//!
//! Specification: <https://www.w3.org/TR/css-syntax-3/#would-start-an-identifier>
//!
//! Escape sequences are not implemented here.

fn is_ident_start(character: char) -> bool {
    character.is_alphabetic() || character == '_' || !character.is_ascii()
}

fn is_ident_continue(character: char) -> bool {
    is_ident_start(character) || character.is_ascii_digit() || character == '-'
}

#[cfg(test)]
pub(super) fn is_unescaped_identifier(value: &str) -> bool {
    let mut characters = value.chars();

    let Some(first) = characters.next() else {
        return false;
    };

    if first == '-' {
        let Some(second) = characters.next() else {
            return false;
        };

        if second != '-' && !is_ident_start(second) {
            return false;
        }
    } else if !is_ident_start(first) {
        return false;
    }

    characters.all(is_ident_continue)
}

/// Consumes an identifier and returns its decoded value and remaining input.
///
/// This is a subset of the CSS Syntax ident-sequence algorithm.
pub(super) fn consume_identifier(input: &str) -> Option<(String, &str)> {
    let mut lookahead = input.chars();

    let first = lookahead.next()?;
    let second = lookahead.next();
    let third = lookahead.next();

    let valid_escape =
        |first: Option<char>, second: Option<char>| first == Some('\\') && second != Some('\n');

    let starts_identifier = if first == '-' {
        second.is_some_and(|character| character == '-' || is_ident_start(character))
            || valid_escape(second, third)
    } else {
        is_ident_start(first) || valid_escape(Some(first), second)
    };

    if !starts_identifier {
        return None;
    }

    let mut characters = input.char_indices().peekable();
    let mut value = String::new();

    while let Some(&(_, character)) = characters.peek() {
        if is_ident_continue(character) {
            characters.next();
            value.push(character);
        } else if character == '\\' {
            characters.next();

            let next = characters.peek().map(|&(_, character)| character);

            if next == Some('\n') {
                break;
            }

            if next.is_none() {
                value.push('\u{FFFD}');
                break;
            }

            if next.is_some_and(|character| character.is_ascii_hexdigit()) {
                let mut code_point = 0_u32;

                for _ in 0..6 {
                    let Some(&(_, digit)) = characters.peek() else {
                        break;
                    };

                    if !digit.is_ascii_hexdigit() {
                        break;
                    }

                    let digit_value = digit.to_digit(16).expect("ASCII hexadecimal digit");
                    code_point = code_point * 16 + digit_value;
                    characters.next();
                }

                if characters.peek().is_some_and(|&(_, character)| {
                    matches!(character, ' ' | '\n' | '\t' | '\r' | '\u{000C}')
                }) {
                    characters.next();
                }

                let decoded = char::from_u32(code_point)
                    .filter(|&character| character != '\0')
                    .unwrap_or('\u{FFFD}');

                value.push(decoded);
            } else {
                let (_, escaped) = characters.next().expect("checked above");
                value.push(escaped);
            }
        } else {
            break;
        }
    }

    let boundary = characters.peek().map_or(input.len(), |&(index, _)| index);

    Some((value, &input[boundary..]))
}

#[cfg(test)]
mod tests {
    use super::{consume_identifier, is_unescaped_identifier};

    #[test]
    fn accepts_unescaped_identifier_starts() {
        for value in ["hello", "_x", "--x", "--", "-x", "é", "\u{FFFD}foo", "x7"] {
            assert!(is_unescaped_identifier(value), "{value:?}")
        }
    }

    #[test]
    fn rejects_invalid_or_unsupported_identifier_starts() {
        for value in ["", "-", "123", "-1a", "a+b", "\\61"] {
            assert!(!is_unescaped_identifier(value), "{value:?}")
        }
    }

    #[test]
    fn decodes_css_escapes() {
        for (input, expected, tail) in [
            (r"\31 23", "123", ""),
            (r"a\.b#x", "a.b", "#x"),
            (r"\0 a", "\u{FFFD}a", ""),
            (r"\110000", "\u{FFFD}", ""),
            (r"\41 B", "AB", ""),
            (r"\1F4A1", "💡", ""),
        ] {
            assert_eq!(
                consume_identifier(input),
                Some((expected.to_owned(), tail)),
                "input: {input:?}"
            );
        }
    }
}
