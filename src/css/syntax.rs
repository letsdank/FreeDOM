//! Partial CSS declaration parsing.
//!
//! Specification: <https://www.w3.org/TR/css-syntax-3/#parse-blocks-contents>
//!
//! This is not yet a complete CSS tokenizer or error-recovery implementation.

use super::CssDeclaration;

pub(crate) fn parse_declaration_list(source: &str) -> Vec<CssDeclaration> {
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
}
