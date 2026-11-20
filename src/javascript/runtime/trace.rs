//! Diagnostic reachability traversal for the current Rc-backed runtime.
//!
//! This is not a garbage collector and is not connected to the future heap.

use std::{collections::HashSet, rc::Rc};

use super::{
    ArgumentsValue, ArrayStorage, DocumentStorage, FunctionValue, Interpreter, ObjectStorage,
    RegExpValue, RootVisitError, RuntimeRoot, Value, promise::PromiseHandle,
};
use crate::javascript::value::{ArrayState, ObjectState};

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
    Object(*const ()),
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
    fn identity(&self) -> Option<NodeIdentity> {
        match self {
            Self::Value(_) => None,
            Self::Function(value) => Some(NodeIdentity::Function(value.trace_identity())),
            Self::Promise(value) => Some(NodeIdentity::Promise(value.trace_identity())),
            Self::Object(value) => Some(NodeIdentity::Object(Rc::as_ptr(value).cast())),
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
    /// Performs a diagnostic traversal without collecting or mutating objects.
    fn trace_reachable(&self) -> Result<TraceReport, RootVisitError> {
        let mut pending = Vec::new();
        let _host_snapshot =
            self.visit_runtime_roots(&mut |root| pending.push(TraceNode::from(root)))?;
        let mut seen = HashSet::new();
        let mut retained = Vec::new();
        let mut report = TraceReport::default();

        while let Some(node) = pending.pop() {
            if let Some(identity) = node.identity() {
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
                TraceNode::Object(object) => ObjectState::visit_references(object, &mut enqueue)?,
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
    use super::{Interpreter, RootVisitError, Value};
    use crate::javascript::{RegExpState, RegExpValue};
    use std::{cell::RefCell, rc::Rc};

    #[test]
    fn traverses_cycles_once_and_fails_closed_on_borrowed_state() {
        let mut interpreter = Interpreter::new();
        let baseline = interpreter.trace_reachable().unwrap();
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
        let first = interpreter.trace_reachable().unwrap();
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
        assert_eq!(interpreter.trace_reachable().unwrap(), first);
        let host_value = interpreter.export_value(Value::array(vec![]));
        assert_eq!(
            interpreter.trace_reachable().unwrap().arrays,
            first.arrays + 1
        );
        drop(host_value);
        assert_eq!(interpreter.trace_reachable().unwrap(), first);
        {
            let _guard = regexp.inner.borrow_mut();
            assert_eq!(
                interpreter.trace_reachable(),
                Err(RootVisitError::RegExpBorrowed)
            );
        }
        {
            let _guard = array.borrow_mut();
            assert_eq!(
                interpreter.trace_reachable(),
                Err(RootVisitError::ArrayBorrowed)
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
}
