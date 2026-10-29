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
        self.properties
            .iter()
            .map(|(name, _)| name.clone())
            .collect()
    }

    fn as_hash_map(&self) -> HashMap<String, Value> {
        self.properties.iter().cloned().collect()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ArrayState {
    pub(super) values: Vec<Value>,
    pub(super) properties: ObjectState,
}

pub(crate) type ArrayStorage = Rc<RefCell<ArrayState>>;
pub(crate) type ObjectStorage = Rc<RefCell<ObjectState>>;

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
}
