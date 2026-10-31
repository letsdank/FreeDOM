//! Runtime values and partial object storage for the JavaScript interpreter.
//!
//! <https://tc39.es/ecma262/#sec-ecmascript-language-types>
//!
//! Object, string, and function semantics are not yet fully implemented.

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use crate::url::Url;

use super::{DocumentState, UserFunction, regexp};

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
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DocumentMethod {
    GetElementById(DocumentState),
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
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TestContextMethod {
    AddCleanup,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ObjectState {
    properties: Vec<(String, Value)>,
    prototype: ObjectPrototype,
}

#[derive(Clone, Debug, Default, PartialEq)]
enum ObjectPrototype {
    #[default]
    Unknown,
    Null,
    Object(ObjectStorage),
}

pub(crate) enum PropertyLookup {
    Found(Value),
    Absent,
    Unknown,
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
            .find(|(property, _)| property == name)
            .map(|(_, value)| value)
    }

    pub(crate) fn set(&mut self, name: String, value: Value) {
        if let Some((_, existing_value)) = self
            .properties
            .iter_mut()
            .find(|(property, _)| property == &name)
        {
            *existing_value = value;
        } else {
            self.properties.push((name, value));
        }
    }

    pub(crate) fn keys(&self) -> Vec<String> {
        let mut indices = Vec::new();
        let mut strings = Vec::new();

        for (key, _) in &self.properties {
            match key.parse::<u32>() {
                Ok(index) if index != u32::MAX && index.to_string() == *key => {
                    indices.push((index, key.clone()));
                }

                _ => strings.push(key.clone()),
            }
        }

        indices.sort_unstable_by_key(|(index, _)| *index);

        indices
            .into_iter()
            .map(|(_, key)| key)
            .chain(strings)
            .collect()
    }

    fn as_hash_map(&self) -> HashMap<String, Value> {
        self.properties.iter().cloned().collect()
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
                ObjectPrototype::Object(prototype) => current = prototype,
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

                (state.keys(), state.prototype.clone())
            };

            for key in keys {
                if seen.insert(key.clone()) {
                    result.push(key);
                }
            }

            match prototype {
                ObjectPrototype::Null => return Some(result),

                ObjectPrototype::Unknown => return None,

                ObjectPrototype::Object(prototype) => {
                    current = prototype;
                }
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ArrayState {
    pub(super) values: Vec<Value>,
    pub(super) properties: ObjectState,
}

pub(crate) type ArrayStorage = Rc<RefCell<ArrayState>>;
pub(crate) type ObjectStorage = Rc<RefCell<ObjectState>>;

/// A partial native Error object; prototypes and property descriptors are not implemented.
#[derive(Clone, Debug)]
pub struct NativeErrorValue {
    default_name: String,
    properties: ObjectStorage,
}

impl NativeErrorValue {
    fn new(name: &str, message: &str) -> Self {
        let mut properties = ObjectState::default();

        properties.set("message".to_owned(), Value::String(message.to_owned()));

        Self {
            default_name: name.to_owned(),
            properties: Rc::new(RefCell::new(properties)),
        }
    }

    pub(super) fn get(&self, name: &str) -> Value {
        if let Some(value) = self.properties.borrow().get(name) {
            return value.clone();
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
    Document(DocumentState),
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
    Function(UserFunction),
    TestContext,
    ElementConstructor(String),
    TestContextMethod(TestContextMethod),
    NodeList(Vec<u64>),
    ClassList(u64),
    ObjectConstructor,
    RegExp(RegExpValue),
    RegExpTest(RegExpValue),
    RegExpExec(RegExpValue),
}

impl Value {
    pub(super) fn native_error(name: &str, message: &str) -> Self {
        Self::NativeError(NativeErrorValue::new(name, message))
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
        }
    }

    pub(crate) fn array(values: Vec<Value>) -> Self {
        Self::Array(Rc::new(RefCell::new(ArrayState {
            values,
            properties: ObjectState::default(),
        })))
    }

    pub(crate) fn object(entries: impl IntoIterator<Item = (String, Value)>) -> Self {
        Self::Object(Rc::new(RefCell::new(ObjectState::from_entries(entries))))
    }

    pub(super) fn object_with_null_prototype() -> Self {
        let state = ObjectState {
            properties: Vec::new(),
            prototype: ObjectPrototype::Null,
        };

        Self::Object(Rc::new(RefCell::new(state)))
    }

    pub(super) fn object_with_prototype(prototype: ObjectStorage) -> Self {
        let state = ObjectState {
            properties: Vec::new(),
            prototype: ObjectPrototype::Object(prototype),
        };

        Self::Object(Rc::new(RefCell::new(state)))
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
}

#[cfg(test)]
mod tests {
    use super::{ObjectState, Value};

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

        storage.borrow_mut().values.push(Value::Number(2.0));

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
}
