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

#[cfg(test)]
mod tests {
    use super::is_unescaped_identifier;

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
}
