//! Partial parser for JavaScript switch statements.
//!
//! <https://tc39.es/ecma262/#sec-switch-statement>

use super::{Expression, JsError, Parser, Statement, TokenKind};
use crate::javascript::ast::SwitchCase;

impl Parser<'_> {
    pub(super) fn parse_switch_statement(&mut self) -> Result<Statement, JsError> {
        self.cursor
            .consume(&TokenKind::LeftParen, "expected '(' after switch")?;

        let discriminant = self.parse_expression()?;

        self.cursor.consume(
            &TokenKind::RightParen,
            "expected ')' after switch expression",
        )?;

        self.cursor
            .consume(&TokenKind::LeftBrace, "expected '{' after switch")?;

        let mut cases = Vec::new();
        let mut has_default = false;

        while !self.cursor.check(&TokenKind::RightBrace)? {
            let test: Option<Expression> = if self.consume_contextual_keyword("case")? {
                Some(self.parse_expression()?)
            } else if self.consume_contextual_keyword("default")? {
                if has_default {
                    return Err(JsError::syntax(
                        self.cursor.peek()?.offset,
                        "duplicate default in switch",
                    ));
                }

                has_default = true;

                None
            } else {
                return Err(JsError::syntax(
                    self.cursor.peek()?.offset,
                    "expected case or default in switch",
                ));
            };

            self.cursor
                .consume(&TokenKind::Colon, "expected ':' after switch label")?;

            let mut body = Vec::new();

            loop {
                let next = self.cursor.peek()?;

                if matches!(next.kind, TokenKind::RightBrace | TokenKind::EndOfFile)
                    || matches!(&next.kind,TokenKind::Identifier(name)if name == "case" || name == "default")
                {
                    break;
                }

                body.push(self.parse_statement()?);
            }

            cases.push(SwitchCase { test, body });
        }

        self.cursor.advance()?;

        Ok(Statement::Switch {
            discriminant,
            cases,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::Statement;
    use crate::javascript::parser::parse_program;

    #[test]
    fn parses_switch_header_from_real_testharness() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        let header = "switch (typeof val) {";

        assert!(source.contains(header));

        let program = parse_program(&format!(
            "{header} case \"string\": return 1; default: return 2; }}"
        ))
        .unwrap();

        assert!(matches!(
            program.as_slice(),
            [Statement::Switch { cases , .. }] if cases.len() == 2
        ));
    }

    #[test]
    fn rejects_invalid_switch_syntax() {
        for source in [
            "switch (1) { default: default: }",
            "switch (1) { case 1 }",
            "switch (1) { 1; }",
            "switch (1) { case 1:",
        ] {
            assert!(parse_program(source).is_err(), "{source}");
        }
    }
}
