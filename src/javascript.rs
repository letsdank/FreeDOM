use std::{
    cell::{OnceCell, RefCell},
    error::Error,
    fmt,
    rc::{Rc, Weak},
};

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
pub(crate) use json::{JsonValue, parse_json};

use parser::parse_program;

#[path = "javascript/dom.rs"]
mod dom;

pub use dom::DocumentState;

#[path = "javascript/runtime.rs"]
mod runtime;

// The arena will be connected when object storage is migrated.
#[allow(dead_code)]
#[path = "javascript/heap.rs"]
mod heap;

#[path = "javascript/object_properties.rs"]
mod object_properties;

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

use runtime::Interpreter;

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
pub enum JsErrorKind<T> {
    Syntax,
    UnclassifiedRuntime,
    UnsupportedFeature,
    Thrown(T),
    NativeException(T),
    ResourceLimit,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct JsError {
    pub offset: Option<usize>,
    pub message: String,
    pub kind: JsErrorKind<Value>,
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
#[derive(Clone, Debug)]
pub struct HostError {
    pub offset: Option<usize>,
    pub message: String,
    pub kind: JsErrorKind<HostValue>,
}

impl HostError {
    fn from_internal(error: JsError, roots: &Rc<HostRoots>) -> Self {
        Self {
            offset: error.offset,
            message: error.message,
            kind: match error.kind {
                JsErrorKind::Syntax => JsErrorKind::Syntax,

                JsErrorKind::UnclassifiedRuntime => JsErrorKind::UnclassifiedRuntime,

                JsErrorKind::UnsupportedFeature => JsErrorKind::UnsupportedFeature,

                JsErrorKind::Thrown(value) => JsErrorKind::Thrown(roots.register(value)),

                JsErrorKind::NativeException(value) => {
                    JsErrorKind::NativeException(roots.register(value))
                }

                JsErrorKind::ResourceLimit => JsErrorKind::ResourceLimit,
            },
        }
    }
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        format_error(f, &self.message, self.offset)
    }
}

impl Error for HostError {}

#[derive(Default, Debug)]
struct HostRoots {
    roots: RefCell<Vec<Weak<HostRoot>>>,
    heap: OnceCell<Result<RefCell<heap::Heap<heap::HeapObject>>, heap::HeapError>>,
}

#[derive(Debug)]
struct HostRoot {
    value: Value,
    _owner: Rc<HostRoots>,
}

impl HostRoots {
    // This allocation boundary still uses Rc-backed storage until heap migration.
    fn create_ordinary_object(
        &self,
        entries: impl IntoIterator<Item = (String, Value)>,
        prototype: value::ObjectPrototype,
    ) -> Value {
        Value::object_with_entries_and_prototype(entries, prototype)
    }

    fn register(self: &Rc<Self>, value: Value) -> HostValue {
        let root = Rc::new(HostRoot {
            value,
            _owner: self.clone(),
        });

        let mut roots = self.roots.borrow_mut();

        if roots.len() >= 64 && roots.len() % 64 == 0 {
            roots.retain(|root| root.strong_count() > 0);
        }

        roots.push(Rc::downgrade(&root));

        HostValue(root)
    }

    #[allow(dead_code)]
    fn heap_cell(&self) -> Result<&RefCell<heap::Heap<heap::HeapObject>>, heap::HeapAccessError> {
        self.heap
            .get_or_init(|| heap::Heap::new().map(RefCell::new))
            .as_ref()
            .map_err(|error| heap::HeapAccessError::Initialization(*error))
    }

    /// Experimental heap operations do not run JavaScript or collect garbage.
    #[allow(dead_code)]
    fn allocate_heap_object(
        &self,
        object: heap::HeapObject,
    ) -> Result<heap::ObjectId, heap::HeapAccessError> {
        let mut heap = self
            .heap_cell()?
            .try_borrow_mut()
            .map_err(|_| heap::HeapAccessError::Borrowed)?;
        let mut invalid = None;
        object.visit_references(|reference| {
            if invalid.is_none() && heap.get(reference).is_none() {
                invalid = Some(reference);
            }
        });
        if let Some(reference) = invalid {
            return Err(heap::HeapAccessError::InvalidReference(reference));
        }
        Ok(heap.allocate(object))
    }

    #[allow(dead_code)]
    fn contains_heap_object(&self, id: heap::ObjectId) -> Result<bool, heap::HeapAccessError> {
        let heap = self
            .heap_cell()?
            .try_borrow()
            .map_err(|_| heap::HeapAccessError::Borrowed)?;
        Ok(heap.get(id).is_some())
    }

    /// Reads an ordinary object's own property; no prototype lookup is performed.
    #[allow(dead_code)]
    fn read_heap_property(
        &self,
        id: heap::ObjectId,
        name: &str,
    ) -> Result<Option<heap::HeapValue>, heap::HeapAccessError> {
        let heap = self
            .heap_cell()?
            .try_borrow()
            .map_err(|_| heap::HeapAccessError::Borrowed)?;
        let object = heap
            .get(id)
            .ok_or(heap::HeapAccessError::InvalidObject(id))?;
        let heap::HeapObject::Object(object) = object else {
            return Err(heap::HeapAccessError::NotOrdinaryObject(id));
        };
        let value = object.properties.get(name).cloned();
        if let Some(heap::HeapValue::Reference(reference)) = &value {
            if heap.get(*reference).is_none() {
                return Err(heap::HeapAccessError::InvalidReference(*reference));
            }
        }
        Ok(value)
    }

    /// Writes an ordinary object's own data property without invoking JavaScript.
    #[allow(dead_code)]
    fn write_heap_property(
        &self,
        id: heap::ObjectId,
        name: String,
        value: heap::HeapValue,
    ) -> Result<(), heap::HeapAccessError> {
        let mut heap = self
            .heap_cell()?
            .try_borrow_mut()
            .map_err(|_| heap::HeapAccessError::Borrowed)?;
        match heap.get(id) {
            None => return Err(heap::HeapAccessError::InvalidObject(id)),
            Some(heap::HeapObject::Array(_)) => {
                return Err(heap::HeapAccessError::NotOrdinaryObject(id));
            }
            Some(heap::HeapObject::Object(_)) => {}
        }
        if let heap::HeapValue::Reference(reference) = &value {
            if heap.get(*reference).is_none() {
                return Err(heap::HeapAccessError::InvalidReference(*reference));
            }
        }
        let Some(heap::HeapObject::Object(object)) = heap.get_mut(id) else {
            unreachable!("validated ordinary object must still exist");
        };
        object.properties.set(name, value, true);
        Ok(())
    }

    /// Takes strong references to roots live at the time of the snapshot.
    /// A future collector must prevent root mutations while using it.
    #[allow(dead_code)]
    fn snapshot(&self) -> Vec<Rc<HostRoot>> {
        let mut live = Vec::new();

        self.roots.borrow_mut().retain(|root| match root.upgrade() {
            Some(root) => {
                live.push(root);
                true
            }

            None => false,
        });

        live
    }
}

/// A value returned to the embedding application.
///
/// Object values still use reference-counted storage; this is not yet a GC root.
#[derive(Clone, Debug)]
pub struct HostValue(Rc<HostRoot>);

impl HostValue {
    pub fn as_boolean(&self) -> Option<bool> {
        match &self.0.value {
            Value::Boolean(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<f64> {
        match &self.0.value {
            Value::Number(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<&str> {
        match &self.0.value {
            Value::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self.0.value, Value::Null)
    }

    pub fn is_undefined(&self) -> bool {
        matches!(self.0.value, Value::Undefined)
    }

    #[cfg(test)]
    pub(crate) fn into_test_value(self) -> Value {
        self.0.value.clone()
    }

    #[cfg(test)]
    pub(crate) fn as_test_value(&self) -> &Value {
        &self.0.value
    }

    fn is_user_function(&self) -> bool {
        matches!(self.0.value, Value::Function(_))
    }

    /// Snapshots the current stored array slots as independently rooted values.
    /// This does not implement JavaScript indexed property access.
    fn array_items(&self) -> Result<Option<Vec<HostValue>>, std::cell::BorrowError> {
        let Value::Array(array) = &self.0.value else {
            return Ok(None);
        };

        let array = array.try_borrow()?;

        let values = array
            .values
            .iter()
            .cloned()
            .map(|value| self.0._owner.register(value))
            .collect();

        Ok(Some(values))
    }
}

#[cfg(test)]
impl PartialEq for HostValue {
    fn eq(&self, other: &Self) -> bool {
        self.0.value == other.0.value
    }
}

#[cfg(test)]
impl PartialEq<Value> for HostValue {
    fn eq(&self, other: &Value) -> bool {
        self.0.value == *other
    }
}

fn format_error(f: &mut fmt::Formatter<'_>, message: &str, offset: Option<usize>) -> fmt::Result {
    match offset {
        Some(offset) => write!(f, "{message} at byte {offset}"),
        None => f.write_str(message),
    }
}

pub fn evaluate(source: &str) -> Result<HostValue, HostError> {
    let mut interpreter = Interpreter::new();

    let program = parse_program(source).map_err(|error| interpreter.export_error(error))?;

    interpreter.execute_host_script(&program)
}

pub fn evaluate_with_document(
    source: &str,
    document: &mut DocumentState,
) -> Result<HostValue, HostError> {
    DocumentScriptSession::new(document).evaluate(source, document)
}

#[cfg(test)]
#[path = "javascript/tests.rs"]
mod tests;
