//! Runtime values and partial object storage for the JavaScript interpreter.
//!
//! <https://tc39.es/ecma262/#sec-ecmascript-language-types>
//!
//! Object, string, and function semantics are not yet fully implemented.

use crate::url::Url;
use std::{cell::RefCell, collections::HashMap, fmt, rc::Rc};

use super::arguments::ArgumentsValue;
use super::runtime::bindings::{CapturedBindings, visit_captured_bindings};
use super::runtime::promise::PromiseHandle;
use super::runtime::{RootVisitError, RuntimeRoot};
use super::{DocumentState, FunctionKind, UserFunction, regexp};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeFunction {
    AssertEquals,
    Test,
    AssertTrue,
    AssertFalse,
    AssertNotEquals,
    ObjectKeys,
    ObjectCreate,
    ObjectEntries,
    ObjectHasOwnProperty,
    FunctionCall,
    FunctionApply,
    ObjectToString,
    ObjectValueOf,
    ArrayToString,
    FunctionBind,
    QueueMicrotask,
    PromiseConstructor,
    PromiseResolve,
    PromiseReject,
    PromiseThen,
    StringSplit,
    StringReplace,
    StringConstructor,
    StringFromCharCode,
    ArrayConstructor,
    ArraySlice,
    ArrayMap,
    ArraySome,
    ErrorConstructor,
    ErrorToString,
    TypeErrorConstructor,
    ReferenceErrorConstructor,
    HtmlCollectionItem,
    HtmlCollectionNamedItem,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DocumentMethod {
    GetElementById,
    CreateElement,
    GetElementsByTagName,
    QuerySelector,
    QuerySelectorAll,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ElementMethod {
    GetAttribute(u64),
    SetAttribute(u64),
    AppendChild(u64),
    RemoveChild(u64),
    RemoveAttribute(u64),
    InsertBefore(u64),
    QuerySelector(u64),
    QuerySelectorAll(u64),
    ClassListContains(u64),
    ClassListAdd(u64),
    ClassListRemove(u64),
    ClassListToggle(u64),
    GetContext(u64),
    CanvasFillRect(u64),
    CanvasClearRect(u64),
    CanvasBeginPath(u64),
    CanvasMoveTo(u64),
    CanvasLineTo(u64),
    CanvasClosePath(u64),
    CanvasFill(u64),
    CanvasQuadraticCurveTo(u64),
    CanvasBezierCurveTo(u64),
    CanvasStroke(u64),
    CanvasSetLineDash(u64),
    CanvasGetLineDash(u64),
    CanvasSave(u64),
    CanvasRestore(u64),
    CanvasTranslate(u64),
    CanvasRotate(u64),
    CanvasScale(u64),
    CanvasTransform(u64),
    CanvasResetTransform(u64),
    CanvasSetTransform(u64),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TestContextMethod {
    AddCleanup,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ObjectState {
    properties: Vec<ObjectProperty>,
    prototype: ObjectPrototype,
    pub(super) promise: Option<PromiseHandle>,
}

#[derive(Clone, Debug, PartialEq)]
struct ObjectProperty {
    name: String,
    value: Value,
    enumerable: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
enum ObjectPrototype {
    #[default]
    Unknown,
    IntrinsicObjectRoot,
    Null,
    Object(ObjectStorage),
}

pub(crate) enum PropertyLookup {
    Found(Value),
    Absent,
    Unknown,
}

// These names exist on the initial Object.prototype, but some values and
// descriptors are not implemented. Other missing string keys are absent.
// https://tc39.es/ecma262/#sec-properties-of-the-object-prototype-object
fn is_object_prototype_intrinsic(name: &str) -> bool {
    matches!(
        name,
        "constructor"
            | "hasOwnProperty"
            | "isPrototypeOf"
            | "propertyIsEnumerable"
            | "toLocaleString"
            | "toString"
            | "valueOf"
            | "__proto__"
            | "__defineGetter__"
            | "__defineSetter__"
            | "__lookupGetter__"
            | "__lookupSetter__"
    )
}

impl ObjectState {
    fn from_entries(entries: impl IntoIterator<Item = (String, Value)>) -> Self {
        let mut state = Self::default();

        for (name, value) in entries {
            state.set(name, value);
        }

        state
    }

    pub(crate) fn get(&self, name: &str) -> Option<&Value> {
        self.properties
            .iter()
            .find(|property| property.name == name)
            .map(|property| &property.value)
    }

    pub(crate) fn set(&mut self, name: String, value: Value) {
        if let Some(property) = self
            .properties
            .iter_mut()
            .find(|property| property.name == name)
        {
            property.value = value;
        } else {
            let enumerable = !(matches!(self.prototype, ObjectPrototype::IntrinsicObjectRoot)
                && is_object_prototype_intrinsic(&name));

            self.properties.push(ObjectProperty {
                name,
                value,
                enumerable,
            });
        }
    }

    pub(super) fn define_own_data_property(
        &mut self,
        name: String,
        value: Value,
        enumerable: bool,
    ) {
        if let Some(property) = self
            .properties
            .iter_mut()
            .find(|property| property.name == name)
        {
            property.value = value;
            property.enumerable = enumerable;
        } else {
            self.properties.push(ObjectProperty {
                name,
                value,
                enumerable,
            });
        }
    }

    pub(crate) fn keys(&self) -> Vec<String> {
        self.own_keys_with_enumerability()
            .into_iter()
            .filter_map(|(name, enumerable)| enumerable.then_some(name))
            .collect()
    }

    fn own_keys_with_enumerability(&self) -> Vec<(String, bool)> {
        let mut indices = Vec::new();
        let mut strings = Vec::new();

        for property in &self.properties {
            let key = &property.name;

            match key.parse::<u32>() {
                Ok(index) if index != u32::MAX && index.to_string() == *key => {
                    indices.push((index, key.clone(), property.enumerable));
                }

                _ => {
                    strings.push((key.clone(), property.enumerable));
                }
            }
        }

        indices.sort_unstable_by_key(|(index, _, _)| *index);

        indices
            .into_iter()
            .map(|(_, key, enumerable)| (key, enumerable))
            .chain(strings)
            .collect()
    }

    fn as_hash_map(&self) -> HashMap<String, Value> {
        self.properties
            .iter()
            .map(|property| (property.name.clone(), property.value.clone()))
            .collect()
    }

    pub(crate) fn lookup(object: &ObjectStorage, name: &str) -> PropertyLookup {
        let mut current = object.clone();

        loop {
            let next = {
                let state = current.borrow();

                if let Some(value) = state.get(name) {
                    return PropertyLookup::Found(value.clone());
                }

                state.prototype.clone()
            };

            match next {
                ObjectPrototype::Null => return PropertyLookup::Absent,

                ObjectPrototype::Unknown => return PropertyLookup::Unknown,

                ObjectPrototype::IntrinsicObjectRoot => {
                    return if is_object_prototype_intrinsic(name) {
                        PropertyLookup::Unknown
                    } else {
                        PropertyLookup::Absent
                    };
                }

                ObjectPrototype::Object(prototype) => current = prototype,
            }
        }
    }

    pub(crate) fn has_prototype(object: &ObjectStorage, target: &ObjectStorage) -> Option<bool> {
        let mut current = object.clone();

        loop {
            let prototype = current.borrow().prototype.clone();

            match prototype {
                ObjectPrototype::Object(next) => {
                    if Rc::ptr_eq(&next, target) {
                        return Some(true);
                    }

                    current = next;
                }

                ObjectPrototype::Null | ObjectPrototype::IntrinsicObjectRoot => {
                    return Some(false);
                }

                ObjectPrototype::Unknown => return None,
            }
        }
    }

    pub(crate) fn for_in_keys(object: &ObjectStorage) -> Option<Vec<String>> {
        let mut current = object.clone();
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();

        loop {
            let (keys, prototype) = {
                let state = current.borrow();

                // The legacy __proto__ setter is not implemented yet.
                if matches!(state.prototype, ObjectPrototype::IntrinsicObjectRoot)
                    && state.get("__proto__").is_some()
                {
                    return None;
                }

                (state.own_keys_with_enumerability(), state.prototype.clone())
            };

            for (key, enumerable) in keys {
                if seen.insert(key.clone()) && enumerable {
                    result.push(key);
                }
            }

            match prototype {
                ObjectPrototype::Null => return Some(result),

                ObjectPrototype::Unknown => return None,

                // The remaining built-in Object.prototype properties are
                // non-enumerable and the intrinsic root has no prototype.
                ObjectPrototype::IntrinsicObjectRoot => return Some(result),

                ObjectPrototype::Object(prototype) => {
                    current = prototype;
                }
            }
        }
    }

    /// Visits immediate references without traversing their children.
    /// The callback must not mutate the visited graph or re-enter JavaScript.
    #[allow(dead_code)]
    pub(super) fn visit_references(
        object: &ObjectStorage,
        visit: &mut impl FnMut(RuntimeRoot<'_>),
    ) -> Result<(), RootVisitError> {
        let state = object
            .try_borrow()
            .map_err(|_| RootVisitError::ObjectBorrowed)?;

        state.visit_contents(visit);
        Ok(())
    }

    #[allow(dead_code)]
    fn visit_contents(&self, visit: &mut impl FnMut(RuntimeRoot<'_>)) {
        for property in &self.properties {
            visit(RuntimeRoot::Value(&property.value));
        }

        match &self.prototype {
            ObjectPrototype::Object(prototype) => {
                visit(RuntimeRoot::Object(prototype));
            }
            ObjectPrototype::Unknown
            | ObjectPrototype::IntrinsicObjectRoot
            | ObjectPrototype::Null => {}
        }

        if let Some(promise) = &self.promise {
            visit(RuntimeRoot::Promise(promise));
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ArrayState {
    pub(super) values: Vec<Value>,
    pub(super) present: Vec<bool>,
    pub(super) properties: ObjectState,
}

pub(crate) type ArrayStorage = Rc<RefCell<ArrayState>>;

impl ArrayState {
    /// Visits immediate references without traversing their children.
    /// The callback must not mutate the visited graph or re-enter JavaScript.
    #[allow(dead_code)]
    pub(super) fn visit_references(
        array: &ArrayStorage,
        visit: &mut impl FnMut(RuntimeRoot<'_>),
    ) -> Result<(), RootVisitError> {
        let state = array
            .try_borrow()
            .map_err(|_| RootVisitError::ArrayBorrowed)?;

        // Visit all stored values, including slots currently marked as holes.
        for value in &state.values {
            visit(RuntimeRoot::Value(value));
        }

        state.properties.visit_contents(visit);
        Ok(())
    }
}

pub(crate) type ObjectStorage = Rc<RefCell<ObjectState>>;
pub type DocumentStorage = Rc<RefCell<DocumentState>>;

/// A partial native Error object with optional prototype support.
#[derive(Clone, Debug)]
pub struct NativeErrorValue {
    default_name: String,
    properties: ObjectStorage,
}

impl NativeErrorValue {
    pub(super) fn lookup_method(&self, name: &str) -> PropertyLookup {
        if let Some(value) = self.properties.borrow().get(name) {
            return PropertyLookup::Found(value.clone());
        }

        match self.prototype() {
            Some(prototype) => ObjectState::lookup(&prototype, name),
            None => PropertyLookup::Unknown,
        }
    }

    fn new(name: &str, message: &str) -> Self {
        let mut properties = ObjectState::default();

        properties.define_own_data_property(
            "message".to_owned(),
            Value::String(message.to_owned()),
            false,
        );

        Self {
            default_name: name.to_owned(),
            properties: Rc::new(RefCell::new(properties)),
        }
    }

    fn new_with_prototype(message: Option<String>, prototype: ObjectStorage) -> Self {
        let mut properties = ObjectState::default();

        properties.prototype = ObjectPrototype::Object(prototype);

        if let Some(message) = message {
            properties.define_own_data_property(
                "message".to_owned(),
                Value::String(message),
                false,
            );
        }

        Self {
            default_name: "Error".to_owned(),
            properties: Rc::new(RefCell::new(properties)),
        }
    }

    fn prototype(&self) -> Option<ObjectStorage> {
        match &self.properties.borrow().prototype {
            ObjectPrototype::Object(prototype) => Some(prototype.clone()),

            ObjectPrototype::Unknown
            | ObjectPrototype::IntrinsicObjectRoot
            | ObjectPrototype::Null => None,
        }
    }

    pub(super) fn get(&self, name: &str) -> Value {
        if let Some(value) = self.properties.borrow().get(name) {
            return value.clone();
        }

        if let Some(prototype) = self.prototype() {
            return match ObjectState::lookup(&prototype, name) {
                PropertyLookup::Found(value) => value,
                PropertyLookup::Absent | PropertyLookup::Unknown => Value::Undefined,
            };
        }

        if name == "name" {
            Value::String(self.default_name.clone())
        } else {
            Value::Undefined
        }
    }

    pub(super) fn set(&self, name: String, value: Value) {
        self.properties.borrow_mut().set(name, value);
    }

    pub(super) fn has_own_property(&self, name: &str) -> bool {
        self.properties.borrow().get(name).is_some()
    }

    pub(super) fn has_prototype(&self, target: &ObjectStorage) -> Option<bool> {
        let prototype = self.prototype()?;

        if Rc::ptr_eq(&prototype, target) {
            Some(true)
        } else {
            ObjectState::has_prototype(&prototype, target)
        }
    }

    pub(super) fn builtin_name(&self) -> &str {
        &self.default_name
    }

    pub(super) fn attach_prototype_if_missing(&mut self, prototype: ObjectStorage) {
        let mut properties = self.properties.borrow_mut();

        if matches!(properties.prototype, ObjectPrototype::Unknown) {
            properties.prototype = ObjectPrototype::Object(prototype);
        }
    }

    /// Visits immediate references without traversing their children.
    /// The callback must not mutate the visited graph or re-enter JavaScript.
    #[allow(dead_code)]
    pub(super) fn visit_references(&self, visit: &mut impl FnMut(RuntimeRoot<'_>)) {
        visit(RuntimeRoot::Object(&self.properties));
    }
}

impl PartialEq for NativeErrorValue {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.properties, &other.properties)
    }
}

#[derive(Clone, Debug)]
pub struct RegExpValue {
    pub(super) inner: Rc<RefCell<RegExpState>>,
}

#[derive(Debug)]
pub(super) struct RegExpState {
    pub(super) regexp: regexp::object::RegExp,
    pub(super) last_index: Value,
}

impl RegExpValue {
    /// Visits immediate references without traversing their children.
    /// The callback must not mutate the visited graph or re-enter JavaScript.
    #[allow(dead_code)]
    pub(super) fn visit_references(
        &self,
        visit: &mut impl FnMut(RuntimeRoot<'_>),
    ) -> Result<(), RootVisitError> {
        let state = self
            .inner
            .try_borrow()
            .map_err(|_| RootVisitError::RegExpBorrowed)?;

        visit(RuntimeRoot::Value(&state.last_index));
        Ok(())
    }
}

impl PartialEq for RegExpValue {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.inner, &other.inner)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArrayMethodKind {
    Push,
    Pop,
    Join,
}

/// A user function with reference identity and partial own properties.
#[derive(Clone)]
pub struct FunctionValue {
    definition: Rc<UserFunction>,
    properties: ObjectStorage,
    binding: Option<Rc<BoundFunction>>,
    captures: CapturedBindings,
}

pub(super) struct BoundFunction {
    pub(super) target: FunctionValue,
    pub(super) this_arg: Value,
    pub(super) arguments: Vec<Value>,
}

impl FunctionValue {
    pub(super) fn definition(&self) -> &UserFunction {
        &self.definition
    }

    pub(super) fn get(&self, name: &str) -> Value {
        let own = self.properties.borrow().get(name).cloned();

        match own {
            Some(value) => value,

            None if name == "call" => Value::NativeFunction(NativeFunction::FunctionCall),

            None if name == "apply" => Value::NativeFunction(NativeFunction::FunctionApply),

            None if name == "bind" => Value::NativeFunction(NativeFunction::FunctionBind),

            None => Value::Undefined,
        }
    }

    pub(super) fn set(&self, name: String, value: Value) {
        self.properties.borrow_mut().set(name, value);
    }

    pub(super) fn bind(target: Self, this_arg: Value, arguments: Vec<Value>) -> Self {
        Self {
            definition: target.definition.clone(),
            properties: Rc::new(RefCell::new(ObjectState::default())),
            captures: target.captures.clone(),
            binding: Some(Rc::new(BoundFunction {
                target,
                this_arg,
                arguments,
            })),
        }
    }

    pub(super) fn binding(&self) -> Option<&BoundFunction> {
        self.binding.as_deref()
    }

    pub(super) fn is_bound(&self) -> bool {
        self.binding.is_some()
    }

    pub(super) fn captures(&self) -> &CapturedBindings {
        &self.captures
    }

    /// Visits immediate references without traversing their children.
    /// The callback must not mutate the visited graph or re-enter JavaScript.
    #[allow(dead_code)]
    pub(super) fn visit_references(
        &self,
        visit: &mut impl FnMut(RuntimeRoot<'_>),
    ) -> Result<(), RootVisitError> {
        visit(RuntimeRoot::Object(&self.properties));

        if let Some(bound) = &self.binding {
            visit(RuntimeRoot::Function(&bound.target));
            visit(RuntimeRoot::Value(&bound.this_arg));

            for argument in &bound.arguments {
                visit(RuntimeRoot::Value(argument));
            }
        }

        visit_captured_bindings(&self.captures, visit)
    }
}

impl fmt::Debug for FunctionValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FunctionValue")
            .field("identity", &Rc::as_ptr(&self.properties))
            .finish()
    }
}

impl PartialEq for FunctionValue {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.properties, &other.properties)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    String(String),
    Array(ArrayStorage),
    ArrayMethod {
        receiver: ArrayStorage,
        method: ArrayMethodKind,
    },
    Object(ObjectStorage),
    NativeError(NativeErrorValue),
    Url(Url),
    NativeFunction(NativeFunction),
    Document(DocumentStorage),
    Element(u64),
    HtmlCollection(String),
    ElementChildren(u64),
    Attributes(u64),
    Attribute {
        node_id: u64,
        name: String,
    },
    ElementMethod(ElementMethod),
    DocumentMethod(DocumentMethod),
    Function(FunctionValue),
    TestContext,
    ElementConstructor(String),
    TestContextMethod(TestContextMethod),
    NodeList(Vec<u64>),
    ClassList(u64),
    ObjectConstructor,
    RegExp(RegExpValue),
    RegExpTest(RegExpValue),
    RegExpExec(RegExpValue),
    Arguments(ArgumentsValue),
}

impl Value {
    pub(super) fn native_error(name: &str, message: &str) -> Self {
        Self::NativeError(NativeErrorValue::new(name, message))
    }

    pub(super) fn arguments(values: Vec<Value>, potentially_mapped: usize) -> Self {
        Self::Arguments(ArgumentsValue::new(values, potentially_mapped))
    }

    pub(super) fn to_js_string(&self) -> String {
        match self {
            Self::Undefined => "undefined".to_owned(),
            Self::Null => "null".to_owned(),
            Self::Boolean(value) => value.to_string(),
            Self::Number(value) => value.to_string(),
            Self::String(value) => value.clone(),

            Self::Array(values) => values
                .borrow()
                .values
                .iter()
                .map(|value| value.to_js_string())
                .collect::<Vec<_>>()
                .join(","),

            Self::ArrayMethod { .. } => "function".to_owned(),

            Self::Object(_) => "[object Object]".to_owned(),

            Self::NativeError(error) => {
                let name = match error.get("name") {
                    Value::Undefined => "Error".to_owned(),
                    value => value.to_js_string(),
                };

                let message = match error.get("message") {
                    Value::Undefined => String::new(),
                    value => value.to_js_string(),
                };

                if name.is_empty() {
                    message
                } else if message.is_empty() {
                    name
                } else {
                    format!("{name}: {message}")
                }
            }

            Self::Url(url) => url.to_string(),
            Self::NativeFunction(_) => "function".to_owned(),
            Self::Document(_) => "[object Document]".to_owned(),
            Self::Element(_) => "[object HTMLElement]".to_owned(),
            Self::HtmlCollection(_) | Self::ElementChildren(_) => {
                "[object HTMLCollection]".to_owned()
            }
            Self::Attributes(_) => "[object NamedNodeMap]".to_owned(),
            Self::Attribute { .. } => "[object Attr]".to_owned(),
            Self::DocumentMethod(_) => "function".to_owned(),
            Self::Function(_) => "function".to_owned(),
            Self::TestContext => "[object TestContext]".to_owned(),
            Self::ElementConstructor(_) => "function".to_owned(),
            Self::TestContextMethod(_) => "function".to_owned(),
            Self::ElementMethod(_) => "function".to_owned(),
            Self::NodeList(_) => "[object NodeList]".to_owned(),
            Self::ClassList(_) => "[object DOMTokenList]".to_owned(),
            Self::ObjectConstructor => "function".to_owned(),

            // RegExp.prototype.toString is not implemented yet.
            Self::RegExp(_) => "[object RegExp]".to_owned(),
            Self::RegExpTest(_) => "function".to_owned(),
            Self::RegExpExec(_) => "function".to_owned(),

            Self::Arguments(_) => "[object Arguments]".to_owned(),
        }
    }

    pub(crate) fn array(values: Vec<Value>) -> Self {
        let present = vec![true; values.len()];

        Self::Array(Rc::new(RefCell::new(ArrayState {
            values,
            present,
            properties: ObjectState::default(),
        })))
    }

    pub(super) fn array_from_slots(slots: Vec<Option<Value>>) -> Self {
        let mut values = Vec::with_capacity(slots.len());
        let mut present = Vec::with_capacity(slots.len());

        for slot in slots {
            present.push(slot.is_some());
            values.push(slot.unwrap_or(Value::Undefined));
        }

        Self::Array(Rc::new(RefCell::new(ArrayState {
            values,
            present,
            properties: ObjectState::default(),
        })))
    }

    pub(crate) fn object(entries: impl IntoIterator<Item = (String, Value)>) -> Self {
        Self::Object(Rc::new(RefCell::new(ObjectState::from_entries(entries))))
    }

    pub(super) fn object_with_entries_and_prototype(
        entries: impl IntoIterator<Item = (String, Value)>,
        prototype: ObjectStorage,
    ) -> Self {
        let mut state = ObjectState::from_entries(entries);

        state.prototype = ObjectPrototype::Object(prototype);

        Self::Object(Rc::new(RefCell::new(state)))
    }

    pub(super) fn object_with_null_prototype() -> Self {
        let state = ObjectState {
            properties: Vec::new(),
            prototype: ObjectPrototype::Null,
            promise: None,
        };

        Self::Object(Rc::new(RefCell::new(state)))
    }

    pub(super) fn object_with_prototype(prototype: ObjectStorage) -> Self {
        let state = ObjectState {
            properties: Vec::new(),
            prototype: ObjectPrototype::Object(prototype),
            promise: None,
        };

        Self::Object(Rc::new(RefCell::new(state)))
    }

    pub(super) fn intrinsic_object_prototype() -> Self {
        let state = ObjectState {
            properties: Vec::new(),
            prototype: ObjectPrototype::IntrinsicObjectRoot,
            promise: None,
        };

        Self::Object(Rc::new(RefCell::new(state)))
    }

    pub(super) fn function(
        definition: UserFunction,
        object_prototype: ObjectStorage,
        captures: CapturedBindings,
    ) -> Self {
        let mut properties = ObjectState::default();

        if definition.kind == FunctionKind::Ordinary {
            properties.define_own_data_property(
                "prototype".to_owned(),
                Value::object_with_prototype(object_prototype),
                false,
            );
        }

        Self::Function(FunctionValue {
            definition: Rc::new(definition),
            properties: Rc::new(RefCell::new(properties)),
            binding: None,
            captures,
        })
    }

    pub(super) fn error_with_prototype(message: Option<String>, prototype: ObjectStorage) -> Self {
        Self::NativeError(NativeErrorValue::new_with_prototype(message, prototype))
    }

    pub fn as_array(&self) -> Option<Vec<Value>> {
        match self {
            Self::Array(values) => Some(values.borrow().values.clone()),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<HashMap<String, Value>> {
        match self {
            Self::Object(values) => Some(values.borrow().as_hash_map()),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Boolean(value) => Some(*value),
            _ => None,
        }
    }

    /// Visits immediate references without traversing their children.
    /// The callback must not mutate the visited graph or re-enter JavaScript.
    #[allow(dead_code)]
    pub(super) fn visit_references(
        &self,
        visit: &mut impl FnMut(RuntimeRoot<'_>),
    ) -> Result<(), RootVisitError> {
        match self {
            Self::Array(array)
            | Self::ArrayMethod {
                receiver: array, ..
            } => {
                visit(RuntimeRoot::Array(array));
            }

            Self::Object(object) => visit(RuntimeRoot::Object(object)),
            Self::Function(function) => visit(RuntimeRoot::Function(function)),
            Self::Document(document) => visit(RuntimeRoot::Document(document)),

            Self::NativeError(error) => error.visit_references(visit),

            Self::RegExp(regexp) | Self::RegExpTest(regexp) | Self::RegExpExec(regexp) => {
                visit(RuntimeRoot::RegExp(regexp));
            }

            Self::Arguments(arguments) => visit(RuntimeRoot::Arguments(arguments)),

            Self::Undefined
            | Self::Null
            | Self::Boolean(_)
            | Self::Number(_)
            | Self::String(_)
            | Self::Url(_)
            | Self::NativeFunction(_)
            | Self::Element(_)
            | Self::HtmlCollection(_)
            | Self::ElementChildren(_)
            | Self::Attributes(_)
            | Self::Attribute { .. }
            | Self::ElementMethod(_)
            | Self::DocumentMethod(_)
            | Self::TestContext
            | Self::ElementConstructor(_)
            | Self::TestContextMethod(_)
            | Self::NodeList(_)
            | Self::ClassList(_)
            | Self::ObjectConstructor => {}
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{ArrayState, ObjectState, PropertyLookup, RootVisitError, RuntimeRoot, Value};
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn object_updates_existing_properties_without_reordering() {
        let mut object = ObjectState::default();

        object.set("first".into(), Value::Number(1.0));
        object.set("second".into(), Value::Number(2.0));
        object.set("first".into(), Value::Number(3.0));

        assert_eq!(object.keys(), vec!["first", "second"]);
        assert_eq!(object.get("first"), Some(&Value::Number(3.0)));
    }

    #[test]
    fn cloned_array_values_share_the_same_storage() {
        let original = Value::array(vec![Value::Number(1.0)]);
        let alias = original.clone();

        let Value::Array(storage) = alias else {
            unreachable!();
        };

        let mut array = storage.borrow_mut();

        array.values.push(Value::Number(2.0));
        array.present.push(true);

        drop(array);

        assert_eq!(
            original.as_array(),
            Some(vec![Value::Number(1.0), Value::Number(2.0)])
        );
    }

    #[test]
    fn own_keys_sort_only_canonical_array_indices() {
        let mut object = ObjectState::default();

        for key in ["4294967295", "01", "4294967294", "10", "2", "0", "-0"] {
            object.set(key.to_owned(), Value::Undefined);
        }

        assert_eq!(
            object.keys(),
            ["0", "2", "10", "4294967294", "4294967295", "01", "-0",]
        );
    }

    #[test]
    fn non_enumerable_properties_remain_readable_and_stay_hidden_after_assignment() {
        let mut object = ObjectState::default();

        object.define_own_data_property("hidden".into(), Value::Number(1.0), false);

        object.set("visible".into(), Value::Number(2.0));
        object.set("hidden".into(), Value::Number(3.0));

        assert_eq!(object.get("hidden"), Some(&Value::Number(3.0)));
        assert_eq!(object.keys(), ["visible"]);

        assert_eq!(
            object.as_hash_map().get("hidden"),
            Some(&Value::Number(3.0))
        );
    }

    #[test]
    fn non_enumerable_own_property_shadows_enumerable_prototype_property() {
        let Value::Object(parent) = Value::object_with_null_prototype() else {
            panic!("expected object");
        };

        parent.borrow_mut().set("hidden".into(), Value::Number(1.0));
        parent
            .borrow_mut()
            .set("inherited".into(), Value::Number(2.0));

        let Value::Object(child) = Value::object_with_prototype(parent) else {
            panic!("expected object");
        };

        child
            .borrow_mut()
            .define_own_data_property("hidden".into(), Value::Number(3.0), false);

        child.borrow_mut().set("visible".into(), Value::Number(4.0));

        assert!(matches!(
            ObjectState::lookup(&child, "hidden"),
            PropertyLookup::Found(Value::Number(3.0))
        ));

        assert_eq!(
            ObjectState::for_in_keys(&child),
            Some(vec!["visible".to_owned(), "inherited".to_owned()])
        );
    }

    #[test]
    fn function_prototype_stays_non_enumerable_after_assignment() {
        let value = crate::javascript::evaluate(
            "function F() {} \
            F.prototype = {}; \
            F.visible = 1; \
            F;",
        )
        .unwrap();

        let Value::Function(function) = value.into_test_value() else {
            panic!("expected function");
        };

        let properties = function.properties.borrow();

        assert!(matches!(
            properties.get("prototype"),
            Some(Value::Object(_))
        ));

        assert_eq!(properties.keys(), ["visible"]);
    }

    #[test]
    fn enumeration_with_unknown_prototype_is_not_guessed() {
        let Value::Object(object) = Value::object([("own".to_owned(), Value::Number(1.0))]) else {
            panic!("expected object");
        };

        assert_eq!(ObjectState::for_in_keys(&object), None);
    }

    #[test]
    fn visits_object_and_array_references_without_following_cycles() {
        let Value::Object(prototype) = Value::object([]) else {
            unreachable!();
        };

        let Value::Object(object) = Value::object_with_prototype(prototype.clone()) else {
            unreachable!();
        };

        let Value::Array(array) = Value::array(vec![Value::Object(object.clone())]) else {
            unreachable!();
        };

        {
            let mut state = object.borrow_mut();
            state.define_own_data_property("hidden".into(), Value::Array(array.clone()), false);
        }

        {
            let mut state = array.borrow_mut();
            state.present[0] = false;
            state.properties.set("extra".into(), Value::Number(17.0));
        }

        let mut saw_array = false;
        let mut saw_prototype = false;

        ObjectState::visit_references(&object, &mut |root| match root {
            RuntimeRoot::Value(Value::Array(value)) => {
                saw_array = Rc::ptr_eq(value, &array);
            }
            RuntimeRoot::Object(value) => {
                saw_prototype = Rc::ptr_eq(value, &prototype);
            }
            _ => {}
        })
        .unwrap();

        assert!(saw_array && saw_prototype);

        let mut saw_object = false;
        let mut saw_extra = false;

        ArrayState::visit_references(&array, &mut |root| match root {
            RuntimeRoot::Value(Value::Object(value)) => {
                saw_object = Rc::ptr_eq(value, &object);
            }
            RuntimeRoot::Value(Value::Number(17.0)) => saw_extra = true,
            _ => {}
        })
        .unwrap();

        assert!(saw_object && saw_extra);

        {
            let _object_guard = object.borrow_mut();

            assert_eq!(
                ObjectState::visit_references(&object, &mut |_| {}),
                Err(RootVisitError::ObjectBorrowed)
            );

            let _array_guard = array.borrow_mut();

            assert_eq!(
                ArrayState::visit_references(&array, &mut |_| {}),
                Err(RootVisitError::ArrayBorrowed)
            );
        }

        object.borrow_mut().properties.clear();
    }

    #[test]
    fn visits_arguments_regexp_and_native_error_references() {
        let Value::Object(object) = Value::object([]) else {
            unreachable!();
        };
        let Value::Object(prototype) = Value::object([]) else {
            unreachable!();
        };

        let Value::Arguments(arguments) =
            Value::arguments(vec![Value::Object(object.clone()), Value::Number(3.0)], 1)
        else {
            unreachable!();
        };

        let mut values = Vec::new();

        arguments.visit_references(&mut |root| {
            if let RuntimeRoot::Value(value) = root {
                values.push(value.clone());
            }
        });

        assert_eq!(values.len(), 2);
        assert!(matches!(
            &values[0],
            Value::Object(found) if Rc::ptr_eq(found, &object)
        ));
        assert_eq!(values[1], Value::Number(3.0));

        let regexp = super::RegExpValue {
            inner: Rc::new(RefCell::new(super::RegExpState {
                regexp: super::regexp::object::RegExp::compile("a", "g").unwrap(),
                last_index: Value::Object(object.clone()),
            })),
        };

        let mut saw_index = false;

        regexp
            .visit_references(&mut |root| {
                if let RuntimeRoot::Value(Value::Object(found)) = root {
                    saw_index = Rc::ptr_eq(found, &object);
                }
            })
            .unwrap();

        assert!(saw_index);

        {
            let _guard = regexp.inner.borrow_mut();

            assert_eq!(
                regexp.visit_references(&mut |_| {}),
                Err(RootVisitError::RegExpBorrowed)
            );
        }

        let Value::NativeError(error) = Value::error_with_prototype(None, prototype.clone()) else {
            unreachable!();
        };

        error.set("cause".into(), Value::Object(object.clone()));

        let mut saw_properties = false;
        let mut saw_prototype = false;

        error.visit_references(&mut |root| {
            if let RuntimeRoot::Object(found) = root {
                saw_properties |= Rc::ptr_eq(found, &error.properties);
            }
        });

        assert!(saw_properties);

        let mut saw_cause = false;

        ObjectState::visit_references(&error.properties, &mut |root| match root {
            RuntimeRoot::Value(Value::Object(found)) => {
                saw_cause = Rc::ptr_eq(found, &object);
            }

            RuntimeRoot::Object(found) => {
                saw_prototype = Rc::ptr_eq(found, &prototype);
            }

            _ => {}
        })
        .unwrap();

        assert!(saw_cause && saw_prototype);
    }

    #[test]
    fn value_reference_dispatch_covers_wrappers_and_borrow_errors() {
        let Value::Array(array) = Value::array(vec![]) else {
            unreachable!();
        };

        let Value::Object(object) = Value::object([]) else {
            unreachable!();
        };

        let document = Rc::new(RefCell::new(super::DocumentState::default()));

        let function = crate::javascript::evaluate("function f() {} f;")
            .unwrap()
            .into_test_value();

        let regexp = super::RegExpValue {
            inner: Rc::new(RefCell::new(super::RegExpState {
                regexp: super::regexp::object::RegExp::compile("a", "").unwrap(),
                last_index: Value::Object(object.clone()),
            })),
        };

        let arguments = Value::arguments(vec![Value::Number(7.0)], 0);

        let Value::Arguments(arguments_node) = &arguments else {
            unreachable!();
        };

        let Value::Arguments(distinct_arguments) = Value::arguments(vec![Value::Number(7.0)], 0)
        else {
            unreachable!();
        };

        assert_ne!(arguments_node, &distinct_arguments);

        let values = [
            Value::Array(array.clone()),
            Value::ArrayMethod {
                receiver: array.clone(),
                method: super::ArrayMethodKind::Push,
            },
            Value::Object(object.clone()),
            Value::Document(document.clone()),
            function,
            Value::RegExp(regexp.clone()),
            Value::RegExpTest(regexp.clone()),
            Value::RegExpExec(regexp.clone()),
            arguments.clone(),
            Value::native_error("Error", "oops"),
            Value::Null,
        ];

        let mut kinds = Vec::new();

        for value in &values {
            value
                .visit_references(&mut |root| match root {
                    RuntimeRoot::Array(found) if Rc::ptr_eq(found, &array) => {
                        kinds.push("array");
                    }

                    RuntimeRoot::Object(found) if Rc::ptr_eq(found, &object) => {
                        kinds.push("object");
                    }

                    RuntimeRoot::Document(found) if Rc::ptr_eq(found, &document) => {
                        kinds.push("document");
                    }

                    RuntimeRoot::Function(_) => kinds.push("function"),

                    RuntimeRoot::RegExp(found) if found == &regexp => {
                        kinds.push("regexp");
                    }

                    RuntimeRoot::Arguments(found) if found == arguments_node => {
                        kinds.push("arguments");
                    }

                    RuntimeRoot::Object(_) => kinds.push("error properties"),

                    _ => panic!("unexpected reference"),
                })
                .unwrap();
        }

        assert_eq!(
            kinds,
            [
                "array",
                "array",
                "object",
                "document",
                "function",
                "regexp",
                "regexp",
                "regexp",
                "arguments",
                "error properties"
            ]
        );

        {
            let _guard = regexp.inner.borrow_mut();
            let mut saw_regexp = false;

            values[7]
                .visit_references(&mut |root| {
                    if let RuntimeRoot::RegExp(found) = root {
                        saw_regexp = found == &regexp;
                    }
                })
                .unwrap();

            assert!(saw_regexp);

            assert_eq!(
                regexp.visit_references(&mut |_| {}),
                Err(RootVisitError::RegExpBorrowed)
            );
        }

        regexp.inner.borrow_mut().last_index = Value::RegExp(regexp.clone());

        let mut saw_self = false;

        regexp
            .visit_references(&mut |root| {
                if let RuntimeRoot::Value(Value::RegExp(found)) = root {
                    saw_self = found == &regexp;
                }
            })
            .unwrap();

        assert!(saw_self);

        regexp.inner.borrow_mut().last_index = Value::Number(0.0);

        let cyclic_value = Value::arguments(vec![Value::Object(object.clone())], 0);

        let Value::Arguments(cyclic_arguments) = &cyclic_value else {
            unreachable!();
        };

        object
            .borrow_mut()
            .set("cycle".into(), cyclic_value.clone());

        let mut saw_object = false;

        cyclic_arguments.visit_references(&mut |root| {
            if let RuntimeRoot::Value(Value::Object(found)) = root {
                saw_object = Rc::ptr_eq(found, &object);
            }
        });

        let mut saw_arguments = false;

        ObjectState::visit_references(&object, &mut |root| {
            if let RuntimeRoot::Value(Value::Arguments(found)) = root {
                saw_arguments = found == cyclic_arguments;
            }
        })
        .unwrap();

        assert!(saw_object && saw_arguments);
        assert_ne!(cyclic_arguments, arguments_node);

        object.borrow_mut().set("cycle".into(), Value::Undefined);
    }

    #[test]
    fn native_error_aliases_share_a_late_attached_prototype() {
        let Value::NativeError(mut error) = Value::native_error("TypeError", "test") else {
            unreachable!();
        };

        let alias = error.clone();

        let Value::Object(prototype) = Value::object([("custom".into(), Value::Number(42.0))])
        else {
            unreachable!();
        };

        error.attach_prototype_if_missing(prototype.clone());

        assert_eq!(error, alias);
        assert_eq!(alias.get("custom"), Value::Number(42.0));
        assert_eq!(alias.has_prototype(&prototype), Some(true));

        let Value::Object(other) = Value::object([]) else {
            unreachable!();
        };

        error.attach_prototype_if_missing(other);

        assert_eq!(alias.get("custom"), Value::Number(42.0));
    }
}
