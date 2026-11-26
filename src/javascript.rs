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

pub use session::DocumentScriptSession;

/// Classification of parser and interpreter failures.
///
/// Explicitly thrown JavaScript values are distinct from engine failures.
/// Other runtime failures are still unclassified.
#[derive(Clone, Debug, PartialEq)]
pub enum JsErrorKind<T, N = T> {
    Syntax,
    UnclassifiedRuntime,
    UnsupportedFeature,
    Thrown(T),
    NativeException(N),
    ResourceLimit,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct JsError {
    pub offset: Option<usize>,
    pub message: String,
    pub kind: JsErrorKind<Value, NativeException>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum NativeErrorKind {
    TypeError,
    ReferenceError,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum NativeException {
    Pending {
        kind: NativeErrorKind,
        message: String,
    },
    Materialized(Value),
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

    fn native_exception(kind: NativeErrorKind, message: impl Into<String>) -> Self {
        let message = message.into();
        let name = match kind {
            NativeErrorKind::TypeError => "TypeError",
            NativeErrorKind::ReferenceError => "ReferenceError",
        };

        Self {
            offset: None,
            message: format!("{name}: {message}"),
            kind: JsErrorKind::NativeException(NativeException::Pending { kind, message }),
        }
    }

    fn reference_error(name: &str) -> Self {
        Self::native_exception(
            NativeErrorKind::ReferenceError,
            format!("{name} is not defined"),
        )
    }

    fn type_error(message: impl Into<String>) -> Self {
        Self::native_exception(NativeErrorKind::TypeError, message)
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

                JsErrorKind::NativeException(NativeException::Materialized(value)) => {
                    JsErrorKind::NativeException(roots.register(value))
                }

                JsErrorKind::NativeException(NativeException::Pending { .. }) => {
                    unreachable!("native exception must be materialized before export")
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
    owner: Rc<HostRoots>,
}

// Valid only while the corresponding Rc-backed object remains alive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct OrdinaryObjectIdentity(*const ());

impl HostRoots {
    // This allocation boundary still uses Rc-backed storage until heap migration.
    fn create_ordinary_object(
        &self,
        entries: impl IntoIterator<Item = (String, Value)>,
        prototype: value::ObjectPrototype,
    ) -> Value {
        Value::object_with_entries_and_prototype(entries, prototype)
    }

    // These operations still use Rc-backed objects until heap migration.
    fn lookup_ordinary_property(
        &self,
        object: &value::ObjectStorage,
        name: &str,
    ) -> value::PropertyLookup {
        let mut current = object.clone();
        loop {
            let next = {
                let state = current.borrow();
                if let Some(value) = state.get(name) {
                    return value::PropertyLookup::Found(value.clone());
                }
                state.prototype()
            };
            match next {
                value::ObjectPrototype::Null => return value::PropertyLookup::Absent,
                value::ObjectPrototype::Unknown => return value::PropertyLookup::Unknown,
                value::ObjectPrototype::IntrinsicObjectRoot => {
                    return if value::is_object_prototype_intrinsic(name) {
                        value::PropertyLookup::Unknown
                    } else {
                        value::PropertyLookup::Absent
                    };
                }
                value::ObjectPrototype::Object(prototype) => current = prototype,
            }
        }
    }

    fn read_ordinary_own_property(
        &self,
        object: &value::ObjectStorage,
        name: &str,
    ) -> Option<Value> {
        object.borrow().get(name).cloned()
    }

    fn write_ordinary_property(&self, object: &value::ObjectStorage, name: String, value: Value) {
        object.borrow_mut().set(name, value);
    }

    fn ordinary_prototype(&self, object: &value::ObjectStorage) -> value::ObjectPrototype {
        object.borrow().prototype()
    }

    fn define_ordinary_property(
        &self,
        object: &value::ObjectStorage,
        name: String,
        value: Value,
        enumerable: bool,
    ) {
        object
            .borrow_mut()
            .define_own_data_property(name, value, enumerable);
    }

    fn same_ordinary_object(
        &self,
        left: &value::ObjectStorage,
        right: &value::ObjectStorage,
    ) -> bool {
        Rc::ptr_eq(left, right)
    }

    #[allow(dead_code)]
    fn ordinary_object_identity(&self, object: &value::ObjectStorage) -> OrdinaryObjectIdentity {
        OrdinaryObjectIdentity(Rc::as_ptr(object).cast())
    }

    /// Visits outgoing references while the current Rc-backed storage is borrowed.
    /// The visitor must not mutate the graph or re-enter JavaScript.
    #[allow(dead_code)]
    fn visit_ordinary_references(
        &self,
        object: &value::ObjectStorage,
        visit: &mut impl FnMut(runtime::RuntimeRoot<'_>),
    ) -> Result<(), runtime::RootVisitError> {
        let state = object
            .try_borrow()
            .map_err(|_| runtime::RootVisitError::ObjectBorrowed)?;
        state.visit_contents(visit);
        Ok(())
    }

    fn set_ordinary_promise(
        &self,
        object: &value::ObjectStorage,
        promise: runtime::promise::PromiseHandle,
    ) {
        object.borrow_mut().promise = Some(promise);
    }

    fn ordinary_promise(
        &self,
        object: &value::ObjectStorage,
    ) -> Option<runtime::promise::PromiseHandle> {
        object.borrow().promise.clone()
    }

    fn ordinary_own_keys(&self, object: &value::ObjectStorage) -> Vec<String> {
        object.borrow().keys()
    }

    fn ordinary_for_in_keys(&self, object: &value::ObjectStorage) -> Option<Vec<String>> {
        let mut current = object.clone();
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();
        loop {
            let (keys, prototype) = {
                let state = current.borrow();
                let prototype = state.prototype();
                // The legacy __proto__ setter is not implemented yet.
                if matches!(prototype, value::ObjectPrototype::IntrinsicObjectRoot)
                    && state.get("__proto__").is_some()
                {
                    return None;
                }

                (state.own_keys_with_enumerability(), prototype)
            };
            for (key, enumerable) in keys {
                if seen.insert(key.clone()) && enumerable {
                    result.push(key);
                }
            }
            match prototype {
                value::ObjectPrototype::Null | value::ObjectPrototype::IntrinsicObjectRoot => {
                    return Some(result);
                }
                value::ObjectPrototype::Unknown => return None,
                value::ObjectPrototype::Object(prototype) => {
                    current = prototype;
                }
            }
        }
    }

    fn ordinary_has_prototype(
        &self,
        object: &value::ObjectStorage,
        prototype: &value::ObjectStorage,
    ) -> Option<bool> {
        let mut current = object.clone();
        loop {
            match self.ordinary_prototype(&current) {
                value::ObjectPrototype::Object(next) => {
                    if self.same_ordinary_object(&next, prototype) {
                        return Some(true);
                    }
                    current = next;
                }
                value::ObjectPrototype::Null | value::ObjectPrototype::IntrinsicObjectRoot => {
                    return Some(false);
                }
                value::ObjectPrototype::Unknown => return None,
            }
        }
    }

    fn register(self: &Rc<Self>, value: Value) -> HostValue {
        let root = Rc::new(HostRoot {
            value,
            owner: self.clone(),
        });

        let mut roots = self.roots.borrow_mut();

        if roots.len() >= 64 && roots.len() % 64 == 0 {
            roots.retain(|root| root.strong_count() > 0);
        }

        roots.push(Rc::downgrade(&root));

        HostValue(root)
    }

    // These snapshots read stored data only; they do not invoke JavaScript.
    fn root_array_slots(
        self: &Rc<Self>,
        array: &ArrayStorage,
    ) -> Result<Vec<HostValue>, std::cell::BorrowError> {
        let array = array.try_borrow()?;
        Ok(array
            .values
            .iter()
            .cloned()
            .map(|value| self.register(value))
            .collect())
    }

    fn root_ordinary_own_property(
        self: &Rc<Self>,
        object: &value::ObjectStorage,
        name: &str,
    ) -> Result<Option<HostValue>, std::cell::BorrowError> {
        let object = object.try_borrow()?;
        Ok(object.get(name).cloned().map(|value| self.register(value)))
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

        self.0.owner.root_array_slots(array).map(Some)
    }

    /// Snapshots an own data property as an independently rooted value.
    /// Returns None for a non-ordinary object or a missing own property.
    /// This does not perform prototype lookup or invoke JavaScript.
    fn object_own_property(&self, name: &str) -> Result<Option<HostValue>, std::cell::BorrowError> {
        let Value::Object(object) = &self.0.value else {
            return Ok(None);
        };
        self.0.owner.root_ordinary_own_property(object, name)
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

#[cfg(test)]
#[path = "javascript/testharness_fixture_tests.rs"]
mod testharness_fixture_tests;
