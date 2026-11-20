//! Diagnostic reachability traversal for the current Rc-backed runtime.
//!
//! This is not a garbage collector and is not connected to the future heap.

use std::{collections::HashSet, rc::Rc};

use super::{
    ArgumentsValue, ArrayStorage, DocumentStorage, FunctionValue, Interpreter, ObjectStorage,
    RegExpValue, RootVisitError, RuntimeRoot, Value, promise::PromiseHandle,
};
use crate::javascript::value::ArrayState;
use crate::javascript::{HostRoots, OrdinaryObjectIdentity};

#[derive(Clone)]
enum TraceNode {
    Value(Value),
    Function(FunctionValue),
    Promise(PromiseHandle),
    Object(ObjectStorage),
    Array(ArrayStorage),
    Document(DocumentStorage),
    RegExp(RegExpValue),
    Arguments(ArgumentsValue),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum NodeIdentity {
    Function(*const ()),
    Promise(*const ()),
    Object(OrdinaryObjectIdentity),
    Array(*const ()),
    Document(*const ()),
    RegExp(*const ()),
    Arguments(*const ()),
}

#[derive(Debug, Default, PartialEq, Eq)]
struct TraceReport {
    functions: usize,
    promises: usize,
    objects: usize,
    arrays: usize,
    documents: usize,
    regexps: usize,
    arguments: usize,
}

#[derive(Debug, PartialEq, Eq)]
enum DiagnosticTraceError {
    InterpreterBusy,
    Root(RootVisitError),
}

impl From<RuntimeRoot<'_>> for TraceNode {
    fn from(root: RuntimeRoot<'_>) -> Self {
        match root {
            RuntimeRoot::Value(value) => Self::Value(value.clone()),
            RuntimeRoot::Function(function) => Self::Function(function.clone()),
            RuntimeRoot::Promise(promise) => Self::Promise(promise.clone()),
            RuntimeRoot::Object(object) => Self::Object(object.clone()),
            RuntimeRoot::Array(array) => Self::Array(array.clone()),
            RuntimeRoot::Document(document) => Self::Document(document.clone()),
            RuntimeRoot::RegExp(regexp) => Self::RegExp(regexp.clone()),
            RuntimeRoot::Arguments(arguments) => Self::Arguments(arguments.clone()),
        }
    }
}

impl TraceNode {
    fn identity(&self, owner: &HostRoots) -> Option<NodeIdentity> {
        match self {
            Self::Value(_) => None,
            Self::Function(value) => Some(NodeIdentity::Function(value.trace_identity())),
            Self::Promise(value) => Some(NodeIdentity::Promise(value.trace_identity())),
            Self::Object(value) => {
                Some(NodeIdentity::Object(owner.ordinary_object_identity(value)))
            }
            Self::Array(value) => Some(NodeIdentity::Array(Rc::as_ptr(value).cast())),
            Self::Document(value) => Some(NodeIdentity::Document(Rc::as_ptr(value).cast())),
            Self::RegExp(value) => Some(NodeIdentity::RegExp(Rc::as_ptr(&value.inner).cast())),
            Self::Arguments(value) => Some(NodeIdentity::Arguments(value.trace_identity())),
        }
    }
}

impl TraceReport {
    fn record(&mut self, identity: NodeIdentity) {
        match identity {
            NodeIdentity::Function(_) => self.functions += 1,
            NodeIdentity::Promise(_) => self.promises += 1,
            NodeIdentity::Object(_) => self.objects += 1,
            NodeIdentity::Array(_) => self.arrays += 1,
            NodeIdentity::Document(_) => self.documents += 1,
            NodeIdentity::RegExp(_) => self.regexps += 1,
            NodeIdentity::Arguments(_) => self.arguments += 1,
        }
    }
}

impl Interpreter {
    /// Runs diagnostic tracing when no interpreter work is active.
    /// This is not a GC safepoint: caller-owned values may still be unregistered.
    fn trace_when_quiescent(&self) -> Result<TraceReport, DiagnosticTraceError> {
        if self.execution_depth != 0
            || !self.execution_frames.is_empty()
            || self.performing_microtask_checkpoint
        {
            return Err(DiagnosticTraceError::InterpreterBusy);
        }

        self.trace_reachable().map_err(DiagnosticTraceError::Root)
    }

    /// Performs a diagnostic traversal without collecting or mutating objects.
    fn trace_reachable(&self) -> Result<TraceReport, RootVisitError> {
        let mut pending = Vec::new();
        let _host_snapshot =
            self.visit_runtime_roots(&mut |root| pending.push(TraceNode::from(root)))?;
        let mut seen = HashSet::new();
        let mut retained = Vec::new();
        let mut report = TraceReport::default();

        while let Some(node) = pending.pop() {
            if let Some(identity) = node.identity(&self.host_roots) {
                if !seen.insert(identity) {
                    continue;
                }
                report.record(identity);
                // Keep all visited allocations alive while their addresses are in `seen`.
                retained.push(node.clone());
            }

            let mut enqueue = |root: RuntimeRoot<'_>| pending.push(TraceNode::from(root));
            match &node {
                TraceNode::Value(value) => value.visit_references(&mut enqueue)?,
                TraceNode::Function(function) => function.visit_references(&mut enqueue)?,
                TraceNode::Promise(promise) => promise.visit_references(&mut enqueue)?,
                TraceNode::Object(object) => self
                    .host_roots
                    .visit_ordinary_references(object, &mut enqueue)?,
                TraceNode::Array(array) => ArrayState::visit_references(array, &mut enqueue)?,
                TraceNode::Document(_) => {}
                TraceNode::RegExp(regexp) => regexp.visit_references(&mut enqueue)?,
                TraceNode::Arguments(arguments) => arguments.visit_references(&mut enqueue),
            }
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::{DiagnosticTraceError, Interpreter, RootVisitError, Value};
    use crate::javascript::{JsErrorKind, RegExpState, RegExpValue, parse_program};
    use std::{cell::RefCell, rc::Rc};

    #[test]
    fn queued_reaction_traces_derived_promise_and_its_properties() {
        let mut interpreter = Interpreter::new();
        let baseline = interpreter.trace_when_quiescent().unwrap();
        let program = parse_program("Promise.resolve(1).then().extra = {};").unwrap();

        drop(interpreter.execute(&program).unwrap());

        let pending = interpreter.trace_reachable().unwrap();
        assert_eq!(pending.objects, baseline.objects + 2);
        assert_eq!(pending.promises, baseline.promises + 1);

        interpreter.perform_microtask_checkpoint().unwrap();
        assert_eq!(interpreter.trace_reachable().unwrap(), baseline);
    }

    #[test]
    fn traverses_cycles_once_and_fails_closed_on_borrowed_state() {
        let mut interpreter = Interpreter::new();
        let baseline = interpreter.trace_when_quiescent().unwrap();
        let Value::Array(array) = Value::array(vec![]) else {
            unreachable!()
        };
        {
            let mut state = array.borrow_mut();
            state.values.push(Value::Array(array.clone()));
            state.present.push(true);
        }
        let regexp = RegExpValue {
            inner: Rc::new(RefCell::new(RegExpState {
                regexp: crate::javascript::regexp::object::RegExp::compile("a", "").unwrap(),
                last_index: Value::Null,
            })),
        };
        regexp.inner.borrow_mut().last_index = Value::RegExp(regexp.clone());
        let Value::Object(object) = Value::object([]) else {
            unreachable!();
        };
        let arguments = Value::arguments(vec![Value::Object(object.clone())], 0);
        object.borrow_mut().set("cycle".into(), arguments.clone());
        interpreter
            .variables
            .insert("array".into(), Value::Array(array.clone()));
        interpreter
            .variables
            .insert("regexp".into(), Value::RegExp(regexp.clone()));
        interpreter.variables.insert("arguments".into(), arguments);
        let first = interpreter.trace_when_quiescent().unwrap();
        assert_eq!(first.arrays, baseline.arrays + 1);
        assert_eq!(first.regexps, baseline.regexps + 1);
        assert_eq!(first.objects, baseline.objects + 1);
        assert_eq!(first.arguments, baseline.arguments + 1);
        interpreter
            .variables
            .insert("array_alias".into(), Value::Array(array.clone()));
        interpreter
            .variables
            .insert("regexp_alias".into(), Value::RegExp(regexp.clone()));
        interpreter
            .variables
            .insert("object_alias".into(), Value::Object(object.clone()));
        assert_eq!(interpreter.trace_when_quiescent().unwrap(), first);
        let host_value = interpreter.export_value(Value::array(vec![]));
        assert_eq!(
            interpreter.trace_when_quiescent().unwrap().arrays,
            first.arrays + 1
        );
        drop(host_value);
        assert_eq!(interpreter.trace_when_quiescent().unwrap(), first);
        {
            let _guard = regexp.inner.borrow_mut();
            assert_eq!(
                interpreter.trace_when_quiescent(),
                Err(DiagnosticTraceError::Root(RootVisitError::RegExpBorrowed))
            );
        }
        {
            let _guard = array.borrow_mut();
            assert_eq!(
                interpreter.trace_when_quiescent(),
                Err(DiagnosticTraceError::Root(RootVisitError::ArrayBorrowed))
            );
        }
        {
            let _guard = object.borrow_mut();
            assert_eq!(
                interpreter.trace_when_quiescent(),
                Err(DiagnosticTraceError::Root(RootVisitError::ObjectBorrowed))
            );
        }
        object.borrow_mut().set("cycle".into(), Value::Undefined);
        {
            let mut state = array.borrow_mut();
            state.values.clear();
            state.present.clear();
        }
        regexp.inner.borrow_mut().last_index = Value::Null;
    }

    #[test]
    fn diagnostic_trace_requires_quiescence_but_unexported_results_are_not_roots() {
        let mut interpreter = Interpreter::new();
        let baseline = interpreter.trace_when_quiescent().unwrap();

        interpreter
            .with_execution_frame(|interpreter| {
                assert_eq!(
                    interpreter.trace_when_quiescent(),
                    Err(DiagnosticTraceError::InterpreterBusy)
                );
                Ok(())
            })
            .unwrap();

        interpreter
            .with_temporary_values_scope(|interpreter, _| {
                assert_eq!(
                    interpreter.trace_when_quiescent(),
                    Err(DiagnosticTraceError::InterpreterBusy)
                );
                Ok(())
            })
            .unwrap();

        interpreter.performing_microtask_checkpoint = true;

        assert_eq!(
            interpreter.trace_when_quiescent(),
            Err(DiagnosticTraceError::InterpreterBusy)
        );

        interpreter.performing_microtask_checkpoint = false;

        let program = crate::javascript::parse_program("[];").unwrap();
        let local_result = interpreter.execute_script(&program).unwrap().into_value();

        // The local Rust value is not registered as a runtime or host root.
        assert_eq!(interpreter.trace_when_quiescent().unwrap(), baseline);

        let host_result = interpreter.export_value(local_result);

        assert_eq!(
            interpreter.trace_when_quiescent().unwrap().arrays,
            baseline.arrays + 1,
        );

        drop(host_result);

        assert_eq!(interpreter.trace_when_quiescent().unwrap(), baseline);
    }

    #[test]
    fn host_script_exports_results_and_errors_before_returning() {
        let mut interpreter = Interpreter::new();
        let baseline = interpreter.trace_when_quiescent().unwrap();

        let result = interpreter
            .execute_host_script(&parse_program("[];").unwrap())
            .unwrap();

        assert_eq!(
            interpreter.trace_when_quiescent().unwrap().arrays,
            baseline.arrays + 1,
        );

        drop(result);

        assert_eq!(interpreter.trace_when_quiescent().unwrap(), baseline);

        let error = interpreter
            .execute_host_script(&parse_program("throw [];").unwrap())
            .unwrap_err();

        assert!(matches!(&error.kind, JsErrorKind::Thrown(_)));

        assert_eq!(
            interpreter.trace_when_quiescent().unwrap().arrays,
            baseline.arrays + 1,
        );

        drop(error);

        assert_eq!(interpreter.trace_when_quiescent().unwrap(), baseline);

        let error = interpreter
            .execute_host_script(&parse_program("missing;").unwrap())
            .unwrap_err();

        assert!(matches!(&error.kind, JsErrorKind::NativeException(_)));

        assert_eq!(
            interpreter.trace_when_quiescent().unwrap().objects,
            baseline.objects + 1
        );

        drop(error);

        assert_eq!(interpreter.trace_when_quiescent().unwrap(), baseline);
    }

    #[test]
    fn host_array_items_remain_rooted_after_parent_is_changed_and_dropped() {
        let interpreter = Interpreter::new();
        let baseline = interpreter.trace_when_quiescent().unwrap();

        let Value::Array(child) = Value::array(vec![Value::Number(7.0)]) else {
            unreachable!();
        };

        let Value::Array(parent) = Value::array(vec![Value::Array(child.clone())]) else {
            unreachable!();
        };

        let rooted_parent = interpreter.export_value(Value::Array(parent.clone()));
        let children = rooted_parent.array_items().unwrap().unwrap();

        {
            let mut state = parent.borrow_mut();
            state.values.clear();
            state.present.clear();
        }

        drop(rooted_parent);

        assert_eq!(
            interpreter.trace_when_quiescent().unwrap().arrays,
            baseline.arrays + 1
        );

        let nested = children[0].array_items().unwrap().unwrap();
        assert_eq!(nested[0].as_number(), Some(7.0));

        {
            let _guard = child.borrow_mut();
            assert!(children[0].array_items().is_err());
        }

        drop(nested);
        drop(children);

        assert_eq!(interpreter.trace_when_quiescent().unwrap(), baseline);
    }
}
