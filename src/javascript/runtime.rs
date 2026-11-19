use self::bindings::{BindingSnapshot, Bindings};
use super::value::{ArrayStorage, DocumentStorage, FunctionValue, ObjectStorage};
use super::{
    ArrayMethodKind, BinaryOperator, DocumentMethod, DocumentState, ElementMethod, Expression,
    HostError, HostRoots, HostValue, JsError, NativeFunction, PostfixStep, Statement,
    TestContextMethod, UserFunction, Value,
};
use crate::url::Url;
use std::collections::{HashMap, VecDeque};

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

enum ExecutionFrame {
    PendingCompletion(Result<Completion, JsError>),
    ShadowedBindings(Vec<BindingSnapshot>),
    ForOfItems(std::vec::IntoIter<Value>),
    TemporaryValues(Vec<Value>),
    Reference(reference::Reference),
    PendingCall(PendingCall),
    ActiveCall(ActiveCallFrame),
    Microtask(microtasks::Microtask),
}

struct ActiveCallFrame {
    callee: FunctionValue,
    arguments: Vec<Value>,
    replaced_bindings: Vec<BindingSnapshot>,
    local_names: Option<Vec<String>>,
}

struct PendingCall {
    callee: Value,
    receiver: Option<Value>,
    arguments: Vec<Value>,
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
    execution_frames: Vec<ExecutionFrame>,
    document: Option<DocumentStorage>,
    cleanups: Vec<Cleanup>,
    object_prototype: ObjectStorage,
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
    host_roots: std::rc::Rc<HostRoots>,
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
            execution_frames: Vec::new(),
            document: None,
            cleanups: Vec::new(),
            object_prototype,
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
            host_roots: std::rc::Rc::new(HostRoots::default()),
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

        let (result, ()) = self.with_pending_completion(result, |interpreter| {
            let checkpoint = interpreter.perform_microtask_checkpoint();

            if checkpoint.is_err() {
                interpreter.microtasks.clear();
            }

            checkpoint
        })?;

        result
    }

    // Pending completions are future GC roots, but this is not a GC safepoint yet.
    fn with_pending_completion<T>(
        &mut self,
        pending: Result<Completion, JsError>,
        operation: impl FnOnce(&mut Self) -> Result<T, JsError>,
    ) -> Result<(Result<Completion, JsError>, T), JsError> {
        self.execution_frames
            .push(ExecutionFrame::PendingCompletion(pending));

        let outcome = operation(self);

        let pending = match self.execution_frames.pop() {
            Some(ExecutionFrame::PendingCompletion(pending)) => pending,
            _ => unreachable!("pending completion frame must exist"),
        };

        Ok((pending, outcome?))
    }

    fn with_shadowed_bindings<T>(
        &mut self,
        bindings: impl IntoIterator<Item = (String, Value)>,
        operation: impl FnOnce(&mut Self) -> Result<T, JsError>,
    ) -> Result<T, JsError> {
        let snapshots = bindings
            .into_iter()
            .map(|(name, value)| self.variables.shadow(name, value))
            .collect();

        self.execution_frames
            .push(ExecutionFrame::ShadowedBindings(snapshots));

        let result = operation(self);

        let snapshots = match self.execution_frames.pop() {
            Some(ExecutionFrame::ShadowedBindings(snapshots)) => snapshots,
            _ => unreachable!("shadowed bindings frame must exist"),
        };

        for snapshot in snapshots.into_iter().rev() {
            self.variables.restore(snapshot);
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

    fn with_temporary_values_scope<T>(
        &mut self,
        operation: impl FnOnce(&mut Self, usize) -> Result<T, JsError>,
    ) -> Result<T, JsError> {
        let index = self.execution_frames.len();

        self.execution_frames
            .push(ExecutionFrame::TemporaryValues(Vec::new()));

        let result = operation(self, index);

        match self.execution_frames.pop() {
            Some(ExecutionFrame::TemporaryValues(_)) => {}
            _ => unreachable!("temporary values frame must exist"),
        }

        result
    }

    fn with_pending_call<T>(
        &mut self,
        callee: Value,
        receiver: Option<Value>,
        expressions: &[Expression],
        operation: impl FnOnce(&mut Self, usize) -> Result<T, JsError>,
    ) -> Result<T, JsError> {
        self.with_call_frame(
            PendingCall {
                callee,
                receiver,
                arguments: Vec::with_capacity(expressions.len()),
            },
            |interpreter, index| {
                for expression in expressions {
                    let argument = interpreter.evaluate_expression(expression)?;

                    interpreter.pending_call_mut(index).arguments.push(argument);
                }

                operation(interpreter, index)
            },
        )
    }

    fn with_call_frame<T>(
        &mut self,
        call: PendingCall,
        operation: impl FnOnce(&mut Self, usize) -> Result<T, JsError>,
    ) -> Result<T, JsError> {
        let index = self.execution_frames.len();

        self.execution_frames
            .push(ExecutionFrame::PendingCall(call));

        let result = operation(self, index);

        match self.execution_frames.pop() {
            Some(ExecutionFrame::PendingCall(_)) => {}
            _ => unreachable!("pending call frame must exist"),
        }

        result
    }

    fn pending_call(&self, index: usize) -> &PendingCall {
        match self.execution_frames.get(index) {
            Some(ExecutionFrame::PendingCall(call)) => call,
            _ => unreachable!("pending call frame must exist"),
        }
    }

    fn pending_call_mut(&mut self, index: usize) -> &mut PendingCall {
        match self.execution_frames.get_mut(index) {
            Some(ExecutionFrame::PendingCall(call)) => call,
            _ => unreachable!("pending call frame must exist"),
        }
    }

    fn with_for_of_items<T>(
        &mut self,
        items: Vec<Value>,
        operation: impl FnOnce(&mut Self, usize) -> Result<T, JsError>,
    ) -> Result<T, JsError> {
        let index = self.execution_frames.len();

        self.execution_frames
            .push(ExecutionFrame::ForOfItems(items.into_iter()));

        let result = operation(self, index);

        match self.execution_frames.pop() {
            Some(ExecutionFrame::ForOfItems(_)) => {}
            _ => unreachable!("for-of frame must exist"),
        }

        result
    }

    fn next_for_of_item(&mut self, index: usize) -> Option<Value> {
        match self.execution_frames.get_mut(index) {
            Some(ExecutionFrame::ForOfItems(items)) => items.next(),
            _ => unreachable!("for-of frame must exist"),
        }
    }

    fn with_frame_value<T>(
        &mut self,
        value: Value,
        operation: impl FnOnce(&mut Self, usize) -> Result<T, JsError>,
    ) -> Result<T, JsError> {
        self.with_temporary_values_scope(|interpreter, index| {
            interpreter.frame_values_mut(index).push(value);

            operation(interpreter, index)
        })
    }

    fn frame_values(&self, index: usize) -> &[Value] {
        match self.execution_frames.get(index) {
            Some(ExecutionFrame::TemporaryValues(values)) => values,
            _ => unreachable!("temporary value frame must exist"),
        }
    }

    fn frame_values_mut(&mut self, index: usize) -> &mut Vec<Value> {
        match self.execution_frames.get_mut(index) {
            Some(ExecutionFrame::TemporaryValues(values)) => values,
            _ => unreachable!("temporary values frame must exist"),
        }
    }

    fn frame_value(&self, index: usize) -> &Value {
        self.frame_values(index)
            .first()
            .expect("temporary value must exist")
    }

    fn set_frame_value(&mut self, index: usize, value: Value) {
        *self
            .frame_values_mut(index)
            .first_mut()
            .expect("temporary value must exist") = value;
    }

    fn active_call_frame(&self, index: usize) -> &ActiveCallFrame {
        match self.execution_frames.get(index) {
            Some(ExecutionFrame::ActiveCall(frame)) => frame,
            _ => unreachable!("active call frame must exist"),
        }
    }

    fn active_call_frame_mut(&mut self, index: usize) -> &mut ActiveCallFrame {
        match self.execution_frames.get_mut(index) {
            Some(ExecutionFrame::ActiveCall(frame)) => frame,
            _ => unreachable!("active call frame must exist"),
        }
    }

    fn current_active_function(&self) -> Option<&ActiveCallFrame> {
        self.execution_frames
            .iter()
            .rev()
            .find_map(|frame| match frame {
                ExecutionFrame::ActiveCall(frame) => Some(frame),
                _ => None,
            })
    }

    fn function_depth(&self) -> usize {
        self.execution_frames
            .iter()
            .filter(|frame| {
                matches!(
                    frame,
                    ExecutionFrame::ActiveCall(frame) if frame.local_names.is_some()
                )
            })
            .count()
    }

    pub(super) fn export_value(&self, value: Value) -> HostValue {
        self.host_roots.register(value)
    }

    pub(super) fn export_error(&self, error: JsError) -> HostError {
        HostError::from_internal(error, &self.host_roots)
    }
}

#[cfg(test)]
mod tests {
    use super::{Completion, ExecutionFrame, Interpreter};
    use crate::javascript::{JsError, JsErrorKind, Value, parse_program};

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
            assert!(interpreter.execution_frames.is_empty());

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

    #[test]
    fn pending_completions_restore_after_nested_errors() {
        let mut interpreter = Interpreter::new();
        let original = Err(JsError::thrown(Value::Number(7.0)));

        let (result, ()) = interpreter
            .with_pending_completion(original, |runtime| {
                assert_eq!(runtime.execution_frames.len(), 1);

                assert!(matches!(
                    &runtime.execution_frames[0],
                    ExecutionFrame::PendingCompletion(Err(JsError {
                        kind: JsErrorKind::Thrown(Value::Number(7.0)),
                        ..
                    }))
                ));

                let nested = runtime.with_pending_completion(
                    Ok(Completion::Normal(Value::Null)),
                    |runtime| {
                        assert_eq!(runtime.execution_frames.len(), 2);
                        Err::<(), _>(JsError::resource_limit("test limit"))
                    },
                );

                assert!(nested.is_err());
                assert_eq!(runtime.execution_frames.len(), 1);

                Ok(())
            })
            .unwrap();

        assert!(matches!(
            result,
            Err(JsError {
                kind: JsErrorKind::Thrown(Value::Number(7.0)),
                ..
            })
        ));

        assert!(interpreter.execution_frames.is_empty());
    }

    #[test]
    fn script_exception_keeps_identity_across_microtask_checkpoint() {
        let mut interpreter = Interpreter::new();

        let program = parse_program(
            "let value = { x: 1 }; \
            queueMicrotask(function() { value.x = 2; }); \
            throw value;",
        )
        .unwrap();

        let error = interpreter.execute_script(&program).err().unwrap();

        let JsErrorKind::Thrown(value) = error.kind else {
            panic!("expected thrown value");
        };

        assert_eq!(
            interpreter.member_value(value, "x").unwrap(),
            Value::Number(2.0)
        );

        assert!(interpreter.execution_frames.is_empty());
    }

    #[test]
    fn active_call_frames_are_restored_after_nested_calls_and_errors() {
        let mut interpreter = Interpreter::new();

        let program = parse_program(
            r#"
            let x = 42;
            function inner(x) { return x + 1; }
            function outer(x = inner(4), ...rest) { return x + rest[0]; }
            outer(undefined, 3) === 8 && x === 42;
            "#,
        )
        .unwrap();

        let result = interpreter.execute_script(&program).unwrap().into_value();

        assert_eq!(result, Value::Boolean(true));
        assert!(interpreter.execution_frames.is_empty());

        let program = parse_program("function tail(x = missing) {} fail();").unwrap();

        assert!(interpreter.execute_script(&program).is_err());
        assert!(interpreter.execution_frames.is_empty());

        let program = parse_program("x;").unwrap();

        assert_eq!(
            interpreter.execute_script(&program).unwrap().into_value(),
            Value::Number(42.0)
        );
    }

    #[test]
    fn binary_temporary_values_are_scoped_across_nested_calls_and_errors() {
        let mut interpreter = Interpreter::new();

        interpreter
            .execution_frames
            .push(ExecutionFrame::TemporaryValues(vec![Value::Number(77.0)]));

        let program = parse_program(
            r#"
            let trace = '';
            let left = {
                valueOf: function() {
                    trace = trace + 'l';
                    return 2;
                }
            };
            let right = {
                valueOf: function() {
                    trace = trace + 'r';
                    return 3;
                }
            };

            function nested() {
                trace = trace + 'n';
                return right;
            }

            left + nested() === 5 && trace === 'nlr';
            "#,
        )
        .unwrap();

        let result = interpreter.execute_script(&program).unwrap().into_value();

        assert_eq!(result, Value::Boolean(true));
        assert_eq!(interpreter.frame_values(0), &[Value::Number(77.0)]);
        assert_eq!(interpreter.execution_frames.len(), 1);

        for source in [
            "1 + missing;",
            "({valueOf: function() { throw 'error'; }}) + 1;",
        ] {
            let program = parse_program(source).unwrap();

            assert!(interpreter.execute_script(&program).is_err());
            assert_eq!(interpreter.frame_values(0), &[Value::Number(77.0)]);
            assert_eq!(interpreter.execution_frames.len(), 1);
        }
    }

    #[test]
    fn assignment_references_survive_nested_calls_and_errors() {
        let mut interpreter = Interpreter::new();

        let program = parse_program(
            r#"
            let target = {a: 1};
            let original = target;
            let key = 'a';

            function mutate() {
                target = {a: 100};
                key = 'b';
                return 2;
            }

            original[key] += mutate();

            original.a === 3 && target.a === 100 && key === 'b';
            "#,
        )
        .unwrap();

        let result = interpreter.execute_script(&program).unwrap().into_value();

        assert_eq!(result, Value::Boolean(true));
        assert!(interpreter.execution_frames.is_empty());

        for source in [
            "original.a = missing;",
            "original.a += missing;",
            "original.a += ({ valueOf: function() { throw 'error'; } });",
        ] {
            let program = parse_program(source).unwrap();

            assert!(interpreter.execute_script(&program).is_err());
            assert!(interpreter.execution_frames.is_empty());
        }
    }

    #[test]
    fn indexed_updates_restore_temporary_roots_after_calls_and_errors() {
        let mut interpreter = Interpreter::new();

        let program = parse_program(
            r#"
            let calls = 0;
            let target = {
                x: {
                    valueOf: function() {
                        calls = calls + 1;
                        return 4;
                    }
                }
            };
            let original = target;

            function choose() {
                calls = calls + 1;
                target = {x: 99};
                return 'x';
            }

            let before = target[choose()]++;

            before === 4 &&
                original.x === 5 &&
                target.x === 99 &&
                calls === 2;
            "#,
        )
        .unwrap();

        let result = interpreter.execute_script(&program).unwrap().into_value();

        assert_eq!(result, Value::Boolean(true));
        assert!(interpreter.execution_frames.is_empty());

        for source in [
            "original[missing]++;",
            "({x: {valueOf: function() { throw 'error'; }}}).x++;",
        ] {
            let program = parse_program(source).unwrap();

            assert!(interpreter.execute_script(&program).is_err());
            assert!(interpreter.execution_frames.is_empty());
        }
    }

    #[test]
    fn pending_postfix_calls_are_scoped_nested_calls_and_errors() {
        let mut interpreter = Interpreter::new();

        let program = parse_program(
            r#"
            let trace = '';
            let target = {
                factor: 10,
                method: function(a, b) {
                    return this.factor + a.value + b;
                }
            };
            let original = target;

            function first() {
                trace = trace + 'a';
                return {value: 3};
            }

            function second() {
                trace = trace + 'b';
                target = {};
                return 2;
            }

            function fail() {
                throw 'error';
            }

            let result = target.method(first(), second());

            result === 15 && trace === 'ab' && target !== original;
            "#,
        )
        .unwrap();

        let result = interpreter.execute_script(&program).unwrap().into_value();

        assert_eq!(result, Value::Boolean(true));
        assert!(interpreter.execution_frames.is_empty());

        for source in [
            "original.method({}, missing);",
            "original.method({}, fail());",
            "original.factor();",
        ] {
            let program = parse_program(source).unwrap();

            assert!(interpreter.execute_script(&program).is_err());
            assert!(interpreter.execution_frames.is_empty());
        }
    }

    #[test]
    fn literal_temporaries_are_scoped_and_preserve_holes() {
        let mut interpreter = Interpreter::new();

        interpreter
            .execution_frames
            .push(ExecutionFrame::TemporaryValues(vec![Value::Number(77.0)]));

        let program = parse_program(
            r#"
            let trace = '';

            function next() {
                trace = trace + 'n';
                return {value:2};
            }

            let array = [{value: 1},, next()];
            let object = {
                first: array[0],
                second: next(),
                first: array[2]
            };

            array.length === 3 &&
                !array.hasOwnProperty(1) &&
                array[0].value === 1 &&
                object.first.value === 2 &&
                object.second.value === 2 &&
                trace === 'nn';
            "#,
        )
        .unwrap();

        let result = interpreter.execute_script(&program).unwrap().into_value();

        assert_eq!(result, Value::Boolean(true));
        assert_eq!(interpreter.frame_values(0), &[Value::Number(77.0)]);
        assert_eq!(interpreter.execution_frames.len(), 1);

        for source in ["[{x: 1}, missing];", "({x: {a: 1}, y: missing});"] {
            let program = parse_program(source).unwrap();

            assert!(interpreter.execute_script(&program).is_err());
            assert_eq!(interpreter.frame_values(0), &[Value::Number(77.0)]);
            assert_eq!(interpreter.execution_frames.len(), 1);
        }
    }

    #[test]
    fn constructor_arguments_are_scoped_across_nested_calls_and_errors() {
        let mut interpreter = Interpreter::new();

        let program = parse_program(
            r#"
            let trace = '';

            function A(a, b) {
                this.result = a.value + b.value;
            }

            let Constructor = A;

            function first() {
                trace = trace + 'a';
                return {value: 1};
            }

            function second() {
                trace = trace + 'b';
                Constructor = 4;
                return {value: 2};
            }

            let instance = new Constructor(first(), second());

            instance.result === 3 && trace === 'ab' && Constructor === 4;
            "#,
        )
        .unwrap();

        let result = interpreter.execute_script(&program).unwrap().into_value();

        assert_eq!(result, Value::Boolean(true));
        assert!(interpreter.execution_frames.is_empty());

        for source in ["new A(first(), missing);", "new Constructor(first());"] {
            let program = parse_program(source).unwrap();

            assert!(interpreter.execute_script(&program).is_err());
            assert!(interpreter.execution_frames.is_empty());
        }
    }

    #[test]
    fn regexp_arguments_and_apply_length_conversion_restore_temporaries() {
        let mut interpreter = Interpreter::new();

        interpreter
            .execution_frames
            .push(ExecutionFrame::TemporaryValues(vec![Value::Number(77.0)]));

        let program = parse_program(
            r#"
            let trace = '';

            function pattern() {
                trace = trace + 'p';
                return 'a';
            }

            function flags() {
                trace = trace + 'f';
                return 'i';
            }

            function extra() {
                trace = trace + 'e';
                return 0;
            }

            let regexp = new RegExp(pattern(), flags(), extra());

            let list = {
                0: 7,
                length: {
                    valueOf: function() {
                        list = null;
                        return 1;
                    }
                }
            };

            function first(a) {
                return a;
            }

            let result = first.apply(null, list);

            regexp.test('A') &&
                trace === 'pfe' &&
                result === 7 &&
                list === null;
            "#,
        )
        .unwrap();

        let result = interpreter.execute_script(&program).unwrap().into_value();

        assert_eq!(result, Value::Boolean(true));
        assert_eq!(interpreter.frame_values(0), &[Value::Number(77.0)]);
        assert_eq!(interpreter.execution_frames.len(), 1);

        for source in [
            "new RegExp('a', missing);",
            "new RegExp('a', 'z');",
            "first.apply(null, {length: {valueOf: function() { throw 'error'; }}});",
        ] {
            let program = parse_program(source).unwrap();

            assert!(interpreter.execute_script(&program).is_err());
            assert_eq!(interpreter.frame_values(0), &[Value::Number(77.0)]);
            assert_eq!(interpreter.execution_frames.len(), 1);
        }
    }

    #[test]
    fn finally_preserves_pending_values_and_unwinds_frames() {
        let mut interpreter = Interpreter::new();

        let program = parse_program(
            r#"
            let object = { value: 1 };

            function run() {
                try {
                    return object;
                } finally {
                    try {
                        object.value = 2;
                    } finally {
                        object.value = object.value + 1;
                    }
                }
            }

            run() === object && object.value === 3;
            "#,
        )
        .unwrap();

        let result = interpreter.execute_script(&program).unwrap().into_value();

        assert_eq!(result, Value::Boolean(true));
        assert!(interpreter.execution_frames.is_empty());

        let program = parse_program("try { throw 1; } finally { throw 2; }").unwrap();
        let error = interpreter.execute_script(&program).err().unwrap();

        assert!(matches!(
            error.kind,
            JsErrorKind::Thrown(Value::Number(2.0))
        ));

        assert!(interpreter.execution_frames.is_empty());
    }

    #[test]
    fn shadowed_binding_frames_unwind_after_catch_and_loops() {
        let mut interpreter = Interpreter::new();

        let program = parse_program("let x = {value: 7};").unwrap();
        interpreter.execute_script(&program).unwrap();

        for source in [
            "try { throw 1; } catch (x) { throw 2; }",
            "for (let x in {a: 1}) { throw 2; }",
            "for (let x of [1]) { throw 2; }",
            "for (const [x, y] of [[1, 2]]) { throw 2; }",
        ] {
            let program = parse_program(source).unwrap();

            assert!(interpreter.execute_script(&program).is_err(), "{source}");
            assert!(interpreter.execution_frames.is_empty(), "{source}");

            let program = parse_program("x.value;").unwrap();

            assert_eq!(
                interpreter.execute_script(&program).unwrap().into_value(),
                Value::Number(7.0)
            );
        }
    }

    #[test]
    fn for_of_items_remain_in_frames_and_unwind_on_errors() {
        let mut interpreter = Interpreter::new();

        let result = interpreter.with_for_of_items(
            vec![Value::Number(1.0), Value::Number(2.0)],
            |runtime, index| {
                assert_eq!(runtime.next_for_of_item(index), Some(Value::Number(1.0)));

                runtime.with_shadowed_bindings([(String::from("x"), Value::Undefined)], |runtime| {
                    match &runtime.execution_frames[index] {
                        ExecutionFrame::ForOfItems(items) => {
                            assert_eq!(items.as_slice(), &[Value::Number(2.0)]);
                        }

                        _ => panic!("expected for-of frame"),
                    }

                    Err::<(), _>(JsError::resource_limit("test limit"))
                })
            },
        );

        assert!(result.is_err());
        assert!(interpreter.execution_frames.is_empty());

        let program = parse_program(
            r#"
            let source = [{value: 1}, {value: 2}];
            let sum = 0;

            for (let item of source) {
                source = [];

                for (let n of [3, 4]) {
                    sum = sum + n;
                }

                sum = sum + item.value;
            }

            let pairs = [[1, 2], [3, 4]];
            let pair_sum = 0;

            for (const [a, b] of pairs) {
                pairs = [];
                pair_sum = pair_sum + a + b;
            }

            sum === 17 && pair_sum === 10;
            "#,
        )
        .unwrap();

        let result = interpreter.execute_script(&program).unwrap().into_value();

        assert_eq!(result, Value::Boolean(true));
        assert!(interpreter.execution_frames.is_empty());
    }

    #[test]
    fn switch_and_while_keep_values_across_nested_calls() {
        let mut interpreter = Interpreter::new();

        let program = parse_program(
            r#"
            let target = {value: 7};
            let source = [target];
            let matched = false;

            function choose() {
                let result = source[0];
                target = null;
                source = [];
                return result;
            }

            switch (target) {
                case choose():
                    matched = true;
                    break;
            }

            matched && target === null;
            "#,
        )
        .unwrap();

        let result = interpreter.execute_script(&program).unwrap().into_value();

        assert_eq!(result, Value::Boolean(true));
        assert!(interpreter.execution_frames.is_empty());

        let program = parse_program(
            r#"
            let counter = 0;
            let remembered = {value: 7};

            function next() {
                counter = counter + 1;

                if (counter === 2) {
                    remembered = null;
                    return false;
                }

                return true;
            }

            while (next()) {
                remembered;
            }
            "#,
        )
        .unwrap();

        let result = interpreter.execute_script(&program).unwrap().into_value();

        assert_eq!(
            interpreter.member_value(result, "value").unwrap(),
            Value::Number(7.0)
        );
        assert!(interpreter.execution_frames.is_empty());

        for source in ["switch (1) { case missing: break; }", "while (missing) {}"] {
            let program = parse_program(source).unwrap();

            assert!(interpreter.execute_script(&program).is_err());
            assert!(interpreter.execution_frames.is_empty());
        }
    }

    #[test]
    fn reference_frames_survive_nested_frames_and_unwind_on_error() {
        let mut interpreter = Interpreter::new();
        let object = Value::array(vec![Value::Number(5.0)]);

        let reference = super::reference::Reference::Index {
            object: object.clone(),
            index: Value::Number(0.0),
        };

        let result = interpreter.with_active_reference(reference, |runtime, reference_index| {
            runtime.with_frame_value(Value::Number(9.0), |runtime, value_index| {
                assert_eq!(runtime.frame_value(value_index), &Value::Number(9.0));

                match runtime.active_reference(reference_index) {
                    super::reference::Reference::Index {
                        object: actual,
                        index,
                    } => {
                        assert_eq!(actual, &object);
                        assert_eq!(index, &Value::Number(0.0));
                    }

                    _ => panic!("expected index reference"),
                }

                Err::<(), _>(JsError::resource_limit("test limit"))
            })
        });

        assert!(result.is_err());
        assert!(interpreter.execution_frames.is_empty());
    }

    #[test]
    fn pending_call_frames_remain_accessible_during_nested_calls() {
        let mut interpreter = Interpreter::new();

        let arguments = [super::Expression::Literal(Value::Number(3.0))];

        let result = interpreter.with_pending_call(
            Value::Number(1.0),
            Some(Value::Number(2.0)),
            &arguments,
            |runtime, index| {
                let nested = runtime.with_pending_call(Value::Null, None, &[], |runtime, inner| {
                    assert_eq!(runtime.pending_call(index).callee, Value::Number(1.0));
                    assert_eq!(runtime.pending_call(inner).callee, Value::Null);

                    Err::<(), _>(JsError::resource_limit("test limit"))
                });

                assert!(nested.is_err());

                let call = runtime.pending_call(index);

                assert_eq!(call.receiver, Some(Value::Number(2.0)));
                assert_eq!(call.arguments, vec![Value::Number(3.0)]);

                Ok(())
            },
        );

        assert!(result.is_ok());
        assert!(interpreter.execution_frames.is_empty());
    }

    #[test]
    fn map_scopes_results_through_nested_callbacks_and_errors() {
        let mut interpreter = Interpreter::new();

        let program = parse_program(
            r#"
            let source = [1,,3];
            let calls = 0;

            let mapped = source.map(function(value, index) {
                calls = calls + 1;

                if (index === 0) {
                    source[2] = 4;
                }

                let nested = [2].map(function(n) {
                    return n + 1;
                });

                return {value: value + nested[0]};
            });

            mapped.length === 3 &&
                !mapped.hasOwnProperty(1) &&
                mapped[0].value === 4 &&
                mapped[2].value === 7 &&
                calls === 2;
            "#,
        )
        .unwrap();

        let result = interpreter.execute_script(&program).unwrap().into_value();

        assert_eq!(result, Value::Boolean(true));
        assert!(interpreter.execution_frames.is_empty());

        let program = parse_program(
            r#"
            [1, 2].map(function(value) {
                if (value === 2) {
                    throw 'error';
                }

                return {value: value};
            });
            "#,
        )
        .unwrap();

        assert!(interpreter.execute_script(&program).is_err());
        assert!(interpreter.execution_frames.is_empty());
    }

    #[test]
    fn internal_callback_dispatch_unwinds_frames() {
        let mut interpreter = Interpreter::new();

        let program = parse_program(
            r#"
            let trace = '';

            let found = [1, 2, 3].some(function(value) {
                trace = trace + value;
                return value === 2;
            });

            let replaced = 'aba'.replace(/a/g, function() {
                trace = trace + 'r';
                return 'x';
            });

            let object = {
                valueOf: function() {
                    trace = trace + 'v';
                    return {};
                },

                toString: function() {
                    trace = trace + 's';
                    return '4';
                }
            };

            found &&
                replaced === 'xbx' &&
                object + 1 === '41' &&
                trace === '12rrvs';
            "#,
        )
        .unwrap();

        let result = interpreter.execute_script(&program).unwrap().into_value();

        assert_eq!(result, Value::Boolean(true));
        assert!(interpreter.execution_frames.is_empty());

        let program = parse_program("[1].some(function() { throw 'error'; });").unwrap();

        assert!(interpreter.execute_script(&program).is_err());
        assert!(interpreter.execution_frames.is_empty());
    }

    #[test]
    fn host_roots_keep_owner_alive_and_release_without_registry_borrow() {
        use std::rc::Rc;

        let interpreter = Interpreter::new();
        let owner = Rc::downgrade(&interpreter.host_roots);

        let value = interpreter.export_value(Value::array(vec![Value::Number(7.0)]));
        let value_root = Rc::downgrade(&value.0);
        let clone = value.clone();

        let error = interpreter.export_error(JsError::thrown(Value::array(vec![])));

        let registry = owner.upgrade().unwrap();

        assert_eq!(
            registry
                .roots
                .borrow()
                .iter()
                .filter(|root| root.upgrade().is_some())
                .count(),
            2
        );

        drop(interpreter);
        drop(value);

        assert!(value_root.upgrade().is_some());

        let borrowed = registry.roots.borrow_mut();

        drop(clone);

        assert!(value_root.upgrade().is_none());

        drop(borrowed);

        assert_eq!(
            registry
                .roots
                .borrow()
                .iter()
                .filter(|root| root.upgrade().is_some())
                .count(),
            1
        );

        drop(error);
        drop(registry);

        assert!(owner.upgrade().is_none());
    }

    #[test]
    fn host_owner_keeps_its_heap_alive_and_separate() {
        use crate::javascript::heap::HeapObject;
        use crate::javascript::value::{ArrayState, ObjectState};
        use std::rc::Rc;

        let interpreter = Interpreter::new();
        let weak_owner = Rc::downgrade(&interpreter.host_roots);

        let host_value = interpreter.export_value(Value::Boolean(true));

        let object = interpreter
            .host_roots
            .heap()
            .unwrap()
            .borrow_mut()
            .allocate(HeapObject::Object(ObjectState::default()));

        let array = interpreter
            .host_roots
            .heap()
            .unwrap()
            .borrow_mut()
            .allocate(HeapObject::Array(ArrayState::default()));

        let other = Interpreter::new();

        assert!(
            other
                .host_roots
                .heap()
                .unwrap()
                .borrow()
                .get(object)
                .is_none()
        );

        drop(interpreter);

        let owner = weak_owner.upgrade().unwrap();
        let heap = owner.heap().unwrap().borrow();

        assert!(matches!(heap.get(object), Some(HeapObject::Object(_))));
        assert!(matches!(heap.get(array), Some(HeapObject::Array(_))));

        drop(heap);
        drop(owner);
        drop(host_value);

        assert!(weak_owner.upgrade().is_none());
    }
}
