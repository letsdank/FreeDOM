use super::{
    BinaryOperator, Expression, FunctionKind, FunctionParameter, JsError, Statement, Token,
    TokenKind, UserFunction, Value,
};

use cursor::ParserCursor;

const MAX_PARSE_DEPTH: usize = 32;

#[path = "parser/cursor.rs"]
mod cursor;

#[path = "parser/statement.rs"]
mod statement;

#[path = "parser/primary.rs"]
mod primary;

#[path = "parser/function.rs"]
mod function;

#[path = "parser/expression.rs"]
mod expression;

#[path = "parser/switch.rs"]
mod switch;

#[path = "parser/for_of_array.rs"]
mod for_of_array;

pub(super) fn parse_program(source: &str) -> Result<Vec<Statement>, JsError> {
    Parser::new(source).parse_program()
}

struct Parser<'source> {
    cursor: ParserCursor<'source>,
    source: &'source str,
    await_allowed: bool,
    depth: usize,
}

impl<'source> Parser<'source> {
    fn new(source: &'source str) -> Self {
        Self {
            cursor: ParserCursor::new(source),
            source,
            await_allowed: false,
            depth: 0,
        }
    }

    fn parse_program(&mut self) -> Result<Vec<Statement>, JsError> {
        let mut statements = Vec::new();

        while !self.cursor.check(&TokenKind::EndOfFile)? {
            statements.push(self.parse_statement()?);
        }

        Ok(statements)
    }

    fn consume_identifier(&mut self, message: &str) -> Result<(String, usize), JsError> {
        let token = self.cursor.advance()?;

        match token.kind {
            TokenKind::Identifier(name) => Ok((name, token.offset)),
            _ => Err(JsError::syntax(token.offset, message)),
        }
    }

    fn consume_contextual_keyword(&mut self, keyword: &str) -> Result<bool, JsError> {
        match &self.cursor.peek()?.kind {
            TokenKind::Identifier(identifier) if identifier == keyword => {
                self.cursor.advance()?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn with_parse_frame<T>(
        &mut self,
        parse: impl FnOnce(&mut Self) -> Result<T, JsError>,
    ) -> Result<T, JsError> {
        if self.depth >= MAX_PARSE_DEPTH {
            return Err(JsError::resource_limit(
                "JavaScript parser depth limit exceeded",
            ));
        }

        self.depth += 1;

        let result = parse(self);

        self.depth -= 1;

        result
    }
}

#[cfg(test)]
mod tests {
    use super::Parser;
    use crate::javascript::JsErrorKind;

    #[test]
    fn bounds_recursive_parser_nesting_and_unwinds() {
        let cases = [
            format!("{}1{};", "(".repeat(100), ")".repeat(100)),
            format!("{}true;", "!".repeat(100)),
            format!("{}0;", "a = ".repeat(100)),
            format!("{}0;{}", "if (true) {".repeat(100), "}".repeat(100)),
            format!("{}1{};", "[".repeat(100), "]".repeat(100)),
        ];

        for source in cases {
            let mut parser = Parser::new(&source);

            let error = parser.parse_program().err().unwrap();

            assert_eq!(error.kind, JsErrorKind::ResourceLimit);
            assert!(error.message.contains("parser depth"));
            assert_eq!(parser.depth, 0);
        }

        assert!(Parser::new("let x = (1 + 2);").parse_program().is_ok());

        let mut invalid = Parser::new("if (true) { const x = ; }");
        let error = invalid.parse_program().err().unwrap();

        assert_eq!(error.kind, JsErrorKind::Syntax);
        assert_eq!(invalid.depth, 0);
    }
}
