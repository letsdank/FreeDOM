use crate::url::Url;
use std::collections::{HashMap, VecDeque};

use super::value::{FunctionValue, ObjectStorage};
use super::{
    ArrayMethodKind, BinaryOperator, DocumentMethod, DocumentState, ElementMethod, Expression,
    JsError, NativeFunction, Statement, TestContextMethod, UserFunction, Value,
};

use self::bindings::{Bindings, CapturedBindings};

#[path = "runtime/bindings.rs"]
pub(super) mod bindings;

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

#[path = "runtime/references.rs"]
mod references;

#[path = "runtime/construct.rs"]
mod construct;

#[path = "runtime/function_call.rs"]
mod function_call;

#[path = "runtime/function_apply.rs"]
mod function_apply;

#[path = "runtime/function_bind.rs"]
mod function_bind;

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

#[path = "runtime/microtasks.rs"]
mod microtasks;

#[path = "runtime/promise.rs"]
pub(super) mod promise;

#[path = "runtime/string.rs"]
mod string;

#[path = "runtime/array_map.rs"]
mod array_map;

#[path = "runtime/array_some.rs"]
mod array_some;

#[path = "runtime/error.rs"]
mod error;

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
    variables: Bindings,
    cleanups: Vec<Cleanup>,
    function_locals: Vec<Vec<String>>,
    object_prototype: ObjectStorage,
    capture_frames: Vec<CapturedBindings>,
    catch_bindings: Vec<(usize, String)>,
    microtasks: VecDeque<microtasks::Microtask>,
    performing_microtask_checkpoint: bool,
    promise_prototype: ObjectStorage,
    error_prototype: ObjectStorage,
    type_error_prototype: ObjectStorage,
    reference_error_prototype: ObjectStorage,
}

impl Interpreter {
    /// Creates a clean environment for running the upstream WPT harness.
    ///
    /// Legacy WPT helpers remain installed by `new` until existing tests are migrated.
    pub(super) fn new_without_wpt_shim() -> Self {
        let mut interpreter = Self::new();

        for name in [
            "test",
            "assert_equals",
            "assert_true",
            "assert_false",
            "assert_not_equals",
        ] {
            interpreter.variables.remove(name);
        }

        interpreter
    }

    pub(super) fn new() -> Self {
        let mut variables = Bindings::default();

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

        let Value::Object(error_prototype) = Value::object_with_prototype(object_prototype.clone())
        else {
            unreachable!("object constructor returns an object");
        };

        error_prototype.borrow_mut().define_own_data_property(
            "name".to_owned(),
            Value::String("Error".to_owned()),
            false,
        );

        error_prototype.borrow_mut().define_own_data_property(
            "message".to_owned(),
            Value::String(String::new()),
            false,
        );

        error_prototype.borrow_mut().define_own_data_property(
            "constructor".to_owned(),
            Value::NativeFunction(NativeFunction::ErrorConstructor),
            false,
        );

        error_prototype.borrow_mut().define_own_data_property(
            "toString".to_owned(),
            Value::NativeFunction(NativeFunction::ErrorToString),
            false,
        );

        let type_error_prototype = Self::make_error_subtype_prototype(
            &error_prototype,
            "TypeError",
            NativeFunction::TypeErrorConstructor,
        );

        let reference_error_prototype = Self::make_error_subtype_prototype(
            &error_prototype,
            "ReferenceError",
            NativeFunction::ReferenceErrorConstructor,
        );

        let Value::Object(promise_prototype) =
            Value::object_with_prototype(object_prototype.clone())
        else {
            unreachable!("object constructor returns an object");
        };

        promise_prototype.borrow_mut().define_own_data_property(
            "then".to_owned(),
            Value::NativeFunction(NativeFunction::PromiseThen),
            false,
        );

        promise_prototype.borrow_mut().define_own_data_property(
            "constructor".to_owned(),
            Value::NativeFunction(NativeFunction::PromiseConstructor),
            false,
        );

        variables.insert("undefined".to_owned(), Value::Undefined);

        variables.insert(
            "queueMicrotask".to_owned(),
            Value::NativeFunction(NativeFunction::QueueMicrotask),
        );

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

        variables.insert(
            "Array".to_owned(),
            Value::NativeFunction(NativeFunction::ArrayConstructor),
        );

        variables.insert(
            "Error".to_owned(),
            Value::NativeFunction(NativeFunction::ErrorConstructor),
        );

        variables.insert(
            "TypeError".to_owned(),
            Value::NativeFunction(NativeFunction::TypeErrorConstructor),
        );

        variables.insert(
            "ReferenceError".to_owned(),
            Value::NativeFunction(NativeFunction::ReferenceErrorConstructor),
        );

        variables.insert(
            "Promise".to_owned(),
            Value::NativeFunction(NativeFunction::PromiseConstructor),
        );

        variables.insert(
            "String".to_owned(),
            Value::NativeFunction(NativeFunction::StringConstructor),
        );

        Self {
            variables,
            cleanups: Vec::new(),
            function_locals: Vec::new(),
            object_prototype,
            capture_frames: Vec::new(),
            catch_bindings: Vec::new(),
            microtasks: VecDeque::new(),
            performing_microtask_checkpoint: false,
            promise_prototype,
            error_prototype,
            type_error_prototype,
            reference_error_prototype,
        }
    }

    pub(super) fn set_document(&mut self, document: DocumentState) {
        self.variables
            .insert("document".to_owned(), Value::Document(document));
    }

    pub(super) fn snapshot_document(&mut self) -> Option<DocumentState> {
        self.current_document().map(|document| document.clone())
    }

    pub(super) fn execute(&mut self, program: &[Statement]) -> Result<Completion, JsError> {
        let mut result = self.execute_statements(program);

        if let Err(error) = &mut result {
            if let super::JsErrorKind::NativeException(value) = &mut error.kind {
                self.attach_native_error_prototype(value);
            }
        }

        result
    }

    pub(super) fn execute_script(&mut self, program: &[Statement]) -> Result<Completion, JsError> {
        let result = self.execute(program);

        if let Err(error) = &result {
            if !matches!(
                error.kind,
                super::JsErrorKind::Thrown(_) | super::JsErrorKind::NativeException(_)
            ) {
                self.microtasks.clear();
                return result;
            }
        }

        if let Err(error) = self.perform_microtask_checkpoint() {
            self.microtasks.clear();
            return Err(error);
        }

        result
    }
}
