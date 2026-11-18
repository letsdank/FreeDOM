use crate::url::Url;
use std::collections::{HashMap, VecDeque};

use super::value::{ArrayStorage, DocumentStorage, FunctionValue, ObjectStorage};
use super::{
    ArrayMethodKind, BinaryOperator, DocumentMethod, DocumentState, ElementMethod, Expression,
    JsError, NativeFunction, PostfixStep, Statement, TestContextMethod, UserFunction, Value,
};

use self::bindings::{Bindings, CapturedBindings};

const DEFAULT_EXECUTION_STEPS: usize = 1_000_000;
const MAX_EXECUTION_DEPTH: usize = 32;

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

#[path = "runtime/identifier_analysis.rs"]
mod identifier_analysis;

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

#[path = "runtime/reference.rs"]
mod reference;

#[path = "runtime/postfix.rs"]
mod postfix;

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

#[path = "runtime/string_replace.rs"]
mod string_replace;

#[path = "runtime/array_map.rs"]
mod array_map;

#[path = "runtime/array_some.rs"]
mod array_some;

#[path = "runtime/array_slice.rs"]
mod array_slice;

#[path = "runtime/error.rs"]
mod error;

#[path = "runtime/canvas.rs"]
mod canvas;

#[path = "runtime/webidl.rs"]
mod webidl;

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
    document: Option<DocumentStorage>,
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
    array_prototype: ArrayStorage,
    active_array_joins: Vec<ArrayStorage>,
    canvas_contexts: HashMap<u64, ObjectStorage>,
    canvas_surfaces: HashMap<u64, crate::graphics::canvas::Canvas2D>,
    canvas_dashes: HashMap<u64, Vec<f64>>,
    canvas_state_stacks: HashMap<u64, Vec<canvas::CanvasSavedState>>,
    canvas_saved_states: usize,
    canvas_saved_dash_values: usize,
    execution_limit: usize,
    steps_remaining: usize,
    execution_depth: usize,
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

        object_prototype.borrow_mut().define_own_data_property(
            "valueOf".to_owned(),
            Value::NativeFunction(NativeFunction::ObjectValueOf),
            false,
        );

        let Value::Array(array_prototype) = Value::array(Vec::new()) else {
            unreachable!("array constructor returns an array");
        };

        {
            let mut prototype = array_prototype.borrow_mut();

            for (name, function) in [
                ("constructor", NativeFunction::ArrayConstructor),
                ("slice", NativeFunction::ArraySlice),
                ("map", NativeFunction::ArrayMap),
                ("some", NativeFunction::ArraySome),
                ("toString", NativeFunction::ArrayToString),
            ] {
                prototype.properties.define_own_data_property(
                    name.to_owned(),
                    Value::NativeFunction(function),
                    false,
                );
            }
        }

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
            document: None,
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
            array_prototype,
            active_array_joins: Vec::new(),
            canvas_contexts: HashMap::new(),
            canvas_surfaces: HashMap::new(),
            canvas_dashes: HashMap::new(),
            canvas_state_stacks: HashMap::new(),
            canvas_saved_states: 0,
            canvas_saved_dash_values: 0,
            execution_limit: DEFAULT_EXECUTION_STEPS,
            steps_remaining: DEFAULT_EXECUTION_STEPS,
            execution_depth: 0,
        }
    }

    pub(super) fn set_document(&mut self, document: DocumentState) {
        let document = std::rc::Rc::new(std::cell::RefCell::new(document));

        self.document = Some(document.clone());

        self.variables
            .insert("document".to_owned(), Value::Document(document));
    }

    pub(super) fn snapshot_document(&mut self) -> Option<DocumentState> {
        self.current_document().map(|document| document.clone())
    }

    pub(super) fn execute(&mut self, program: &[Statement]) -> Result<Completion, JsError> {
        self.consume_step()?;

        let mut result =
            self.with_execution_frame(|interpreter| interpreter.execute_statements(program));

        if let Err(error) = &mut result {
            if let super::JsErrorKind::NativeException(value) = &mut error.kind {
                self.attach_native_error_prototype(value);
            }
        }

        result
    }

    pub(super) fn execute_script(&mut self, program: &[Statement]) -> Result<Completion, JsError> {
        self.steps_remaining = self.execution_limit;
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

    fn consume_step(&mut self) -> Result<(), JsError> {
        self.steps_remaining = self
            .steps_remaining
            .checked_sub(1)
            .ok_or_else(|| JsError::resource_limit("JavaScript execution budget exceeded"))?;

        Ok(())
    }

    fn with_execution_frame<T>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> Result<T, JsError>,
    ) -> Result<T, JsError> {
        if self.execution_depth >= MAX_EXECUTION_DEPTH {
            return Err(JsError::resource_limit(
                "JavaScript execution depth limit exceeded",
            ));
        }

        self.execution_depth += 1;

        let result = operation(self);

        self.execution_depth -= 1;

        result
    }
}

#[cfg(test)]
mod tests {
    use super::Interpreter;
    use crate::javascript::{JsErrorKind, Value, parse_program};

    #[test]
    fn execution_guard_bounds_loops_and_microtasks() {
        let mut interpreter = Interpreter::new();

        // Keep the test fast; normal execution uses the default limit.
        interpreter.execution_limit = 256;

        for source in [
            "for (;;) {}",
            "function again() { queueMicrotask(again); } queueMicrotask(again);",
        ] {
            let program = parse_program(source).unwrap();

            let error = interpreter.execute_script(&program).err().unwrap();

            assert!(matches!(error.kind, JsErrorKind::ResourceLimit));
            assert!(error.message.contains("execution budget"));

            let next_program = parse_program("42;").unwrap();

            let result = interpreter
                .execute_script(&next_program)
                .unwrap()
                .into_value();

            assert_eq!(result, Value::Number(42.0));
        }
    }

    #[test]
    fn execution_depth_limit_unwinds_and_resets() {
        let mut interpreter = Interpreter::new();

        for source in [
            "function direct() { direct(); } direct();",
            "function nested() { if (true) { nested(); } } nested();",
        ] {
            let program = parse_program(source).unwrap();

            let error = interpreter.execute_script(&program).err().unwrap();

            assert_eq!(error.kind, JsErrorKind::ResourceLimit);
            assert!(error.message.contains("execution depth"));

            assert_eq!(interpreter.execution_depth, 0);

            let next_program = parse_program("42;").unwrap();

            let result = interpreter
                .execute_script(&next_program)
                .unwrap()
                .into_value();

            assert_eq!(result, Value::Number(42.0));
        }
    }
}
