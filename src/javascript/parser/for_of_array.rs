//! Partial array binding patterns for for-of statements.
//!
//! <https://tc39.es/ecma262/#sec-for-in-and-for-of-statements>

use super::{JsError, Parser, Statement, TokenKind};

impl Parser<'_> {
    pub(super) fn parse_for_of_array(&mut self) -> Result<Statement, JsError> {
        self.cursor
            .consume(&TokenKind::LeftBracket, "expected '['")?;

        let mut bindings = Vec::new();
        let mut names = std::collections::HashSet::new();

        if !self.cursor.consume_optional(&TokenKind::RightBracket)? {
            loop {
                let (name, offset) = self.consume_identifier("expected array binding name")?;

                if !names.insert(name.clone()) {
                    return Err(JsError::syntax(offset, "duplicate lexical binding"));
                }

                bindings.push(name);

                if self.cursor.consume_optional(&TokenKind::Comma)? {
                    if self.cursor.consume_optional(&TokenKind::RightBracket)? {
                        break;
                    }

                    continue;
                }

                self.cursor.consume(
                    &TokenKind::RightBracket,
                    "expected ',' or ']' in array binding",
                )?;

                break;
            }
        }

        if !self.consume_contextual_keyword("of")? {
            return Err(JsError::syntax(
                self.cursor.peek()?.offset,
                "expected 'of' after array binding",
            ));
        }

        let iterable = self.parse_expression()?;

        self.cursor
            .consume(&TokenKind::RightParen, "expected ')' after for-of iterable")?;

        let body = self.parse_statement_body()?;

        Ok(Statement::ForOfArray {
            bindings,
            iterable,
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::Statement;
    use crate::javascript::parser::parse_program;

    #[test]
    fn parses_array_binding_from_testharness() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        let header = "for (const [prop, expected] of Object.entries(required_props))";

        assert!(source.contains(header));

        let program = parse_program(&format!("{header} {{}}")).unwrap();

        assert!(matches!(
            program.as_slice(),
            [Statement::ForOfArray { bindings , .. }]
                if bindings == &["prop", "expected"]
        ));
    }

    #[test]
    fn rejects_invalid_duplicate_and_incomplete_bindings() {
        for source in [
            "for (let [x, x] of []) {}",
            "for (const [x of []) {}",
            "for (const [x,] in []) {}",
        ] {
            assert!(parse_program(source).is_err(), "{source}");
        }
    }
}
