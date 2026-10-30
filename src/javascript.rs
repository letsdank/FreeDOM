use std::{error::Error, fmt};

#[path = "javascript/lexer.rs"]
mod lexer;

#[cfg(test)]
#[path = "javascript/token_cursor.rs"]
mod token_cursor;

#[path = "javascript/token_stream.rs"]
mod token_stream;

pub use lexer::{Token, TokenKind};

// Keep the batch tokenizer available as a public entry point.
#[allow(unused_imports)]
pub use lexer::tokenize;

// The scanner will be connected to the lexer once lexical goals are available.
#[allow(dead_code)]
#[path = "javascript/regexp.rs"]
pub(crate) mod regexp;

#[path = "javascript/parser.rs"]
mod parser;

#[cfg(test)]
#[path = "javascript/json.rs"]
mod json;

#[cfg(test)]
pub use json::parse_json;

use parser::parse_program;

#[path = "javascript/dom.rs"]
mod dom;

pub use dom::DocumentState;

#[path = "javascript/runtime.rs"]
mod runtime;

#[path = "javascript/ast.rs"]
mod ast;

#[path = "javascript/number.rs"]
mod number;

#[path = "javascript/value.rs"]
mod value;

pub use value::{NativeFunction, RegExpValue, Value};

pub(crate) use value::{
    ArrayMethodKind, ArrayStorage, DocumentMethod, ElementMethod, TestContextMethod,
};

use value::RegExpState;

pub(crate) use ast::{BinaryOperator, Expression, Statement, UserFunction};

use runtime::{Completion, Interpreter};

/// Classification of parser and interpreter failures.
///
/// Explicitly thrown JavaScript values are distinct from engine failures.
/// Other runtime failures are still unclassified.
#[derive(Clone, Debug, PartialEq)]
pub enum JsErrorKind {
    Syntax,
    UnclassifiedRuntime,
    UnsupportedFeature,
    Thrown(Value),
    NativeException(Value),
}

#[derive(Clone, Debug, PartialEq)]
pub struct JsError {
    pub offset: Option<usize>,
    pub message: String,
    pub kind: JsErrorKind,
}

impl JsError {
    fn syntax(offset: usize, message: impl Into<String>) -> Self {
        Self {
            offset: Some(offset),
            message: message.into(),
            kind: JsErrorKind::Syntax,
        }
    }

    fn runtime(message: impl Into<String>) -> Self {
        Self {
            offset: None,
            message: message.into(),
            kind: JsErrorKind::UnclassifiedRuntime,
        }
    }

    fn unsupported(message: impl Into<String>) -> Self {
        Self {
            offset: None,
            message: message.into(),
            kind: JsErrorKind::UnsupportedFeature,
        }
    }

    fn thrown(value: Value) -> Self {
        Self {
            offset: None,
            message: "Uncaught JavaScript exception".to_owned(),
            kind: JsErrorKind::Thrown(value),
        }
    }

    fn native_exception(name: &str, message: impl Into<String>) -> Self {
        let message = message.into();

        Self {
            offset: None,
            message: format!("{name}: {message}"),
            kind: JsErrorKind::NativeException(Value::native_error(name, &message)),
        }
    }

    fn reference_error(name: &str) -> Self {
        Self::native_exception("ReferenceError", format!("{name} is not defined"))
    }

    fn type_error(message: impl Into<String>) -> Self {
        Self::native_exception("TypeError", message)
    }
}

impl fmt::Display for JsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.offset {
            Some(offset) => write!(f, "{} at byte {offset}", self.message),
            None => write!(f, "{}", self.message),
        }
    }
}

impl Error for JsError {}

pub fn evaluate(source: &str) -> Result<Value, JsError> {
    let program = parse_program(source)?;
    Interpreter::new()
        .execute(&program)
        .map(Completion::into_value)
}

pub fn evaluate_with_document(
    source: &str,
    document: &mut DocumentState,
) -> Result<Value, JsError> {
    let program = parse_program(source)?;

    let mut interpreter = Interpreter::new();
    interpreter.set_document(document.clone());

    let result = interpreter.execute(&program);

    let Some(updated_document) = interpreter.take_document() else {
        return Err(JsError::runtime(
            "TypeError: global document binding must remain a Document",
        ));
    };

    *document = updated_document;

    result.map(Completion::into_value)
}

#[cfg(test)]
#[path = "javascript/tests.rs"]
mod tests;
