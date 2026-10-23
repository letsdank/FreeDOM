use crate::url::Url;
use std::{cell::RefCell, collections::HashMap, error::Error, fmt, rc::Rc};

#[path = "javascript/lexer.rs"]
mod lexer;

pub use lexer::{Token, TokenKind, tokenize};

#[path = "javascript/parser.rs"]
mod parser;

pub use parser::parse_json;
use parser::parse_program;

#[path = "javascript/dom.rs"]
mod dom;

pub use dom::DocumentState;

#[path = "javascript/runtime.rs"]
mod runtime;

#[path = "javascript/ast.rs"]
mod ast;

pub(crate) use ast::{BinaryOperator, Expression, Statement, UserFunction};

use runtime::{Completion, Interpreter};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeFunction {
    AssertEquals,
    Test,
    AssertTrue,
    AssertFalse,
    AssertNotEquals,
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

pub(crate) type ArrayStorage = Rc<RefCell<Vec<Value>>>;
pub(crate) type ObjectStorage = Rc<RefCell<HashMap<String, Value>>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArrayMethodKind {
    Push,
    Pop,
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
}

impl Value {
    fn to_js_string(&self) -> String {
        match self {
            Self::Undefined => "undefined".to_owned(),
            Self::Null => "null".to_owned(),
            Self::Boolean(value) => value.to_string(),
            Self::Number(value) => value.to_string(),
            Self::String(value) => value.clone(),

            Self::Array(values) => values
                .borrow()
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
        }
    }

    pub(crate) fn array(values: Vec<Value>) -> Self {
        Self::Array(Rc::new(RefCell::new(values)))
    }

    pub(crate) fn object(values: HashMap<String, Value>) -> Self {
        Self::Object(Rc::new(RefCell::new(values)))
    }

    pub fn as_array(&self) -> Option<Vec<Value>> {
        match self {
            Self::Array(values) => Some(values.borrow().clone()),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<HashMap<String, Value>> {
        match self {
            Self::Object(values) => Some(values.borrow().clone()),
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsError {
    pub offset: Option<usize>,
    pub message: String,
}

impl JsError {
    fn syntax(offset: usize, message: impl Into<String>) -> Self {
        Self {
            offset: Some(offset),
            message: message.into(),
        }
    }

    fn runtime(message: impl Into<String>) -> Self {
        Self {
            offset: None,
            message: message.into(),
        }
    }
}

impl fmt::Display for JsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.offset {
            Some(offset) => write!(f, "{} at byte {offset}", self.message),
            None => write!(f, "{}", self.message),
        }
    }
}

impl Error for JsError {}

pub fn evaluate(source: &str) -> Result<Value, JsError> {
    let program = parse_program(source)?;
    Interpreter::new()
        .execute(&program)
        .map(Completion::into_value)
}

pub fn evaluate_with_document(
    source: &str,
    document: &mut DocumentState,
) -> Result<Value, JsError> {
    let program = parse_program(source)?;

    let mut interpreter = Interpreter::new();
    interpreter.set_document(document.clone());

    let result = interpreter.execute(&program);

    let Some(updated_document) = interpreter.take_document() else {
        return Err(JsError::runtime(
            "TypeError: global document binding must remain a Document",
        ));
    };

    *document = updated_document;

    result.map(Completion::into_value)
}

#[cfg(test)]
#[path = "javascript/tests.rs"]
mod tests;
