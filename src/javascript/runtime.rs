use self::bindings::{BindingSnapshot, Bindings};
use super::arguments::ArgumentsValue;
use super::value::{
    ArrayStorage, DocumentStorage, FunctionValue, ObjectPrototype, ObjectStorage, RegExpValue,
};
use super::{
    ArrayMethodKind, BinaryOperator, DocumentMethod, DocumentState, ElementMethod, Expression,
    HostError, HostRoots, HostValue, JsError, NativeFunction, PostfixStep, Statement,
    TestContextMethod, UserFunction, Value,
};
use freedom_url::Url;
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

#[allow(dead_code)]
#[path = "runtime/trace.rs"]
mod trace;

#[derive(Clone, Debug)]
struct Cleanup {
    callback: FunctionValue,
    captured: HashMap<String, Value>,
}

#[derive(Clone)]
pub(super) enum Completion {
    Normal(Value),
    Return(Value),
    Break,
    Continue,
}

enum ExecutionFrame {
    PendingCompletion(Result<Completion, JsError>),
    ExceptionScope(Option<JsError>),
    ShadowedBindings(Vec<BindingSnapshot>),
    ForOfItems(std::vec::IntoIter<Value>),
    TemporaryValues(Vec<Value>),
    Reference(reference::Reference),
    PendingCall(PendingCall),
    ActiveCall(ActiveCallFrame),
    Microtask(microtasks::Microtask),
}

// These roots still use Rc-backed values and cannot be passed to the heap tracer.
#[allow(dead_code)]
pub(super) enum RuntimeRoot<'a> {
    Value(&'a Value),
    Function(&'a FunctionValue),
    Promise(&'a promise::PromiseHandle),
    Object(&'a ObjectStorage),
    Array(&'a ArrayStorage),
    Document(&'a DocumentStorage),
    RegExp(&'a RegExpValue),
    Arguments(&'a ArgumentsValue),
}

#[derive(Debug, PartialEq, Eq)]
#[expect(
    clippy::enum_variant_names,
    reason = "each variant names the storage that was already borrowed"
)]
pub(super) enum RootVisitError {
    BindingBorrowed,
    PromiseBorrowed,
    ObjectBorrowed,
    ArrayBorrowed,
    RegExpBorrowed,
}

impl ExecutionFrame {
    #[allow(dead_code)]
    fn visit_roots(&self, visit: &mut impl FnMut(RuntimeRoot<'_>)) -> Result<(), RootVisitError> {
        match self {
            Self::PendingCompletion(result) => match result {
                Ok(Completion::Normal(value) | Completion::Return(value)) => {
                    visit(RuntimeRoot::Value(value));
                }

                Ok(Completion::Break | Completion::Continue) => {}

                Err(error) => visit_error_roots(error, visit),
            },

            Self::ExceptionScope(error) => {
                if let Some(error) = error {
                    visit_error_roots(error, visit);
                }
            }

            Self::ShadowedBindings(bindings) => {
                for binding in bindings {
                    binding.visit_root(visit)?;
                }
            }

            Self::ForOfItems(items) => {
                for value in items.as_slice() {
                    visit(RuntimeRoot::Value(value));
                }
            }

            Self::TemporaryValues(values) => {
                for value in values {
                    visit(RuntimeRoot::Value(value));
                }
            }

            Self::Reference(reference) => {
                reference.visit_roots(visit);
            }

            Self::PendingCall(call) => {
                visit(RuntimeRoot::Value(&call.callee));

                if let Some(receiver) = &call.receiver {
                    visit(RuntimeRoot::Value(receiver));
                }

                for argument in &call.arguments {
                    visit(RuntimeRoot::Value(argument));
                }

                if let Some(value) = &call.handoff_value {
                    visit(RuntimeRoot::Value(value));
                }
            }

            Self::ActiveCall(call) => {
                visit(RuntimeRoot::Function(&call.callee));

                for argument in &call.arguments {
                    visit(RuntimeRoot::Value(argument));
                }

                for binding in &call.replaced_bindings {
                    binding.visit_root(visit)?;
                }

                if let Some(value) = &call.return_value {
                    visit(RuntimeRoot::Value(value));
                }
            }

            Self::Microtask(job) => {
                job.visit_roots(visit);
            }
        }

        Ok(())
    }
}

fn visit_error_roots<'a>(error: &'a JsError, visit: &mut impl FnMut(RuntimeRoot<'a>)) {
    match &error.kind {
        super::JsErrorKind::Thrown(value)
        | super::JsErrorKind::NativeException(super::NativeException::Materialized(value)) => {
            visit(RuntimeRoot::Value(value));
        }
        super::JsErrorKind::NativeException(super::NativeException::Pending { .. })
        | super::JsErrorKind::Syntax
        | super::JsErrorKind::UnclassifiedRuntime
        | super::JsErrorKind::UnsupportedFeature
        | super::JsErrorKind::ResourceLimit => {}
    }
}

struct ActiveCallFrame {
    callee: FunctionValue,
    arguments: Vec<Value>,
    replaced_bindings: Vec<BindingSnapshot>,
    local_names: Option<Vec<String>>,
    return_value: Option<Value>,
}

struct PendingCall {
    callee: Value,
    receiver: Option<Value>,
    arguments: Vec<Value>,
    handoff_value: Option<Value>,
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
    canvas: canvas::CanvasHost,
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
        let host_roots = std::rc::Rc::new(HostRoots::default());

        // Most intrinsic properties are not implemented yet.
        let Value::Object(object_prototype) =
            host_roots.create_ordinary_object([], ObjectPrototype::IntrinsicObjectRoot)
        else {
            unreachable!("object constructor returns an object");
        };

        host_roots.define_ordinary_property(
            &object_prototype,
            "constructor".to_owned(),
            Value::ObjectConstructor,
            false,
        );

        host_roots.define_ordinary_property(
            &object_prototype,
            "hasOwnProperty".to_owned(),
            Value::NativeFunction(NativeFunction::ObjectHasOwnProperty),
            false,
        );

        host_roots.define_ordinary_property(
            &object_prototype,
            "toString".to_owned(),
            Value::NativeFunction(NativeFunction::ObjectToString),
            false,
        );

        host_roots.define_ordinary_property(
            &object_prototype,
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

        let Value::Object(error_prototype) = host_roots
            .create_ordinary_object([], ObjectPrototype::Object(object_prototype.clone()))
        else {
            unreachable!("object constructor returns an object");
        };

        host_roots.define_ordinary_property(
            &error_prototype,
            "name".to_owned(),
            Value::String("Error".to_owned()),
            false,
        );

        host_roots.define_ordinary_property(
            &error_prototype,
            "message".to_owned(),
            Value::String(String::new()),
            false,
        );

        host_roots.define_ordinary_property(
            &error_prototype,
            "constructor".to_owned(),
            Value::NativeFunction(NativeFunction::ErrorConstructor),
            false,
        );

        host_roots.define_ordinary_property(
            &error_prototype,
            "toString".to_owned(),
            Value::NativeFunction(NativeFunction::ErrorToString),
            false,
        );

        let type_error_prototype = Self::make_error_subtype_prototype(
            &host_roots,
            &error_prototype,
            "TypeError",
            NativeFunction::TypeErrorConstructor,
        );

        let reference_error_prototype = Self::make_error_subtype_prototype(
            &host_roots,
            &error_prototype,
            "ReferenceError",
            NativeFunction::ReferenceErrorConstructor,
        );

        let Value::Object(promise_prototype) = host_roots
            .create_ordinary_object([], ObjectPrototype::Object(object_prototype.clone()))
        else {
            unreachable!("object constructor returns an object");
        };

        host_roots.define_ordinary_property(
            &promise_prototype,
            "then".to_owned(),
            Value::NativeFunction(NativeFunction::PromiseThen),
            false,
        );

        host_roots.define_ordinary_property(
            &promise_prototype,
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
            canvas: canvas::CanvasHost::default(),
            execution_limit: DEFAULT_EXECUTION_STEPS,
            steps_remaining: DEFAULT_EXECUTION_STEPS,
            execution_depth: 0,
            host_roots,
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

        let mut result = self.with_execution_frame(|interpreter| {
            let result = interpreter.execute_statements(program);
            interpreter.track_function_completion(&result);
            result
        });

        if let Err(error) = &mut result {
            self.materialize_native_exception(error);
            self.record_exception(error);
        }

        result
    }

    // Retain returns in the active function while nested statements unwind.
    fn track_function_completion(&mut self, result: &Result<Completion, JsError>) {
        if let Ok(Completion::Return(value)) = result {
            self.retain_function_return(value);
            return;
        }

        if let Some(ExecutionFrame::ActiveCall(call)) = self
            .execution_frames
            .iter_mut()
            .rev()
            .find(|frame| matches!(frame, ExecutionFrame::ActiveCall(_)))
        {
            match result {
                Err(_) | Ok(Completion::Break | Completion::Continue) => call.return_value = None,
                Ok(Completion::Normal(_) | Completion::Return(_)) => {}
            }
        }
    }

    fn retain_function_return(&mut self, value: &Value) {
        if let Some(ExecutionFrame::ActiveCall(call)) = self
            .execution_frames
            .iter_mut()
            .rev()
            .find(|frame| matches!(frame, ExecutionFrame::ActiveCall(_)))
        {
            call.return_value = Some(value.clone());
        }
    }

    // Keep the script result rooted through the checkpoint and its final consumer.
    fn with_script_completion<T>(
        &mut self,
        program: &[Statement],
        consume: impl FnOnce(&mut Self, usize) -> T,
    ) -> T {
        self.with_exception_scope(|interpreter| {
            Ok(interpreter.with_script_completion_inner(program, consume))
        })
        .expect("script completion consumer cannot fail as an engine operation")
    }

    fn with_script_completion_inner<T>(
        &mut self,
        program: &[Statement],
        consume: impl FnOnce(&mut Self, usize) -> T,
    ) -> T {
        self.steps_remaining = self.execution_limit;
        let result = self.execute(program);

        let run_checkpoint = match &result {
            Err(error) => matches!(
                error.kind,
                super::JsErrorKind::Thrown(_) | super::JsErrorKind::NativeException(_)
            ),
            Ok(_) => true,
        };

        if !run_checkpoint {
            self.microtasks.clear();
        }

        let index = self.execution_frames.len();
        let (_, output) = self
            .with_pending_completion(result, |interpreter| {
                // The completion frame owns the result during the checkpoint.
                interpreter.clear_exception();
                if run_checkpoint && let Err(error) = interpreter.perform_microtask_checkpoint() {
                    interpreter.microtasks.clear();
                    interpreter.replace_pending_completion(index, Err(error));
                }

                Ok(consume(interpreter, index))
            })
            .expect("script completion consumer cannot fail as an engine operation");

        output
    }

    // Test-only entry point: its returned value is not registered as a host root.
    #[cfg(test)]
    pub(super) fn execute_script(&mut self, program: &[Statement]) -> Result<Completion, JsError> {
        self.with_script_completion(program, |interpreter, index| {
            match &interpreter.execution_frames[index] {
                ExecutionFrame::PendingCompletion(result) => result.clone(),
                _ => unreachable!("pending completion frame must exist"),
            }
        })
    }

    /// Registers the result or exception while the script completion frame is still live.
    pub(super) fn execute_host_script(
        &mut self,
        program: &[Statement],
    ) -> Result<HostValue, HostError> {
        self.with_script_completion(program, |interpreter, index| {
            match &interpreter.execution_frames[index] {
                ExecutionFrame::PendingCompletion(Ok(completion)) => {
                    Ok(interpreter.export_value(completion.clone().into_value()))
                }
                ExecutionFrame::PendingCompletion(Err(error)) => {
                    Err(interpreter.export_error(error.clone()))
                }
                _ => unreachable!("pending completion frame must exist"),
            }
        })
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

    fn replace_pending_completion(&mut self, index: usize, result: Result<Completion, JsError>) {
        match self.execution_frames.get_mut(index) {
            Some(ExecutionFrame::PendingCompletion(pending)) => *pending = result,
            _ => unreachable!("pending completion frame must exist"),
        }
    }

    fn with_shadowed_bindings(
        &mut self,
        bindings: impl IntoIterator<Item = (String, Value)>,
        operation: impl FnOnce(&mut Self) -> Result<Completion, JsError>,
    ) -> Result<Completion, JsError> {
        let snapshots = bindings
            .into_iter()
            .map(|(name, value)| self.variables.shadow(name, value))
            .collect();

        let frame_index = self.execution_frames.len();
        self.execution_frames
            .push(ExecutionFrame::ShadowedBindings(snapshots));

        let result = operation(self);

        let (result, ()) = self
            .with_pending_completion(result, |interpreter| {
                interpreter.restore_frame_bindings(frame_index);
                Ok(())
            })
            .expect("restoring shadowed bindings cannot fail");

        match self.execution_frames.pop() {
            Some(ExecutionFrame::ShadowedBindings(_)) => {}
            _ => unreachable!("shadowed bindings frame must exist"),
        }

        result
    }

    fn frame_bindings_mut(&mut self, index: usize) -> &mut Vec<BindingSnapshot> {
        match self.execution_frames.get_mut(index) {
            Some(ExecutionFrame::ShadowedBindings(bindings)) => bindings,
            Some(ExecutionFrame::ActiveCall(frame)) => &mut frame.replaced_bindings,
            _ => unreachable!("binding frame must exist"),
        }
    }

    // Keep each snapshot in its frame until the corresponding binding is restored.
    fn restore_frame_bindings(&mut self, index: usize) {
        while let Some(snapshot) = self.frame_bindings_mut(index).last().cloned() {
            self.variables.restore(snapshot);
            self.frame_bindings_mut(index).pop();
        }
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

        let result = self.with_exception_scope(operation);

        self.execution_depth -= 1;

        result
    }

    // This retains only errors crossing an active execution or call boundary.
    // It is diagnostic rooting, not permission to run GC at arbitrary points.
    fn with_exception_scope<T>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> Result<T, JsError>,
    ) -> Result<T, JsError> {
        let index = self.execution_frames.len();
        self.execution_frames
            .push(ExecutionFrame::ExceptionScope(None));
        let result = operation(self);

        if let Err(error) = &result {
            self.record_exception(error);
            if let Some(ExecutionFrame::ExceptionScope(parent)) = self.execution_frames[..index]
                .iter_mut()
                .rev()
                .find(|frame| matches!(frame, ExecutionFrame::ExceptionScope(_)))
            {
                *parent = Some(error.clone());
            }
        }

        match self.execution_frames.pop() {
            Some(ExecutionFrame::ExceptionScope(_)) => {}
            _ => unreachable!("exception scope must exist"),
        }
        result
    }

    fn record_exception(&mut self, error: &JsError) {
        if let Some(ExecutionFrame::ExceptionScope(slot)) = self
            .execution_frames
            .iter_mut()
            .rev()
            .find(|frame| matches!(frame, ExecutionFrame::ExceptionScope(_)))
        {
            *slot = Some(error.clone());
        }
    }

    fn clear_exception(&mut self) {
        if let Some(ExecutionFrame::ExceptionScope(slot)) = self
            .execution_frames
            .iter_mut()
            .rev()
            .find(|frame| matches!(frame, ExecutionFrame::ExceptionScope(_)))
        {
            *slot = None;
        }
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

    fn with_call_result_scope(
        &mut self,
        operation: impl FnOnce(&mut Self, usize) -> Result<Value, JsError>,
    ) -> Result<Value, JsError> {
        self.with_temporary_values_scope(|interpreter, index| {
            let result = operation(interpreter, index)?;
            interpreter.retain_call_result(&result);
            Ok(result)
        })
    }

    fn with_frame_value_for_call_result(
        &mut self,
        value: Value,
        operation: impl FnOnce(&mut Self, usize) -> Result<Value, JsError>,
    ) -> Result<Value, JsError> {
        self.with_call_result_scope(|interpreter, index| {
            interpreter.frame_values_mut(index).push(value);
            operation(interpreter, index)
        })
    }

    fn with_pending_call(
        &mut self,
        callee: Value,
        receiver: Option<Value>,
        expressions: &[Expression],
        operation: impl FnOnce(&mut Self, usize) -> Result<Value, JsError>,
    ) -> Result<Value, JsError> {
        self.with_call_frame(
            PendingCall {
                callee,
                receiver,
                arguments: Vec::with_capacity(expressions.len()),
                handoff_value: None,
            },
            |interpreter, index| {
                for expression in expressions {
                    interpreter.evaluate_expression_with_handoff(
                        expression,
                        |interpreter, result| {
                            interpreter.pending_call_mut(index).arguments.push(result?);
                            Ok(())
                        },
                    )?;
                }

                operation(interpreter, index)
            },
        )
    }

    fn with_call_frame(
        &mut self,
        call: PendingCall,
        operation: impl FnOnce(&mut Self, usize) -> Result<Value, JsError>,
    ) -> Result<Value, JsError> {
        self.with_call_frame_handoff(call, operation, |interpreter, result| {
            interpreter.handoff_call_result(result)
        })
    }

    fn with_call_frame_handoff<T>(
        &mut self,
        call: PendingCall,
        operation: impl FnOnce(&mut Self, usize) -> Result<Value, JsError>,
        handoff: impl FnOnce(&mut Self, Result<Value, JsError>) -> Result<T, JsError>,
    ) -> Result<T, JsError> {
        self.with_exception_scope(|interpreter| {
            let index = interpreter.execution_frames.len();
            interpreter
                .execution_frames
                .push(ExecutionFrame::PendingCall(call));
            let result = operation(interpreter, index).map(Completion::Return);

            // Retain the completion until the caller has accepted the result.
            let (_, output) = interpreter.with_pending_completion(result, |interpreter| {
                interpreter.remove_pending_call(index);
                let result = match &interpreter.execution_frames[index] {
                    ExecutionFrame::PendingCompletion(Ok(Completion::Return(value))) => {
                        Ok(value.clone())
                    }
                    ExecutionFrame::PendingCompletion(Err(error)) => Err(error.clone()),
                    _ => unreachable!("call results use return completion"),
                };
                if let Err(error) = &result {
                    interpreter.record_exception(error);
                }
                handoff(interpreter, result)
            })?;
            Ok(output)
        })
    }

    fn remove_pending_call(&mut self, index: usize) {
        match self.execution_frames.get(index..) {
            Some(
                [
                    ExecutionFrame::PendingCall(_),
                    ExecutionFrame::PendingCompletion(_),
                ],
            ) => {}
            _ => unreachable!("pending call must be followed only by its completion"),
        }
        self.execution_frames.remove(index);
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

    // A call can retain a returned value until its caller accepts it.
    fn retain_call_result(&mut self, value: &Value) {
        if let Some(ExecutionFrame::PendingCall(call)) = self
            .execution_frames
            .iter_mut()
            .rev()
            .find(|frame| matches!(frame, ExecutionFrame::PendingCall(_)))
        {
            call.handoff_value = Some(value.clone());
        }
    }

    fn handoff_call_result(&mut self, result: Result<Value, JsError>) -> Result<Value, JsError> {
        if let Ok(value) = &result {
            self.retain_call_result(value);
        }
        result
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

    pub(super) fn export_error(&self, mut error: JsError) -> HostError {
        self.materialize_native_exception(&mut error);
        HostError::from_internal(error, &self.host_roots)
    }

    /// Visits the current shallow roots and returns the retained host snapshot.
    /// The visitor must not run JavaScript or mutate the root set.
    #[allow(dead_code)]
    fn visit_runtime_roots(
        &self,
        visit: &mut impl FnMut(RuntimeRoot<'_>),
    ) -> Result<Vec<std::rc::Rc<super::HostRoot>>, RootVisitError> {
        let host_roots = self.host_roots.snapshot();

        self.variables.visit_roots(visit)?;

        for frame in &self.execution_frames {
            frame.visit_roots(visit)?;
        }

        for root in &host_roots {
            visit(RuntimeRoot::Value(&root.value));
        }

        if let Some(document) = &self.document {
            visit(RuntimeRoot::Document(document));
        }

        for cleanup in &self.cleanups {
            visit(RuntimeRoot::Function(&cleanup.callback));

            for value in cleanup.captured.values() {
                visit(RuntimeRoot::Value(value));
            }
        }

        for object in [
            &self.object_prototype,
            &self.promise_prototype,
            &self.error_prototype,
            &self.type_error_prototype,
            &self.reference_error_prototype,
        ] {
            visit(RuntimeRoot::Object(object));
        }

        visit(RuntimeRoot::Array(&self.array_prototype));

        for array in &self.active_array_joins {
            visit(RuntimeRoot::Array(array));
        }

        self.canvas.visit_roots(visit);

        for job in &self.microtasks {
            job.visit_roots(visit);
        }

        Ok(host_roots)
    }
}

#[cfg(test)]
#[path = "runtime/tests.rs"]
mod tests;
