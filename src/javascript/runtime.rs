use std::collections::HashMap;

use crate::url::Url;

use super::{
    ArrayMethodKind, BinaryOperator, DocumentMethod, DocumentState, ElementMethod, Expression,
    JsError, NativeFunction, Statement, TestContextMethod, UserFunction, Value,
};

#[path = "runtime/dom.rs"]
mod dom_bindings;

#[path = "runtime/regexp.rs"]
mod regexp_bindings;

#[path = "runtime/array.rs"]
mod array;

#[path = "runtime/value_ops.rs"]
mod value_ops;

#[path = "runtime/wpt_shim.rs"]
mod wpt_shim;

#[path = "runtime/statement.rs"]
mod statement;

#[path = "runtime/operators.rs"]
mod operators;

#[path = "runtime/in_operator.rs"]
mod in_operator;

#[path = "runtime/function.rs"]
mod function;

#[path = "runtime/call.rs"]
mod call;

#[path = "runtime/property.rs"]
mod property;

#[path = "runtime/expression.rs"]
mod expression;

#[path = "runtime/try_statement.rs"]
mod try_statement;

#[path = "runtime/assignment.rs"]
mod assignment;

#[path = "runtime/object.rs"]
mod object;

#[derive(Clone, Debug)]
struct Cleanup {
    callback: UserFunction,
    captured: HashMap<String, Value>,
}

pub(super) enum Completion {
    Normal(Value),
    Return(Value),
    Break,
    Continue,
}

impl Completion {
    pub(super) fn into_value(self) -> Value {
        match self {
            Self::Normal(value) | Self::Return(value) => value,
            Self::Break | Self::Continue => Value::Undefined,
        }
    }
}

pub(super) struct Interpreter {
    variables: HashMap<String, Value>,
    cleanups: Vec<Cleanup>,
    function_locals: Vec<Vec<String>>,
}

impl Interpreter {
    pub(super) fn new() -> Self {
        let mut variables = HashMap::new();

        variables.insert("undefined".to_owned(), Value::Undefined);

        variables.insert(
            "assert_equals".to_owned(),
            Value::NativeFunction(NativeFunction::AssertEquals),
        );

        variables.insert(
            "test".to_owned(),
            Value::NativeFunction(NativeFunction::Test),
        );

        variables.insert(
            "assert_true".to_owned(),
            Value::NativeFunction(NativeFunction::AssertTrue),
        );

        variables.insert(
            "assert_false".to_owned(),
            Value::NativeFunction(NativeFunction::AssertFalse),
        );

        variables.insert(
            "assert_not_equals".to_owned(),
            Value::NativeFunction(NativeFunction::AssertNotEquals),
        );

        variables.insert(
            "HTMLDivElement".to_owned(),
            Value::ElementConstructor("div".to_owned()),
        );

        variables.insert("Object".to_owned(), Value::ObjectConstructor);

        Self {
            variables,
            cleanups: Vec::new(),
            function_locals: Vec::new(),
        }
    }

    pub(super) fn set_document(&mut self, document: DocumentState) {
        self.variables
            .insert("document".to_owned(), Value::Document(document));
    }

    pub(super) fn take_document(&mut self) -> Option<DocumentState> {
        let Value::Document(document) = self.variables.remove("document")? else {
            return None;
        };

        Some(document)
    }

    pub(super) fn execute(&mut self, program: &[Statement]) -> Result<Completion, JsError> {
        self.execute_statements(program)
    }
}
