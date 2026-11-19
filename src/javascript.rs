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
pub fn parse_json(source: &str) -> Result<Value, HostError> {
    json::parse_json(source).map_err(HostError::from)
}

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

#[path = "javascript/arguments.rs"]
mod arguments;

pub use value::{NativeFunction, RegExpValue, Value};

pub(crate) use value::{
    ArrayMethodKind, ArrayStorage, DocumentMethod, ElementMethod, TestContextMethod,
};

use value::RegExpState;

pub(crate) use ast::{
    BinaryOperator, Expression, FunctionKind, FunctionParameter, PostfixStep, Statement,
    UserFunction,
};

use runtime::{Completion, Interpreter};

#[path = "javascript/testharness_probe.rs"]
mod testharness_probe;

pub(crate) use testharness_probe::{probe_testharness, probe_testharness_test};

#[path = "javascript/session.rs"]
mod session;

pub(crate) use session::DocumentScriptSession;

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
    ResourceLimit,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct JsError {
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

    fn resource_limit(message: impl Into<String>) -> Self {
        Self {
            offset: None,
            message: message.into(),
            kind: JsErrorKind::ResourceLimit,
        }
    }
}

impl fmt::Display for JsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        format_error(f, &self.message, self.offset)
    }
}

impl Error for JsError {}

/// An error returned to the embedding application.
///
/// Exception values still use the current reference-counted representation.
#[derive(Clone, Debug, PartialEq)]
pub struct HostError {
    pub offset: Option<usize>,
    pub message: String,
    pub kind: JsErrorKind,
}

impl From<JsError> for HostError {
    fn from(error: JsError) -> Self {
        Self {
            offset: error.offset,
            message: error.message,
            kind: error.kind,
        }
    }
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        format_error(f, &self.message, self.offset)
    }
}

impl Error for HostError {}

fn format_error(f: &mut fmt::Formatter<'_>, message: &str, offset: Option<usize>) -> fmt::Result {
    match offset {
        Some(offset) => write!(f, "{message} at byte {offset}"),
        None => f.write_str(message),
    }
}

pub fn evaluate(source: &str) -> Result<Value, HostError> {
    let program = parse_program(source)?;

    Interpreter::new()
        .execute_script(&program)
        .map(Completion::into_value)
        .map_err(HostError::from)
}

pub fn evaluate_with_document(
    source: &str,
    document: &mut DocumentState,
) -> Result<Value, HostError> {
    DocumentScriptSession::new(document).evaluate(source, document)
}

#[cfg(test)]
#[path = "javascript/tests.rs"]
mod tests;
