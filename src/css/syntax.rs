//! Partial CSS declaration parsing.
//!
//! Specification: <https://www.w3.org/TR/css-syntax-3/#parse-blocks-contents>
//!
//! This is not yet a complete CSS tokenizer or error-recovery implementation.

use super::CssDeclaration;

/// Splits at commas outside quoted strings and balanced parentheses/brackets.
///
/// The caller must preprocess CSS input and handle comments first.
pub(super) fn split_top_level_commas(source: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quote = None;
    let mut closing_delimiters = Vec::new();
    let mut characters = source.char_indices().peekable();

    while let Some((index, character)) = characters.next() {
        if character == '\\' {
            characters.next();
            continue;
        }

        if let Some(delimiter) = quote {
            if character == delimiter {
                quote = None;
            }

            continue;
        }

        match character {
            '\'' | '"' => quote = Some(character),

            '(' => closing_delimiters.push(')'),
            '[' => closing_delimiters.push(']'),

            ')' | ']' if closing_delimiters.last() == Some(&character) => {
                closing_delimiters.pop();
            }

            ',' if closing_delimiters.is_empty() => {
                parts.push(&source[start..index]);
                start = index + 1;
            }

            _ => {}
        }
    }

    parts.push(&source[start..]);

    parts
}

pub(crate) fn parse_declaration_list(source: &str) -> Vec<CssDeclaration> {
    let normalized = super::preprocess_input(source);
    let source = normalized.as_str();
    let mut declarations = Vec::new();
    let mut start = 0;
    let mut quote = None;
    let mut closing_delimiters = Vec::new();
    let mut characters = source.char_indices().peekable();

    while let Some((index, character)) = characters.next() {
        if character == '\\' {
            characters.next();
            continue;
        }

        if let Some(delimiter) = quote {
            if character == delimiter {
                quote = None;
            }

            continue;
        }

        if character == '/' && characters.peek().is_some_and(|&(_, next)| next == '*') {
            characters.next();

            while let Some((_, next)) = characters.next() {
                if next == '*'
                    && characters
                        .peek()
                        .is_some_and(|&(_, character)| character == '/')
                {
                    characters.next();
                    break;
                }
            }

            continue;
        }

        match character {
            '\'' | '"' => quote = Some(character),

            '(' => closing_delimiters.push(')'),
            '[' => closing_delimiters.push(']'),
            '{' => closing_delimiters.push('}'),

            ')' | ']' | '}' if closing_delimiters.last() == Some(&character) => {
                closing_delimiters.pop();
            }

            ';' if closing_delimiters.is_empty() => {
                append_declaration(&source[start..index], &mut declarations);
                start = index + 1;
            }

            _ => {}
        }
    }

    append_declaration(&source[start..], &mut declarations);

    declarations
}

fn append_declaration(source: &str, declarations: &mut Vec<CssDeclaration>) {
    let Some((name, value)) = source.split_once(':') else {
        return;
    };

    let name = name.trim().to_ascii_lowercase();

    if !name.is_empty() {
        declarations.push(CssDeclaration {
            name,
            value: value.trim().to_owned(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::parse_declaration_list;

    #[test]
    fn keeps_nested_semicolons_in_values() {
        let source = "\
            font-family: 'Nko; Test'; \
            background: url(a;b); \
            x: fn([a;b]); \
            color: green";

        let declarations = parse_declaration_list(source);

        let actual: Vec<_> = declarations
            .iter()
            .map(|declaration| (declaration.name.as_str(), declaration.value.as_str()))
            .collect();

        assert_eq!(
            actual,
            vec![
                ("font-family", "'Nko; Test'"),
                ("background", "url(a;b)"),
                ("x", "fn([a;b])"),
                ("color", "green"),
            ]
        );
    }

    #[test]
    fn keeps_semicolons_in_escaped_strings() {
        let source = r#"content: 'a\';b'; color: green"#;
        let declarations = parse_declaration_list(source);

        assert_eq!(declarations.len(), 2);
        assert_eq!(declarations[0].value, r#"'a\';b'"#);
        assert_eq!(declarations[1].name, "color");
    }

    #[test]
    fn preprocesses_declaration_values() {
        let declarations = parse_declaration_list("font-family: 'A\0B'; color: green");

        assert_eq!(declarations[0].value, "'A\u{FFFD}B'");
        assert_eq!(declarations[1].name, "color");
    }

    #[test]
    fn splits_only_top_level_unescaped_commas() {
        use super::split_top_level_commas;

        assert_eq!(
            split_top_level_commas(r#".a\,b, :is(.c,.d), [data="x,y"], .end"#),
            vec![r".a\,b", " :is(.c,.d)", r#" [data="x,y"]"#, " .end",]
        );

        assert_eq!(split_top_level_commas(r".a\\, .b"), vec![r".a\\", " .b"]);
    }
}
