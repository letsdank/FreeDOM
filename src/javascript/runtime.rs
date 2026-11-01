use std::collections::HashMap;

use crate::url::Url;

use super::value::{FunctionValue, ObjectStorage};
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

#[path = "runtime/instanceof.rs"]
mod instanceof;

#[path = "runtime/function.rs"]
mod function;

#[path = "runtime/construct.rs"]
mod construct;

#[path = "runtime/function_call.rs"]
mod function_call;

#[path = "runtime/function_apply.rs"]
mod function_apply;

#[path = "runtime/array_like.rs"]
mod array_like;

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

#[path = "runtime/object_to_string.rs"]
mod object_to_string;

#[path = "runtime/array_to_string.rs"]
mod array_to_string;

#[path = "runtime/for_in.rs"]
mod for_in;

#[path = "runtime/switch.rs"]
mod switch;

#[path = "runtime/for_of_array.rs"]
mod for_of_array;

#[derive(Clone, Debug)]
struct Cleanup {
    callback: FunctionValue,
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
    object_prototype: ObjectStorage,
}

impl Interpreter {
    pub(super) fn new() -> Self {
        let mut variables = HashMap::new();

        // Most intrinsic properties are not implemented yet.
        let Value::Object(object_prototype) = Value::intrinsic_object_prototype() else {
            unreachable!("object constructor returns an object");
        };

        object_prototype.borrow_mut().define_own_data_property(
            "constructor".to_owned(),
            Value::ObjectConstructor,
            false,
        );

        object_prototype.borrow_mut().define_own_data_property(
            "hasOwnProperty".to_owned(),
            Value::NativeFunction(NativeFunction::ObjectHasOwnProperty),
            false,
        );

        object_prototype.borrow_mut().define_own_data_property(
            "toString".to_owned(),
            Value::NativeFunction(NativeFunction::ObjectToString),
            false,
        );

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
            object_prototype,
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
