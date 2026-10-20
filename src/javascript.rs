use crate::{
    freedom_url::Url,
    html::{Document as HtmlDocument, Node as HtmlNode},
};
use std::{collections::HashMap, error::Error, fmt};

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    Identifier(String),
    String(String),
    Number(f64),

    Let,
    Const,
    Var,
    New,
    True,
    False,
    Null,

    LeftParen,
    RightParen,
    LeftBrace,
    RightBrace,
    LeftBracket,
    RightBracket,

    Dot,
    Comma,
    Semicolon,
    Colon,
    Question,

    Plus,
    Minus,
    Star,
    Slash,

    Assign,
    Equal,
    StrictEqual,
    Not,
    NotEqual,
    StrictNotEqual,

    Less,
    LessEqual,
    Greater,
    GreaterEqual,

    Arrow,
    EndOfFile,

    Function,
    Instanceof,
    Return,

    If,
    Else,

    And,
    Or,
    While,
    For,
    Break,
    Continue,
    PlusPlus,
    MinusMinus,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub offset: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LexError {
    pub offset: usize,
    pub message: String,
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.message, self.offset)
    }
}

impl Error for LexError {}

pub fn tokenize(source: &str) -> Result<Vec<Token>, LexError> {
    Lexer::new(source).tokenize()
}

struct Lexer<'source> {
    source: &'source str,
    index: usize,
    tokens: Vec<Token>,
}

impl<'source> Lexer<'source> {
    fn new(source: &'source str) -> Self {
        Self {
            source,
            index: 0,
            tokens: Vec::new(),
        }
    }

    fn tokenize(mut self) -> Result<Vec<Token>, LexError> {
        while let Some(character) = self.advance() {
            let offset = self.index - character.len_utf8();

            match character {
                character if character.is_ascii_whitespace() => {}

                '/' if self.consume_if('/') => self.skip_line_comment(),

                '/' if self.consume_if('*') => self.skip_block_comment(offset)?,

                '"' | '\'' => self.read_string(character, offset)?,

                character if character.is_ascii_digit() => self.read_number(offset)?,

                character if is_identifier_start(character) => self.read_identifier(offset),

                '(' => self.push(TokenKind::LeftParen, offset),
                ')' => self.push(TokenKind::RightParen, offset),
                '{' => self.push(TokenKind::LeftBrace, offset),
                '}' => self.push(TokenKind::RightBrace, offset),
                '[' => self.push(TokenKind::LeftBracket, offset),
                ']' => self.push(TokenKind::RightBracket, offset),
                '.' => self.push(TokenKind::Dot, offset),
                ',' => self.push(TokenKind::Comma, offset),
                ';' => self.push(TokenKind::Semicolon, offset),
                ':' => self.push(TokenKind::Colon, offset),
                '?' => self.push(TokenKind::Question, offset),
                '+' if self.consume_if('+') => self.push(TokenKind::PlusPlus, offset),
                '-' if self.consume_if('-') => self.push(TokenKind::MinusMinus, offset),
                '+' => self.push(TokenKind::Plus, offset),
                '-' => self.push(TokenKind::Minus, offset),
                '*' => self.push(TokenKind::Star, offset),
                '/' => self.push(TokenKind::Slash, offset),

                '=' if self.consume_if('=') && self.consume_if('=') => {
                    self.push(TokenKind::StrictEqual, offset);
                }

                '=' if self.consume_if('=') => self.push(TokenKind::Equal, offset),

                '=' if self.consume_if('>') => self.push(TokenKind::Arrow, offset),

                '=' => self.push(TokenKind::Assign, offset),

                '!' if self.consume_if('=') && self.consume_if('=') => {
                    self.push(TokenKind::StrictNotEqual, offset);
                }

                '!' if self.consume_if('=') => self.push(TokenKind::NotEqual, offset),

                '!' => self.push(TokenKind::Not, offset),

                '<' if self.consume_if('=') => self.push(TokenKind::LessEqual, offset),
                '<' => self.push(TokenKind::Less, offset),

                '>' if self.consume_if('=') => self.push(TokenKind::GreaterEqual, offset),
                '>' => self.push(TokenKind::Greater, offset),

                '&' if self.consume_if('&') => self.push(TokenKind::And, offset),
                '|' if self.consume_if('|') => self.push(TokenKind::Or, offset),

                unexpected => {
                    return Err(LexError {
                        offset,
                        message: format!("unexpected character {unexpected:?}"),
                    });
                }
            }
        }

        self.push(TokenKind::EndOfFile, self.index);

        Ok(self.tokens)
    }

    fn read_string(&mut self, quote: char, offset: usize) -> Result<(), LexError> {
        let mut value = String::new();

        loop {
            let Some(character) = self.advance() else {
                return Err(LexError {
                    offset,
                    message: "unterminated string literal".to_owned(),
                });
            };

            if character == quote {
                break;
            }

            if matches!(character, '\n' | '\r') {
                return Err(LexError {
                    offset,
                    message: "newline in string literal".to_owned(),
                });
            }

            if character != '\\' {
                value.push(character);
                continue;
            }

            let Some(escape) = self.advance() else {
                return Err(LexError {
                    offset,
                    message: "unterminated escape sequence".to_owned(),
                });
            };

            match escape {
                '"' => value.push('"'),
                '\'' => value.push('\''),
                '\\' => value.push('\\'),
                '/' => value.push('/'),
                'b' => value.push('\u{0008}'),
                'f' => value.push('\u{000C}'),
                'n' => value.push('\n'),
                'r' => value.push('\r'),
                't' => value.push('\t'),
                'u' => value.push(self.read_unicode_escape(offset)?),

                unknown => {
                    return Err(LexError {
                        offset,
                        message: format!("unsupported escape sequence \\{unknown}"),
                    });
                }
            }
        }

        self.push(TokenKind::String(value), offset);

        Ok(())
    }

    fn read_unicode_escape(&mut self, offset: usize) -> Result<char, LexError> {
        let mut value = 0_u32;

        for _ in 0..4 {
            let Some(character) = self.advance() else {
                return Err(LexError {
                    offset,
                    message: "incomplete Unicode escape".to_owned(),
                });
            };

            let Some(digit) = character.to_digit(16) else {
                return Err(LexError {
                    offset,
                    message: "invalid Unicode escape".to_owned(),
                });
            };

            value = value * 16 + digit;
        }

        char::from_u32(value).ok_or_else(|| LexError {
            offset,
            message: "invalid Unicode scalar value".to_owned(),
        })
    }

    fn read_number(&mut self, offset: usize) -> Result<(), LexError> {
        while self
            .peek()
            .is_some_and(|character| character.is_ascii_digit())
        {
            self.advance();
        }

        if self.peek() == Some('.') {
            self.advance();

            while self
                .peek()
                .is_some_and(|character| character.is_ascii_digit())
            {
                self.advance();
            }
        }

        if matches!(self.peek(), Some('e' | 'E')) {
            let exponent_start = self.index;
            self.advance();

            if matches!(self.peek(), Some('+' | '-')) {
                self.advance();
            }

            if self
                .peek()
                .is_some_and(|character| character.is_ascii_digit())
            {
                while self
                    .peek()
                    .is_some_and(|character| character.is_ascii_digit())
                {
                    self.advance();
                }
            } else {
                self.index = exponent_start;
            }
        }

        let number = self.source[offset..self.index]
            .parse::<f64>()
            .map_err(|_| LexError {
                offset,
                message: "invalid number literal".to_owned(),
            })?;

        self.push(TokenKind::Number(number), offset);

        Ok(())
    }

    fn read_identifier(&mut self, offset: usize) {
        while self.peek().is_some_and(is_identifier_continue) {
            self.advance();
        }

        let identifier = &self.source[offset..self.index];

        let kind = match identifier {
            "let" => TokenKind::Let,
            "const" => TokenKind::Const,
            "var" => TokenKind::Var,
            "new" => TokenKind::New,
            "true" => TokenKind::True,
            "false" => TokenKind::False,
            "null" => TokenKind::Null,
            "function" => TokenKind::Function,
            "instanceof" => TokenKind::Instanceof,
            "return" => TokenKind::Return,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "while" => TokenKind::While,
            "for" => TokenKind::For,
            "break" => TokenKind::Break,
            "continue" => TokenKind::Continue,
            _ => TokenKind::Identifier(identifier.to_owned()),
        };

        self.push(kind, offset);
    }

    fn skip_line_comment(&mut self) {
        while self.peek().is_some_and(|character| character != '\n') {
            self.advance();
        }
    }

    fn skip_block_comment(&mut self, offset: usize) -> Result<(), LexError> {
        loop {
            let Some(character) = self.advance() else {
                return Err(LexError {
                    offset,
                    message: "unterminated block comment".to_owned(),
                });
            };

            if character == '*' && self.consume_if('/') {
                return Ok(());
            }
        }
    }

    fn advance(&mut self) -> Option<char> {
        let character = self.peek()?;
        self.index += character.len_utf8();
        Some(character)
    }

    fn consume_if(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<char> {
        self.source[self.index..].chars().next()
    }

    fn push(&mut self, kind: TokenKind, offset: usize) {
        self.tokens.push(Token { kind, offset });
    }
}

fn is_identifier_start(character: char) -> bool {
    character.is_ascii_alphabetic() || matches!(character, '_' | '$')
}

fn is_identifier_continue(character: char) -> bool {
    is_identifier_start(character) || character.is_ascii_digit()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeFunction {
    AssertEquals,
    Test,
    AssertTrue,
    AssertFalse,
    AssertNotEquals,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AttributeState {
    name: String,
    value: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ElementState {
    node_id: u64,
    id: String,
    tag_name: String,
    text_content: String,
    text_content_was_set: bool,
    attributes: Vec<AttributeState>,
    parent: Option<u64>,
    children: Vec<u64>,
    connected: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DocumentState {
    pub title: String,
    root_nodes: Vec<u64>,
    elements_by_node_id: HashMap<u64, ElementState>,
    body: Option<u64>,
    next_node_id: u64,
}

impl DocumentState {
    pub fn with_title(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..Self::default()
        }
    }

    fn insert_element(
        &mut self,
        id: &str,
        tag_name: &str,
        text_content: String,
        parent: Option<u64>,
        connected: bool,
        is_document_node: bool,
    ) -> u64 {
        let node_id = self.next_node_id;
        self.next_node_id += 1;

        let parent = parent.filter(|node_id| self.elements_by_node_id.contains_key(node_id));

        let mut attributes = Vec::new();

        if !id.is_empty() {
            attributes.push(AttributeState {
                name: "id".to_owned(),
                value: id.to_owned(),
            });
        }

        self.elements_by_node_id.insert(
            node_id,
            ElementState {
                node_id,
                id: id.to_owned(),
                tag_name: tag_name.to_owned(),
                text_content,
                text_content_was_set: false,
                attributes,
                parent,
                children: Vec::new(),
                connected,
            },
        );

        if let Some(parent_node_id) = parent {
            if let Some(parent) = self.elements_by_node_id.get_mut(&parent_node_id) {
                parent.children.push(node_id);
            }
        }

        if is_document_node && parent.is_none() {
            self.root_nodes.push(node_id);
        }

        node_id
    }

    pub fn register_parsed_element(
        &mut self,
        tag_name: &str,
        id: &str,
        text_content: String,
        parent: Option<u64>,
    ) -> u64 {
        let node_id = self.insert_element(id, tag_name, text_content, parent, true, true);

        if tag_name == "body" && self.body.is_none() {
            self.body = Some(node_id);
        }

        node_id
    }

    pub fn register_element(&mut self, id: &str, tag_name: &str, text_content: String) {
        if !id.is_empty() {
            self.register_parsed_element(tag_name, id, text_content, None);
        }
    }

    pub fn register_body(&mut self, id: &str, text_content: String) {
        if self.body.is_none() {
            self.register_parsed_element("body", id, text_content, None);
        }
    }

    pub fn register_element_attribute(&mut self, node_id: u64, name: &str, value: String) {
        self.set_node_attribute(node_id, name, value);
    }

    fn create_element(&mut self, tag_name: &str) -> u64 {
        self.insert_element("", tag_name, String::new(), None, false, false)
    }

    fn body_node(&self) -> Option<u64> {
        self.body
    }

    fn get_element_by_id(&self, id: &str) -> Option<u64> {
        if id.is_empty() {
            return None;
        }

        for node_id in &self.root_nodes {
            if let Some(found) = self.find_element_by_id(*node_id, id) {
                return Some(found);
            }
        }

        None
    }

    fn find_element_by_id(&self, node_id: u64, id: &str) -> Option<u64> {
        let element = self.elements_by_node_id.get(&node_id)?;

        if !element.connected {
            return None;
        }

        if element.id == id {
            return Some(node_id);
        }

        for child_node_id in &element.children {
            if let Some(found) = self.find_element_by_id(*child_node_id, id) {
                return Some(found);
            }
        }

        None
    }

    fn get_elements_by_tag_name(&self, tag_name: &str) -> Vec<u64> {
        let tag_name = tag_name.to_ascii_lowercase();
        let mut result = Vec::new();

        for node_id in &self.root_nodes {
            self.collect_elements_by_tag_name(*node_id, &tag_name, &mut result);
        }

        result
    }

    fn collect_elements_by_tag_name(&self, node_id: u64, tag_name: &str, result: &mut Vec<u64>) {
        let Some(element) = self.elements_by_node_id.get(&node_id) else {
            return;
        };

        if !element.connected {
            return;
        }

        if tag_name == "*" || element.tag_name == tag_name {
            result.push(node_id);
        }

        for child_node_id in &element.children {
            self.collect_elements_by_tag_name(*child_node_id, tag_name, result);
        }
    }

    fn element(&self, node_id: u64) -> Option<&ElementState> {
        self.elements_by_node_id.get(&node_id)
    }

    fn set_subtree_connected(&mut self, node_id: u64, connected: bool) {
        let Some(element) = self.elements_by_node_id.get_mut(&node_id) else {
            return;
        };

        element.connected = connected;
        let children = element.children.clone();

        for child_node_id in children {
            self.set_subtree_connected(child_node_id, connected);
        }
    }

    fn set_node_text_content(&mut self, node_id: u64, text_content: String) -> bool {
        let Some(element) = self.elements_by_node_id.get_mut(&node_id) else {
            return false;
        };

        element.text_content = text_content;
        element.text_content_was_set = true;
        true
    }

    fn set_node_attribute(&mut self, node_id: u64, name: &str, value: String) -> bool {
        let name = name.to_ascii_lowercase();
        let is_id = name == "id";

        let Some(element) = self.elements_by_node_id.get_mut(&node_id) else {
            return false;
        };

        if let Some(attribute) = element
            .attributes
            .iter_mut()
            .find(|attribute| attribute.name == name)
        {
            attribute.value = value.clone();
        } else {
            element.attributes.push(AttributeState {
                name,
                value: value.clone(),
            });
        }

        if is_id {
            element.id = value.clone();
        }

        true
    }

    fn remove_node_attribute(&mut self, node_id: u64, name: &str) -> bool {
        let name = name.to_ascii_lowercase();

        let Some(element) = self.elements_by_node_id.get_mut(&node_id) else {
            return false;
        };

        element
            .attributes
            .retain(|attribute| attribute.name != name);

        if name == "id" {
            element.id.clear();
        }

        true
    }

    fn append_child(&mut self, parent_node_id: u64, child_node_id: u64) -> bool {
        self.insert_child(parent_node_id, child_node_id, None)
    }

    fn insert_before(
        &mut self,
        parent_node_id: u64,
        child_node_id: u64,
        reference_node_id: u64,
    ) -> bool {
        let Some(parent) = self.elements_by_node_id.get(&parent_node_id) else {
            return false;
        };

        let Some(index) = parent
            .children
            .iter()
            .position(|node_id| *node_id == reference_node_id)
        else {
            return false;
        };

        self.insert_child(parent_node_id, child_node_id, Some(index))
    }

    fn insert_child(
        &mut self,
        parent_node_id: u64,
        child_node_id: u64,
        index: Option<usize>,
    ) -> bool {
        if parent_node_id == child_node_id {
            return false;
        }

        let Some(parent) = self.elements_by_node_id.get(&parent_node_id) else {
            return false;
        };

        let parent_connected = parent.connected;

        let Some(child) = self.elements_by_node_id.get(&child_node_id) else {
            return false;
        };

        let old_parent = child.parent;

        if let Some(old_parent_node_id) = old_parent {
            if let Some(old_parent) = self.elements_by_node_id.get_mut(&old_parent_node_id) {
                old_parent
                    .children
                    .retain(|node_id| *node_id != child_node_id);
            }
        } else {
            self.root_nodes.retain(|node_id| *node_id != child_node_id);
        }

        let Some(child) = self.elements_by_node_id.get_mut(&child_node_id) else {
            return false;
        };

        child.parent = Some(parent_node_id);

        let Some(parent) = self.elements_by_node_id.get_mut(&parent_node_id) else {
            return false;
        };

        let index = index
            .unwrap_or(parent.children.len())
            .min(parent.children.len());
        parent.children.insert(index, child_node_id);

        self.set_subtree_connected(child_node_id, parent_connected);

        true
    }

    fn remove_child(&mut self, parent_node_id: u64, child_node_id: u64) -> bool {
        let Some(child) = self.elements_by_node_id.get(&child_node_id) else {
            return false;
        };

        if child.parent != Some(parent_node_id) {
            return false;
        }

        let Some(parent) = self.elements_by_node_id.get_mut(&parent_node_id) else {
            return false;
        };

        parent.children.retain(|node_id| *node_id != child_node_id);

        let Some(child) = self.elements_by_node_id.get_mut(&child_node_id) else {
            return false;
        };

        child.parent = None;

        self.set_subtree_connected(child_node_id, false);

        true
    }

    fn set_node_inner_html(&mut self, node_id: u64, html: &str) -> bool {
        let fragment = HtmlDocument::parse(html);
        let fragment_text = fragment.text_content();

        let (previous_children, connected) = {
            let Some(element) = self.elements_by_node_id.get_mut(&node_id) else {
                return false;
            };

            element.text_content = fragment_text;
            element.text_content_was_set = false;

            (std::mem::take(&mut element.children), element.connected)
        };

        for child_node_id in previous_children {
            if let Some(child) = self.elements_by_node_id.get_mut(&child_node_id) {
                child.parent = None;
            }

            self.set_subtree_connected(child_node_id, false);
        }

        for node in &fragment.children {
            let _ = self.import_html_node(node, Some(node_id), connected);
        }

        true
    }

    fn set_node_outer_html(&mut self, node_id: u64, html: &str) -> bool {
        let fragment = HtmlDocument::parse(html);

        let Some(element) = self.elements_by_node_id.get(&node_id) else {
            return false;
        };

        let parent = element.parent;
        let connected = element.connected;

        let index = if let Some(parent_node_id) = parent {
            let Some(parent_element) = self.elements_by_node_id.get(&parent_node_id) else {
                return false;
            };

            let Some(index) = parent_element
                .children
                .iter()
                .position(|child_node_id| *child_node_id == node_id)
            else {
                return false;
            };

            index
        } else if let Some(index) = self
            .root_nodes
            .iter()
            .position(|root_node_id| *root_node_id == node_id)
        {
            index
        } else {
            return true;
        };

        if let Some(parent_node_id) = parent {
            let parent_element = self
                .elements_by_node_id
                .get_mut(&parent_node_id)
                .expect("parent was checked above");

            parent_element.children.remove(index);
        } else {
            self.root_nodes.remove(index);
        }

        if let Some(element) = self.elements_by_node_id.get_mut(&node_id) {
            element.parent = None;
        }

        self.set_subtree_connected(node_id, false);

        let mut inserted = 0;

        for node in &fragment.children {
            let Some(imported_node_id) = self.import_html_node(node, parent, connected) else {
                continue;
            };

            if let Some(parent_node_id) = parent {
                let parent_element = self
                    .elements_by_node_id
                    .get_mut(&parent_node_id)
                    .expect("parent was checked above");

                parent_element
                    .children
                    .retain(|child_node_id| *child_node_id != imported_node_id);

                parent_element
                    .children
                    .insert(index + inserted, imported_node_id);
            } else {
                self.root_nodes.insert(index + inserted, imported_node_id);
            }

            inserted += 1;
        }

        true
    }

    fn import_html_node(
        &mut self,
        node: &HtmlNode,
        parent: Option<u64>,
        connected: bool,
    ) -> Option<u64> {
        let HtmlNode::Element(element) = node else {
            return None;
        };

        let id = element
            .attributes
            .iter()
            .find(|attribute| attribute.name == "id")
            .map(|attribute| attribute.value.as_str())
            .unwrap_or("");

        let node_id = self.insert_element(
            id,
            &element.name,
            html_node_text_content(node),
            parent,
            connected,
            false,
        );

        for attribute in &element.attributes {
            self.set_node_attribute(node_id, &attribute.name, attribute.value.clone());
        }

        for child in &element.children {
            let _ = self.import_html_node(child, Some(node_id), connected);
        }

        Some(node_id)
    }

    pub fn set_element_text_content(&mut self, id: &str, text_content: String) -> bool {
        let Some(node_id) = self.get_element_by_id(id) else {
            return false;
        };

        self.set_node_text_content(node_id, text_content)
    }

    pub fn text_content_override(&self, id: &str) -> Option<&str> {
        let node_id = self.get_element_by_id(id)?;
        let element = self.element(node_id)?;

        if element.text_content_was_set {
            Some(element.text_content.as_str())
        } else {
            None
        }
    }
}

fn html_node_text_content(node: &HtmlNode) -> String {
    match node {
        HtmlNode::Text(text) => text.clone(),

        HtmlNode::Element(element) => element
            .children
            .iter()
            .map(html_node_text_content)
            .collect(),

        HtmlNode::Doctype(_) | HtmlNode::Comment(_) => String::new(),
    }
}

#[derive(Clone, Debug, PartialEq)]
enum DocumentMethod {
    GetElementById(DocumentState),
    CreateElement,
    GetElementsByTagName,
}

#[derive(Clone, Debug, PartialEq)]
enum ElementMethod {
    GetAttribute(u64),
    SetAttribute(u64),
    AppendChild(u64),
    RemoveChild(u64),
    RemoveAttribute(u64),
    InsertBefore(u64),
}

#[derive(Clone, Debug, PartialEq)]
enum TestContextMethod {
    AddCleanup,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Object(HashMap<String, Value>),
    Url(Url),
    NativeFunction(NativeFunction),
    Document(DocumentState),
    Element(u64),
    HtmlCollection(String),
    Attributes(u64),
    Attribute { node_id: u64, name: String },
    ElementMethod(ElementMethod),
    DocumentMethod(DocumentMethod),
    Function(UserFunction),
    TestContext,
    ElementConstructor(String),
    TestContextMethod(TestContextMethod),
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
                .iter()
                .map(|value| value.to_js_string())
                .collect::<Vec<_>>()
                .join(","),

            Self::Object(_) => "[object Object]".to_owned(),
            Self::Url(url) => url.to_string(),
            Self::NativeFunction(_) => "function".to_owned(),
            Self::Document(_) => "[object Document]".to_owned(),
            Self::Element(_) => "[object HTMLElement]".to_owned(),
            Self::HtmlCollection(_) => "[object HTMLCollection]".to_owned(),
            Self::Attributes(_) => "[object NamedNodeMap]".to_owned(),
            Self::Attribute { .. } => "[object Attr]".to_owned(),
            Self::DocumentMethod(_) => "function".to_owned(),
            Self::Function(_) => "function".to_owned(),
            Self::TestContext => "[object TestContext]".to_owned(),
            Self::ElementConstructor(_) => "function".to_owned(),
            Self::TestContextMethod(_) => "function".to_owned(),
            Self::ElementMethod(_) => "function".to_owned(),
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Self::Array(values) => Some(values),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&HashMap<String, Value>> {
        match self {
            Self::Object(values) => Some(values),
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

#[derive(Clone, Debug, PartialEq)]
enum Statement {
    Declaration {
        name: String,
        value: Expression,
    },
    Expression(Expression),
    Return(Expression),
    If {
        condition: Expression,
        then_body: Vec<Statement>,
        else_body: Vec<Statement>,
    },
    While {
        condition: Expression,
        body: Vec<Statement>,
    },
    For {
        initializer: Option<Box<Statement>>,
        condition: Option<Expression>,
        update: Option<Expression>,
        body: Vec<Statement>,
    },
    Break,
    Continue,
}

enum Completion {
    Normal(Value),
    Return(Value),
    Break,
    Continue,
}

impl Completion {
    fn into_value(self) -> Value {
        match self {
            Self::Normal(value) | Self::Return(value) => value,
            Self::Break | Self::Continue => Value::Undefined,
        }
    }
}

fn function_local_names(statements: &[Statement]) -> Vec<String> {
    let mut names = Vec::new();

    for statement in statements {
        match statement {
            Statement::Declaration { name, .. } => {
                if !names.contains(name) {
                    names.push(name.clone());
                }
            }

            Statement::If {
                then_body,
                else_body,
                ..
            } => {
                for name in function_local_names(then_body)
                    .into_iter()
                    .chain(function_local_names(else_body))
                {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
            }

            Statement::While { body, .. } => {
                for name in function_local_names(body) {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
            }

            Statement::For {
                initializer, body, ..
            } => {
                if let Some(initializer) = initializer {
                    for name in function_local_names(std::slice::from_ref(initializer.as_ref())) {
                        if !names.contains(&name) {
                            names.push(name);
                        }
                    }
                }

                for name in function_local_names(body) {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
            }

            Statement::Expression(_)
            | Statement::Return(_)
            | Statement::Break
            | Statement::Continue => {}
        }
    }

    names
}

#[derive(Clone, Debug, PartialEq)]
enum Expression {
    Literal(Value),
    Identifier(String),
    New {
        constructor: String,
        arguments: Vec<Expression>,
    },
    Member {
        object: Box<Expression>,
        property: String,
    },
    Call {
        callee: Box<Expression>,
        arguments: Vec<Expression>,
    },
    Assignment {
        target: Box<Expression>,
        value: Box<Expression>,
    },
    Binary {
        left: Box<Expression>,
        operator: BinaryOperator,
        right: Box<Expression>,
    },
    Function(UserFunction),
    Index {
        object: Box<Expression>,
        index: Box<Expression>,
    },
    Not(Box<Expression>),
    Update {
        target: Box<Expression>,
        delta: f64,
        prefix: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
struct UserFunction {
    parameters: Vec<String>,
    body: Vec<Statement>,
}

#[derive(Clone, Debug)]
struct Cleanup {
    callback: UserFunction,
    captured: HashMap<String, Value>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum BinaryOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Equal,
    StrictEqual,
    NotEqual,
    StrictNotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Instanceof,
    And,
    Or,
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
    interpreter
        .variables
        .insert("document".to_owned(), Value::Document(document.clone()));

    let result = interpreter.execute(&program);

    let Some(Value::Document(updated_document)) = interpreter.variables.remove("document") else {
        return Err(JsError::runtime(
            "TypeError: global document binding must remain a Document",
        ));
    };

    *document = updated_document;

    result.map(Completion::into_value)
}

fn parse_program(source: &str) -> Result<Vec<Statement>, JsError> {
    let tokens = tokenize(source).map_err(|error| JsError {
        offset: Some(error.offset),
        message: error.message,
    })?;

    Parser::new(tokens).parse_program()
}

pub fn parse_json(source: &str) -> Result<Value, JsError> {
    let tokens = tokenize(source).map_err(|error| JsError {
        offset: Some(error.offset),
        message: error.message,
    })?;

    JsonParser::new(tokens).parse()
}

struct JsonParser {
    tokens: Vec<Token>,
    position: usize,
}

impl JsonParser {
    fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            position: 0,
        }
    }

    fn parse(&mut self) -> Result<Value, JsError> {
        let value = self.parse_value()?;

        if !self.check(&TokenKind::EndOfFile) {
            return Err(JsError::syntax(
                self.peek().offset,
                "unexpected token after JSON value",
            ));
        }

        Ok(value)
    }

    fn parse_value(&mut self) -> Result<Value, JsError> {
        let token = self.advance();

        match token.kind {
            TokenKind::String(value) => Ok(Value::String(value)),
            TokenKind::Number(value) => Ok(Value::Number(value)),
            TokenKind::True => Ok(Value::Boolean(true)),
            TokenKind::False => Ok(Value::Boolean(false)),
            TokenKind::Null => Ok(Value::Null),
            TokenKind::LeftBracket => self.parse_array(),
            TokenKind::LeftBrace => self.parse_object(),

            TokenKind::EndOfFile => Err(JsError::syntax(
                token.offset,
                "unexpected end of JSON input",
            )),

            _ => Err(JsError::syntax(token.offset, "expected a JSON value")),
        }
    }

    fn parse_array(&mut self) -> Result<Value, JsError> {
        let mut values = Vec::new();

        if self.consume_optional(&TokenKind::RightBracket) {
            return Ok(Value::Array(values));
        }

        loop {
            values.push(self.parse_value()?);

            if self.consume_optional(&TokenKind::Comma) {
                if self.check(&TokenKind::RightBracket) {
                    return Err(JsError::syntax(
                        self.peek().offset,
                        "trailing comma is not valid JSON",
                    ));
                }

                continue;
            }

            self.consume(
                &TokenKind::RightBracket,
                "expected ',' or ']' after JSON array item",
            )?;

            return Ok(Value::Array(values));
        }
    }

    fn parse_object(&mut self) -> Result<Value, JsError> {
        let mut values = HashMap::new();

        if self.consume_optional(&TokenKind::RightBrace) {
            return Ok(Value::Object(values));
        }

        loop {
            let token = self.advance();

            let TokenKind::String(key) = token.kind else {
                return Err(JsError::syntax(
                    token.offset,
                    "JSON object keys must be strings",
                ));
            };

            self.consume(&TokenKind::Colon, "expected ':' after JSON object key")?;

            let value = self.parse_value()?;
            values.insert(key, value);

            if self.consume_optional(&TokenKind::Comma) {
                if self.check(&TokenKind::RightBrace) {
                    return Err(JsError::syntax(
                        self.peek().offset,
                        "trailing comma is not valid JSON",
                    ));
                }

                continue;
            }

            self.consume(
                &TokenKind::RightBrace,
                "expected ',' or '}' after JSON object property",
            )?;

            return Ok(Value::Object(values));
        }
    }

    fn consume(&mut self, expected: &TokenKind, message: &str) -> Result<(), JsError> {
        if self.check(expected) {
            self.advance();
            Ok(())
        } else {
            Err(JsError::syntax(self.peek().offset, message))
        }
    }

    fn consume_optional(&mut self, expected: &TokenKind) -> bool {
        if self.check(expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn check(&self, expected: &TokenKind) -> bool {
        self.peek().kind == *expected
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.position]
    }

    fn advance(&mut self) -> Token {
        let token = self.tokens[self.position].clone();

        if !matches!(token.kind, TokenKind::EndOfFile) {
            self.position += 1;
        }

        token
    }
}

struct Parser {
    tokens: Vec<Token>,
    position: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            position: 0,
        }
    }

    fn parse_program(&mut self) -> Result<Vec<Statement>, JsError> {
        let mut statements = Vec::new();

        while !self.check(&TokenKind::EndOfFile) {
            statements.push(self.parse_statement()?);
        }

        Ok(statements)
    }

    fn parse_statement(&mut self) -> Result<Statement, JsError> {
        if self.consume_optional(&TokenKind::For) {
            return self.parse_for_statement();
        }

        if self.consume_optional(&TokenKind::Break) {
            self.consume_optional(&TokenKind::Semicolon);
            return Ok(Statement::Break);
        }

        if self.consume_optional(&TokenKind::Continue) {
            self.consume_optional(&TokenKind::Semicolon);
            return Ok(Statement::Continue);
        }

        if self.consume_optional(&TokenKind::If) {
            return self.parse_if_statement();
        }

        if self.consume_optional(&TokenKind::While) {
            return self.parse_while_statement();
        }

        if self.consume_optional(&TokenKind::Return) {
            let value = self.parse_expression()?;
            self.consume_optional(&TokenKind::Semicolon);

            return Ok(Statement::Return(value));
        }

        if matches!(
            self.peek().kind,
            TokenKind::Let | TokenKind::Const | TokenKind::Var
        ) {
            self.advance();
            return self.parse_declaration();
        }

        let expression = self.parse_expression()?;
        self.consume_optional(&TokenKind::Semicolon);

        Ok(Statement::Expression(expression))
    }

    fn parse_for_statement(&mut self) -> Result<Statement, JsError> {
        self.consume(&TokenKind::LeftParen, "expected '(' after for")?;

        let initializer = if self.consume_optional(&TokenKind::Semicolon) {
            None
        } else if matches!(
            self.peek().kind,
            TokenKind::Let | TokenKind::Const | TokenKind::For
        ) {
            self.advance();
            Some(Box::new(self.parse_for_declaration()?))
        } else {
            let expression = self.parse_expression()?;

            self.consume(&TokenKind::Semicolon, "expected ';' after for initializer")?;

            Some(Box::new(Statement::Expression(expression)))
        };

        let condition = if self.consume_optional(&TokenKind::Semicolon) {
            None
        } else {
            let condition = self.parse_expression()?;

            self.consume(&TokenKind::Semicolon, "expected ';' after for condition")?;

            Some(condition)
        };

        let update = if self.check(&TokenKind::RightParen) {
            None
        } else {
            Some(self.parse_expression()?)
        };

        self.consume(&TokenKind::RightParen, "expected ')' after for clauses")?;

        let body = self.parse_statement_body()?;

        Ok(Statement::For {
            initializer,
            condition,
            update,
            body,
        })
    }

    fn parse_for_declaration(&mut self) -> Result<Statement, JsError> {
        let (name, offset) = self.consume_identifier("expected a variable name")?;

        let value = if self.consume_optional(&TokenKind::Assign) {
            self.parse_expression()?
        } else {
            Expression::Literal(Value::Undefined)
        };

        self.consume(&TokenKind::Semicolon, "expected ';' after for declaration")?;

        if name.is_empty() {
            return Err(JsError::syntax(offset, "variable name cannot be empty"));
        }

        Ok(Statement::Declaration { name, value })
    }

    fn parse_while_statement(&mut self) -> Result<Statement, JsError> {
        self.consume(&TokenKind::LeftParen, "expected '(' after while")?;

        let condition = self.parse_expression()?;

        self.consume(&TokenKind::RightParen, "expected ')' after while condition")?;

        let body = self.parse_statement_body()?;

        Ok(Statement::While { condition, body })
    }

    fn parse_if_statement(&mut self) -> Result<Statement, JsError> {
        self.consume(&TokenKind::LeftParen, "expected '(' after if")?;

        let condition = self.parse_expression()?;

        self.consume(&TokenKind::RightParen, "expected ')' after if condition")?;

        let then_body = self.parse_statement_body()?;

        let else_body = if self.consume_optional(&TokenKind::Else) {
            self.parse_statement_body()?
        } else {
            Vec::new()
        };

        Ok(Statement::If {
            condition,
            then_body,
            else_body,
        })
    }

    fn parse_statement_body(&mut self) -> Result<Vec<Statement>, JsError> {
        if !self.consume_optional(&TokenKind::LeftBrace) {
            return Ok(vec![self.parse_statement()?]);
        }

        let mut body = Vec::new();

        while !self.check(&TokenKind::RightBrace) {
            if self.check(&TokenKind::EndOfFile) {
                return Err(JsError::syntax(
                    self.peek().offset,
                    "unterminated statement block",
                ));
            }

            body.push(self.parse_statement()?);
        }

        self.advance();

        Ok(body)
    }

    fn parse_declaration(&mut self) -> Result<Statement, JsError> {
        let (name, offset) = self.consume_identifier("expected a variable name")?;

        let value = if self.consume_optional(&TokenKind::Assign) {
            self.parse_expression()?
        } else {
            Expression::Literal(Value::Undefined)
        };

        self.consume_optional(&TokenKind::Semicolon);

        if name.is_empty() {
            return Err(JsError::syntax(offset, "variable name cannot be empty"));
        }

        Ok(Statement::Declaration { name, value })
    }

    fn parse_expression(&mut self) -> Result<Expression, JsError> {
        self.parse_assignment_expression()
    }

    fn parse_assignment_expression(&mut self) -> Result<Expression, JsError> {
        if let TokenKind::Identifier(parameter) = self.peek().kind.clone() {
            let is_arrow = self
                .tokens
                .get(self.position + 1)
                .is_some_and(|token| matches!(token.kind, TokenKind::Arrow));

            if is_arrow {
                self.advance();
                self.advance();

                return Ok(Expression::Function(UserFunction {
                    parameters: vec![parameter],
                    body: self.parse_function_body()?,
                }));
            }
        }

        let expression = self.parse_logical_or_expression()?;

        if self.consume_optional(&TokenKind::Assign) {
            let value = self.parse_assignment_expression()?;

            return Ok(Expression::Assignment {
                target: Box::new(expression),
                value: Box::new(value),
            });
        }

        Ok(expression)
    }

    fn parse_logical_or_expression(&mut self) -> Result<Expression, JsError> {
        let mut expression = self.parse_logical_and_expression()?;

        while self.consume_optional(&TokenKind::Or) {
            let right = self.parse_logical_and_expression()?;

            expression = Expression::Binary {
                left: Box::new(expression),
                operator: BinaryOperator::Or,
                right: Box::new(right),
            };
        }

        Ok(expression)
    }

    fn parse_logical_and_expression(&mut self) -> Result<Expression, JsError> {
        let mut expression = self.parse_equality_expression()?;

        while self.consume_optional(&TokenKind::And) {
            let right = self.parse_equality_expression()?;

            expression = Expression::Binary {
                left: Box::new(expression),
                operator: BinaryOperator::And,
                right: Box::new(right),
            };
        }

        Ok(expression)
    }

    fn parse_equality_expression(&mut self) -> Result<Expression, JsError> {
        let mut expression = self.parse_comparison_expression()?;

        loop {
            let operator = if self.consume_optional(&TokenKind::StrictEqual) {
                BinaryOperator::StrictEqual
            } else if self.consume_optional(&TokenKind::StrictNotEqual) {
                BinaryOperator::StrictNotEqual
            } else if self.consume_optional(&TokenKind::Equal) {
                BinaryOperator::Equal
            } else if self.consume_optional(&TokenKind::NotEqual) {
                BinaryOperator::NotEqual
            } else {
                break;
            };

            let right = self.parse_comparison_expression()?;

            expression = Expression::Binary {
                left: Box::new(expression),
                operator,
                right: Box::new(right),
            };
        }

        Ok(expression)
    }

    fn parse_comparison_expression(&mut self) -> Result<Expression, JsError> {
        let mut expression = self.parse_term_expression()?;

        loop {
            let operator = if self.consume_optional(&TokenKind::LessEqual) {
                BinaryOperator::LessEqual
            } else if self.consume_optional(&TokenKind::GreaterEqual) {
                BinaryOperator::GreaterEqual
            } else if self.consume_optional(&TokenKind::Instanceof) {
                BinaryOperator::Instanceof
            } else if self.consume_optional(&TokenKind::Less) {
                BinaryOperator::Less
            } else if self.consume_optional(&TokenKind::Greater) {
                BinaryOperator::Greater
            } else {
                break;
            };

            let right = self.parse_term_expression()?;

            expression = Expression::Binary {
                left: Box::new(expression),
                operator,
                right: Box::new(right),
            };
        }

        Ok(expression)
    }

    fn parse_term_expression(&mut self) -> Result<Expression, JsError> {
        let mut expression = self.parse_factor_expression()?;

        loop {
            let operator = if self.consume_optional(&TokenKind::Plus) {
                BinaryOperator::Add
            } else if self.consume_optional(&TokenKind::Minus) {
                BinaryOperator::Subtract
            } else {
                break;
            };

            let right = self.parse_factor_expression()?;

            expression = Expression::Binary {
                left: Box::new(expression),
                operator,
                right: Box::new(right),
            };
        }

        Ok(expression)
    }

    fn parse_factor_expression(&mut self) -> Result<Expression, JsError> {
        let mut expression = self.parse_unary_expression()?;

        loop {
            let operator = if self.consume_optional(&TokenKind::Star) {
                BinaryOperator::Multiply
            } else if self.consume_optional(&TokenKind::Slash) {
                BinaryOperator::Divide
            } else {
                break;
            };

            let right = self.parse_unary_expression()?;

            expression = Expression::Binary {
                left: Box::new(expression),
                operator,
                right: Box::new(right),
            };
        }

        Ok(expression)
    }

    fn parse_unary_expression(&mut self) -> Result<Expression, JsError> {
        if self.consume_optional(&TokenKind::PlusPlus) {
            return Ok(Expression::Update {
                target: Box::new(self.parse_unary_expression()?),
                delta: 1.0,
                prefix: true,
            });
        }

        if self.consume_optional(&TokenKind::MinusMinus) {
            return Ok(Expression::Update {
                target: Box::new(self.parse_unary_expression()?),
                delta: -1.0,
                prefix: true,
            });
        }

        if self.consume_optional(&TokenKind::Not) {
            return Ok(Expression::Not(Box::new(self.parse_unary_expression()?)));
        }

        self.parse_postfix_expression()
    }

    fn parse_postfix_expression(&mut self) -> Result<Expression, JsError> {
        let mut expression = self.parse_primary()?;

        loop {
            if self.consume_optional(&TokenKind::Dot) {
                let (property, _) =
                    self.consume_identifier("expected a property name after '.'")?;

                expression = Expression::Member {
                    object: Box::new(expression),
                    property,
                };

                continue;
            }

            if self.consume_optional(&TokenKind::LeftBracket) {
                let index = self.parse_expression()?;

                self.consume(
                    &TokenKind::RightBracket,
                    "expected ']' after index expression",
                )?;

                expression = Expression::Index {
                    object: Box::new(expression),
                    index: Box::new(index),
                };

                continue;
            }

            if self.consume_optional(&TokenKind::LeftParen) {
                let arguments = self.parse_arguments()?;

                expression = Expression::Call {
                    callee: Box::new(expression),
                    arguments,
                };

                continue;
            }

            if self.consume_optional(&TokenKind::PlusPlus) {
                expression = Expression::Update {
                    target: Box::new(expression),
                    delta: 1.0,
                    prefix: false,
                };

                continue;
            }

            if self.consume_optional(&TokenKind::MinusMinus) {
                expression = Expression::Update {
                    target: Box::new(expression),
                    delta: -1.0,
                    prefix: false,
                };

                continue;
            }

            break;
        }

        Ok(expression)
    }

    fn parse_primary(&mut self) -> Result<Expression, JsError> {
        let token = self.advance();

        match token.kind {
            TokenKind::String(value) => Ok(Expression::Literal(Value::String(value))),
            TokenKind::Number(value) => Ok(Expression::Literal(Value::Number(value))),
            TokenKind::True => Ok(Expression::Literal(Value::Boolean(true))),
            TokenKind::False => Ok(Expression::Literal(Value::Boolean(false))),
            TokenKind::Null => Ok(Expression::Literal(Value::Null)),

            TokenKind::Identifier(name) => Ok(Expression::Identifier(name)),

            TokenKind::New => self.parse_new_expression(token.offset),

            TokenKind::EndOfFile => Err(JsError::syntax(
                token.offset,
                "unexpected end of JavaScript source",
            )),

            TokenKind::Function => self.parse_function_expression(),

            TokenKind::LeftParen => {
                if self.consume_optional(&TokenKind::RightParen) {
                    if self.consume_optional(&TokenKind::Arrow) {
                        return Ok(Expression::Function(UserFunction {
                            parameters: Vec::new(),
                            body: self.parse_function_body()?,
                        }));
                    }

                    return Err(JsError::syntax(
                        token.offset,
                        "expected an expression inside parentheses",
                    ));
                }

                if let TokenKind::Identifier(parameter) = self.peek().kind.clone() {
                    let has_closing_parenthesis = self
                        .tokens
                        .get(self.position + 1)
                        .is_some_and(|token| matches!(token.kind, TokenKind::RightParen));

                    let has_arrow = self
                        .tokens
                        .get(self.position + 2)
                        .is_some_and(|token| matches!(token.kind, TokenKind::Arrow));

                    if has_closing_parenthesis && has_arrow {
                        self.advance();
                        self.advance();
                        self.advance();

                        return Ok(Expression::Function(UserFunction {
                            parameters: vec![parameter],
                            body: self.parse_function_body()?,
                        }));
                    }
                }

                let expression = self.parse_expression()?;
                self.consume(&TokenKind::RightParen, "expected ')' after expression")?;

                Ok(expression)
            }

            _ => Err(JsError::syntax(token.offset, "expected an expression")),
        }
    }

    fn parse_function_expression(&mut self) -> Result<Expression, JsError> {
        self.consume(&TokenKind::LeftParen, "expected '(' after function keyword")?;

        let parameters = self.parse_function_parameters()?;
        let body = self.parse_function_body()?;

        Ok(Expression::Function(UserFunction { parameters, body }))
    }

    fn parse_function_parameters(&mut self) -> Result<Vec<String>, JsError> {
        let mut parameters = Vec::new();

        if self.consume_optional(&TokenKind::RightParen) {
            return Ok(parameters);
        }

        loop {
            let (parameter, _) = self.consume_identifier("expected a function identifier")?;

            parameters.push(parameter);

            if self.consume_optional(&TokenKind::Comma) {
                continue;
            }

            self.consume(
                &TokenKind::RightParen,
                "expected ',' or ')' after function parameter",
            )?;

            return Ok(parameters);
        }
    }

    fn parse_function_body(&mut self) -> Result<Vec<Statement>, JsError> {
        self.consume(&TokenKind::LeftBrace, "expected '{' before function body")?;

        let mut body = Vec::new();

        while !self.check(&TokenKind::RightBrace) {
            if self.check(&TokenKind::EndOfFile) {
                return Err(JsError::syntax(
                    self.peek().offset,
                    "unterminated function body",
                ));
            }

            body.push(self.parse_statement()?);
        }

        self.advance();

        Ok(body)
    }

    fn parse_new_expression(&mut self, offset: usize) -> Result<Expression, JsError> {
        let (constructor, _) = self.consume_identifier("expected a constructor after 'new'")?;

        self.consume(&TokenKind::LeftParen, "expected '(' after constructor name")?;

        let arguments = self.parse_arguments()?;

        if constructor.is_empty() {
            return Err(JsError::syntax(offset, "constructor name cannot be empty"));
        }

        Ok(Expression::New {
            constructor,
            arguments,
        })
    }

    fn parse_arguments(&mut self) -> Result<Vec<Expression>, JsError> {
        let mut arguments = Vec::new();

        if self.consume_optional(&TokenKind::RightParen) {
            return Ok(arguments);
        }

        loop {
            arguments.push(self.parse_expression()?);

            if self.consume_optional(&TokenKind::Comma) {
                if self.consume_optional(&TokenKind::RightParen) {
                    return Ok(arguments);
                }

                continue;
            }

            self.consume(&TokenKind::RightParen, "expected ',' or ')' after argument")?;

            return Ok(arguments);
        }
    }

    fn consume_identifier(&mut self, message: &str) -> Result<(String, usize), JsError> {
        let token = self.advance();

        match token.kind {
            TokenKind::Identifier(name) => Ok((name, token.offset)),
            _ => Err(JsError::syntax(token.offset, message)),
        }
    }

    fn consume(&mut self, expected: &TokenKind, message: &str) -> Result<(), JsError> {
        if self.check(expected) {
            self.advance();
            Ok(())
        } else {
            Err(JsError::syntax(self.peek().offset, message))
        }
    }

    fn consume_optional(&mut self, expected: &TokenKind) -> bool {
        if self.check(expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn check(&self, expected: &TokenKind) -> bool {
        self.peek().kind == *expected
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.position]
    }

    fn advance(&mut self) -> Token {
        let token = self.tokens[self.position].clone();

        if !matches!(token.kind, TokenKind::EndOfFile) {
            self.position += 1;
        }

        token
    }
}

struct Interpreter {
    variables: HashMap<String, Value>,
    cleanups: Vec<Cleanup>,
    function_locals: Vec<Vec<String>>,
}

impl Interpreter {
    fn new() -> Self {
        let mut variables = HashMap::new();

        variables.insert("undefined".to_owned(), Value::Undefined);

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

        Self {
            variables,
            cleanups: Vec::new(),
            function_locals: Vec::new(),
        }
    }

    fn execute(&mut self, program: &[Statement]) -> Result<Completion, JsError> {
        let mut result = Value::Undefined;

        for statement in program {
            result = match statement {
                Statement::Declaration { name, value } => {
                    let value = self.evaluate_expression(value)?;
                    self.variables.insert(name.clone(), value);
                    Value::Undefined
                }

                Statement::Expression(expression) => self.evaluate_expression(expression)?,

                Statement::Return(expression) => {
                    return Ok(Completion::Return(self.evaluate_expression(expression)?));
                }

                Statement::If {
                    condition,
                    then_body,
                    else_body,
                } => {
                    let condition = self.evaluate_expression(condition)?;

                    let branch = if self.is_truthy(&condition) {
                        then_body
                    } else {
                        else_body
                    };

                    match self.execute(branch)? {
                        Completion::Normal(value) => value,
                        Completion::Return(value) => {
                            return Ok(Completion::Return(value));
                        }
                        Completion::Break => {
                            return Ok(Completion::Break);
                        }
                        Completion::Continue => return Ok(Completion::Continue),
                    }
                }

                Statement::While { condition, body } => {
                    let mut last_value = Value::Undefined;

                    loop {
                        let condition = self.evaluate_expression(condition)?;

                        if !self.is_truthy(&condition) {
                            break;
                        }

                        match self.execute(body)? {
                            Completion::Normal(value) => last_value = value,
                            Completion::Return(value) => {
                                return Ok(Completion::Return(value));
                            }
                            Completion::Break => break,
                            Completion::Continue => continue,
                        }
                    }

                    last_value
                }

                Statement::Break => {
                    return Ok(Completion::Break);
                }

                Statement::Continue => {
                    return Ok(Completion::Continue);
                }

                Statement::For {
                    initializer,
                    condition,
                    update,
                    body,
                } => {
                    if let Some(initializer) = initializer {
                        match self.execute(std::slice::from_ref(initializer.as_ref()))? {
                            Completion::Normal(_) => {}

                            Completion::Return(value) => {
                                return Ok(Completion::Return(value));
                            }

                            Completion::Break => {
                                return Ok(Completion::Break);
                            }

                            Completion::Continue => {
                                return Ok(Completion::Continue);
                            }
                        }
                    }

                    loop {
                        if let Some(condition) = condition {
                            let value = self.evaluate_expression(condition)?;

                            if !self.is_truthy(&value) {
                                break;
                            }
                        }

                        match self.execute(body)? {
                            Completion::Normal(value) => result = value,
                            Completion::Return(value) => return Ok(Completion::Return(value)),
                            Completion::Break => break,
                            Completion::Continue => {}
                        }

                        if let Some(update) = update {
                            self.evaluate_expression(update)?;
                        }
                    }

                    Value::Undefined
                }
            };
        }

        Ok(Completion::Normal(result))
    }

    fn evaluate_expression(&mut self, expression: &Expression) -> Result<Value, JsError> {
        match expression {
            Expression::Literal(value) => Ok(value.clone()),

            Expression::Identifier(name) => {
                self.variables.get(name).cloned().ok_or_else(|| {
                    JsError::runtime(format!("ReferenceError: ${name} is not defined"))
                })
            }

            Expression::New {
                constructor,
                arguments,
            } => self.construct(constructor, arguments),

            Expression::Member { object, property } => {
                let object = self.evaluate_expression(object)?;

                match (object, property.as_str()) {
                    (Value::TestContext, "add_cleanup") => {
                        Ok(Value::TestContextMethod(TestContextMethod::AddCleanup))
                    }

                    (Value::Url(url), "href") => Ok(Value::String(url.to_string())),

                    (Value::Url(url), "protocol") => {
                        Ok(Value::String(format!("{}:", url.scheme())))
                    }

                    (Value::Document(document), "title") => Ok(Value::String(document.title)),

                    (Value::Document(document), "body") => Ok(document
                        .body_node()
                        .map(Value::Element)
                        .unwrap_or(Value::Null)),

                    (Value::Document(document), "getElementById") => Ok(Value::DocumentMethod(
                        DocumentMethod::GetElementById(document),
                    )),

                    (Value::Document(_), "getElementsByTagName") => {
                        Ok(Value::DocumentMethod(DocumentMethod::GetElementsByTagName))
                    }

                    (Value::Document(_), "createElement") => {
                        Ok(Value::DocumentMethod(DocumentMethod::CreateElement))
                    }

                    (Value::HtmlCollection(tag_name), "length") => {
                        let length = self
                            .current_document()
                            .map(|document| document.get_elements_by_tag_name(&tag_name).len())
                            .unwrap_or(0);

                        Ok(Value::Number(length as f64))
                    }

                    (Value::Element(node_id), property) => {
                        let Some(element) = self.current_document_element(node_id) else {
                            return Err(JsError::runtime(
                                "TypeError: element is no longer available",
                            ));
                        };

                        match property {
                            "id" => Ok(Value::String(element.id.clone())),

                            "tagName" => Ok(Value::String(element.tag_name.to_ascii_uppercase())),

                            "textContent" => Ok(Value::String(element.text_content.clone())),

                            "parentNode" => {
                                Ok(element.parent.map(Value::Element).unwrap_or(Value::Null))
                            }

                            "attributes" => Ok(Value::Attributes(node_id)),

                            "getAttribute" => {
                                Ok(Value::ElementMethod(ElementMethod::GetAttribute(node_id)))
                            }

                            "setAttribute" => {
                                Ok(Value::ElementMethod(ElementMethod::SetAttribute(node_id)))
                            }

                            "removeAttribute" => Ok(Value::ElementMethod(
                                ElementMethod::RemoveAttribute(node_id),
                            )),

                            "appendChild" => {
                                Ok(Value::ElementMethod(ElementMethod::AppendChild(node_id)))
                            }

                            "removeChild" => {
                                Ok(Value::ElementMethod(ElementMethod::RemoveChild(node_id)))
                            }

                            "insertBefore" => {
                                Ok(Value::ElementMethod(ElementMethod::InsertBefore(node_id)))
                            }

                            property => Err(JsError::runtime(format!(
                                "Element.{property} is not implemented yet"
                            ))),
                        }
                    }

                    (Value::Attribute { node_id, name }, "value") => {
                        let Some(element) = self.current_document_element(node_id) else {
                            return Err(JsError::runtime(
                                "TypeError: attribute owner is no longer available",
                            ));
                        };

                        let Some(attribute) = element
                            .attributes
                            .iter()
                            .find(|attribute| attribute.name == name)
                        else {
                            return Err(JsError::runtime(
                                "TypeError: attribute is no longer available",
                            ));
                        };

                        Ok(Value::String(attribute.value.clone()))
                    }

                    (Value::Url(_), property) => Err(JsError::runtime(format!(
                        "URL.{property} is not implemented yet"
                    ))),

                    (Value::Document(_), property) => Err(JsError::runtime(format!(
                        "Document.{property} is not implemented yet"
                    ))),

                    (_, property) => Err(JsError::runtime(format!(
                        "cannot read property {property:?} from this value"
                    ))),
                }
            }

            Expression::Index { object, index } => {
                let object = self.evaluate_expression(object)?;
                let index = self.evaluate_expression(index)?;

                self.index_value(object, index)
            }

            Expression::Function(function) => Ok(Value::Function(function.clone())),

            Expression::Not(expression) => {
                let value = self.evaluate_expression(expression)?;
                Ok(Value::Boolean(!self.is_truthy(&value)))
            }

            Expression::Assignment { target, value } => {
                let value = self.evaluate_expression(value)?;
                self.assign(target, value.clone())?;
                Ok(value)
            }

            Expression::Call { callee, arguments } => self.evaluate_call(callee, arguments),

            Expression::Binary {
                left,
                operator: BinaryOperator::And,
                right,
            } => {
                let left = self.evaluate_expression(left)?;

                if self.is_truthy(&left) {
                    self.evaluate_expression(right)
                } else {
                    Ok(left)
                }
            }

            Expression::Binary {
                left,
                operator: BinaryOperator::Or,
                right,
            } => {
                let left = self.evaluate_expression(left)?;

                if self.is_truthy(&left) {
                    Ok(left)
                } else {
                    self.evaluate_expression(right)
                }
            }

            Expression::Binary {
                left,
                operator,
                right,
            } => {
                let left = self.evaluate_expression(left)?;
                let right = self.evaluate_expression(right)?;

                self.evaluate_binary(left, *operator, right)
            }

            Expression::Update {
                target,
                delta,
                prefix,
            } => self.evaluate_update(target, *delta, *prefix),
        }
    }

    fn index_value(&self, object: Value, index: Value) -> Result<Value, JsError> {
        let Some(index) = self.to_number(&index) else {
            return Err(JsError::runtime(
                "TypeError: collection index must be numeric",
            ));
        };

        if !index.is_finite() || index < 0.0 || index.fract() != 0.0 {
            return Ok(Value::Undefined);
        }

        match object {
            Value::Array(values) => Ok(values
                .get(index as usize)
                .cloned()
                .unwrap_or(Value::Undefined)),

            Value::Attributes(node_id) => Ok(self
                .current_document_element(node_id)
                .and_then(|element| element.attributes.get(index as usize))
                .map(|attribute| Value::Attribute {
                    node_id,
                    name: attribute.name.clone(),
                })
                .unwrap_or(Value::Undefined)),

            Value::HtmlCollection(tag_name) => Ok(self
                .current_document()
                .and_then(|document| {
                    document
                        .get_elements_by_tag_name(&tag_name)
                        .get(index as usize)
                        .copied()
                })
                .map(Value::Element)
                .unwrap_or(Value::Undefined)),

            _ => Err(JsError::runtime("TypeError: this value cannot be indexed")),
        }
    }

    fn evaluate_binary(
        &self,
        left: Value,
        operator: BinaryOperator,
        right: Value,
    ) -> Result<Value, JsError> {
        match operator {
            BinaryOperator::StrictEqual => Ok(Value::Boolean(left == right)),
            BinaryOperator::StrictNotEqual => Ok(Value::Boolean(left != right)),

            BinaryOperator::Equal => Ok(Value::Boolean(self.loosely_equals(&left, &right))),
            BinaryOperator::NotEqual => Ok(Value::Boolean(!self.loosely_equals(&left, &right))),

            BinaryOperator::And | BinaryOperator::Or => {
                unreachable!("logical operators are evaluated with short-circuiting")
            }

            BinaryOperator::Add => {
                if matches!(&left, Value::String(_)) || matches!(&right, Value::String(_)) {
                    return Ok(Value::String(format!(
                        "{}{}",
                        left.to_js_string(),
                        right.to_js_string()
                    )));
                }

                let (left, right) = self.numeric_operands(&left, &right)?;
                Ok(Value::Number(left + right))
            }

            BinaryOperator::Subtract => {
                let (left, right) = self.numeric_operands(&left, &right)?;
                Ok(Value::Number(left - right))
            }

            BinaryOperator::Multiply => {
                let (left, right) = self.numeric_operands(&left, &right)?;
                Ok(Value::Number(left * right))
            }

            BinaryOperator::Divide => {
                let (left, right) = self.numeric_operands(&left, &right)?;
                Ok(Value::Number(left / right))
            }

            BinaryOperator::Less => Ok(Value::Boolean(self.compare_values(
                &left,
                &right,
                |a, b| a < b,
            )?)),

            BinaryOperator::LessEqual => Ok(Value::Boolean(self.compare_values(
                &left,
                &right,
                |a, b| a <= b,
            )?)),

            BinaryOperator::Greater => Ok(Value::Boolean(self.compare_values(
                &left,
                &right,
                |a, b| a > b,
            )?)),

            BinaryOperator::GreaterEqual => Ok(Value::Boolean(self.compare_values(
                &left,
                &right,
                |a, b| a >= b,
            )?)),

            BinaryOperator::Instanceof => {
                let matches = match (&left, &right) {
                    (Value::Element(node_id), Value::ElementConstructor(tag)) => self
                        .current_document_element(*node_id)
                        .map(|element| element.tag_name == *tag)
                        .unwrap_or(false),

                    _ => false,
                };

                Ok(Value::Boolean(matches))
            }
        }
    }

    fn loosely_equals(&self, left: &Value, right: &Value) -> bool {
        match (left, right) {
            (Value::Undefined, Value::Null) | (Value::Null, Value::Undefined) => true,

            (Value::String(_), Value::Number(_)) | (Value::Number(_), Value::String(_)) => {
                self.to_number(left) == self.to_number(right)
            }

            _ => left == right,
        }
    }

    fn numeric_operands(&self, left: &Value, right: &Value) -> Result<(f64, f64), JsError> {
        let Some(left) = self.to_number(left) else {
            return Err(JsError::runtime(
                "TypeError: operator requires numeric operands",
            ));
        };

        let Some(right) = self.to_number(right) else {
            return Err(JsError::runtime(
                "TypeError: operator requires numeric operands",
            ));
        };

        Ok((left, right))
    }

    fn is_truthy(&self, value: &Value) -> bool {
        match value {
            Value::Undefined | Value::Null | Value::Boolean(false) => false,
            Value::Number(value) => *value != 0.0 && !value.is_nan(),
            Value::String(value) => !value.is_empty(),
            _ => true,
        }
    }

    fn to_number(&self, value: &Value) -> Option<f64> {
        match value {
            Value::Number(value) => Some(*value),
            Value::Boolean(true) => Some(1.0),
            Value::Boolean(false) | Value::Null => Some(0.0),

            Value::String(value) => {
                let value = value.trim();

                if value.is_empty() {
                    Some(0.0)
                } else {
                    value.parse::<f64>().ok()
                }
            }

            Value::Undefined
            | Value::Array(_)
            | Value::Object(_)
            | Value::Url(_)
            | Value::NativeFunction(_)
            | Value::Document(_)
            | Value::Element(_)
            | Value::HtmlCollection(_)
            | Value::Attributes(_)
            | Value::Attribute { .. }
            | Value::DocumentMethod(_)
            | Value::Function(_)
            | Value::ElementConstructor(_)
            | Value::TestContextMethod(_)
            | Value::ElementMethod(_)
            | Value::TestContext => None,
        }
    }

    fn compare_values(
        &self,
        left: &Value,
        right: &Value,
        comparison: impl FnOnce(f64, f64) -> bool,
    ) -> Result<bool, JsError> {
        let (left, right) = self.numeric_operands(left, right)?;
        Ok(comparison(left, right))
    }

    fn current_document(&self) -> Option<&DocumentState> {
        let Value::Document(document) = self.variables.get("document")? else {
            return None;
        };

        Some(document)
    }

    fn current_document_element(&self, node_id: u64) -> Option<&ElementState> {
        self.current_document()?.element(node_id)
    }

    fn current_document_mut(&mut self) -> Result<&mut DocumentState, JsError> {
        let Some(Value::Document(document)) = self.variables.get_mut("document") else {
            return Err(JsError::runtime(
                "TypeError: document binding is unavailable",
            ));
        };

        Ok(document)
    }

    fn set_document_element_text_content(
        &mut self,
        node_id: u64,
        text_content: String,
    ) -> Result<(), JsError> {
        if self
            .current_document_mut()?
            .set_node_text_content(node_id, text_content)
        {
            Ok(())
        } else {
            Err(JsError::runtime(
                "TypeError: element is no longer available",
            ))
        }
    }

    fn set_document_element_inner_html(
        &mut self,
        node_id: u64,
        html: String,
    ) -> Result<(), JsError> {
        if self
            .current_document_mut()?
            .set_node_inner_html(node_id, &html)
        {
            Ok(())
        } else {
            Err(JsError::runtime(
                "TypeError: element is no longer available",
            ))
        }
    }

    fn set_document_element_outer_html(
        &mut self,
        node_id: u64,
        html: String,
    ) -> Result<(), JsError> {
        if self
            .current_document_mut()?
            .set_node_outer_html(node_id, &html)
        {
            Ok(())
        } else {
            Err(JsError::runtime(
                "TypeError: element is no longer available",
            ))
        }
    }

    fn set_document_element_id(&mut self, node_id: u64, id: String) -> Result<(), JsError> {
        if self
            .current_document_mut()?
            .set_node_attribute(node_id, "id", id)
        {
            Ok(())
        } else {
            Err(JsError::runtime(
                "TypeError: element is no longer available",
            ))
        }
    }

    fn evaluate_update(
        &mut self,
        target: &Expression,
        delta: f64,
        prefix: bool,
    ) -> Result<Value, JsError> {
        let previous = self.evaluate_expression(target)?;

        let Some(number) = self.to_number(&previous) else {
            return Err(JsError::runtime(
                "TypeError: update operator requires a numeric value",
            ));
        };

        let updated = Value::Number(number + delta);

        self.assign(target, updated.clone())?;

        if prefix { Ok(updated) } else { Ok(previous) }
    }

    fn assign(&mut self, target: &Expression, value: Value) -> Result<(), JsError> {
        match target {
            Expression::Identifier(name) => {
                self.variables.insert(name.clone(), value);
                Ok(())
            }

            Expression::Member { object, property } => {
                if let Expression::Identifier(name) = object.as_ref() {
                    if name == "document" && property == "title" {
                        let Some(Value::Document(document)) = self.variables.get_mut("document")
                        else {
                            return Err(JsError::runtime(
                                "TypeError: document binding is unavailable",
                            ));
                        };

                        document.title = value.to_js_string();
                        return Ok(());
                    }
                }

                let object = self.evaluate_expression(object)?;

                match object {
                    Value::Element(node_id) if property == "textContent" => {
                        self.set_document_element_text_content(node_id, value.to_js_string())
                    }

                    Value::Element(node_id) if property == "innerHTML" => {
                        self.set_document_element_inner_html(node_id, value.to_js_string())
                    }

                    Value::Element(node_id) if property == "outerHTML" => {
                        self.set_document_element_outer_html(node_id, value.to_js_string())
                    }

                    Value::Element(node_id) if property == "id" => {
                        self.set_document_element_id(node_id, value.to_js_string())
                    }

                    Value::Attribute { node_id, name } if property == "value" => {
                        let serialized_value = value.to_js_string();

                        if self.current_document_mut()?.set_node_attribute(
                            node_id,
                            &name,
                            serialized_value,
                        ) {
                            Ok(())
                        } else {
                            Err(JsError::runtime(
                                "TypeError: attribute owner is no longer available",
                            ))
                        }
                    }

                    Value::Element(_) => Err(JsError::runtime(format!(
                        "Element.{property} is not implemented yet"
                    ))),

                    _ => Err(JsError::runtime(
                        "TypeError: assignment target is not supported",
                    )),
                }
            }

            _ => Err(JsError::runtime(
                "TypeError: assignment target is not supported",
            )),
        }
    }

    fn evaluate_call(
        &mut self,
        callee: &Expression,
        arguments: &[Expression],
    ) -> Result<Value, JsError> {
        let callee = self.evaluate_expression(callee)?;

        let mut values = Vec::with_capacity(arguments.len());

        for argument in arguments {
            values.push(self.evaluate_expression(argument)?);
        }

        match callee {
            Value::NativeFunction(NativeFunction::AssertEquals) => self.assert_equals(values),

            Value::NativeFunction(NativeFunction::Test) => self.run_test(values),

            Value::NativeFunction(NativeFunction::AssertTrue) => self.assert_true(values),

            Value::NativeFunction(NativeFunction::AssertFalse) => self.assert_false(values),

            Value::NativeFunction(NativeFunction::AssertNotEquals) => {
                self.assert_not_equals(values)
            }

            Value::DocumentMethod(DocumentMethod::GetElementById(document)) => {
                self.get_element_by_id(document, values)
            }

            Value::DocumentMethod(DocumentMethod::CreateElement) => {
                self.create_document_element(values)
            }

            Value::DocumentMethod(DocumentMethod::GetElementsByTagName) => {
                self.get_elements_by_tag_name(values)
            }

            Value::ElementMethod(ElementMethod::GetAttribute(node_id)) => {
                self.get_document_element_attribute(node_id, values)
            }

            Value::ElementMethod(ElementMethod::SetAttribute(node_id)) => {
                self.set_document_element_attribute(node_id, values)
            }

            Value::ElementMethod(ElementMethod::RemoveAttribute(node_id)) => {
                self.remove_document_element_attribute(node_id, values)
            }

            Value::ElementMethod(ElementMethod::AppendChild(parent_node_id)) => {
                self.append_document_child(parent_node_id, values)
            }

            Value::ElementMethod(ElementMethod::InsertBefore(parent_node_id)) => {
                self.insert_document_child_before(parent_node_id, values)
            }

            Value::ElementMethod(ElementMethod::RemoveChild(parent_node_id)) => {
                self.remove_document_child(parent_node_id, values)
            }

            Value::TestContextMethod(TestContextMethod::AddCleanup) => self.add_cleanup(values),

            Value::Function(function) => self.call_function(&function, values),

            _ => Err(JsError::runtime("TypeError: this value is not callable")),
        }
    }

    fn run_test(&mut self, arguments: Vec<Value>) -> Result<Value, JsError> {
        if !(1..=2).contains(&arguments.len()) {
            return Err(JsError::runtime(
                "TypeError: test expects a callback and an optional description",
            ));
        }

        let Value::Function(callback) = &arguments[0] else {
            return Err(JsError::runtime(
                "TypeError: test expects a function callback",
            ));
        };

        if callback.parameters.len() > 1 {
            return Err(JsError::runtime(
                "TypeError: test callbacks may accept at most one parameter",
            ));
        }

        let callback_arguments = if callback.parameters.is_empty() {
            Vec::new()
        } else {
            vec![Value::TestContext]
        };

        let cleanup_start = self.cleanups.len();
        let previous_this = self.variables.insert("this".to_owned(), Value::TestContext);

        let test_result = self.call_function(callback, callback_arguments);

        if let Some(previous_this) = previous_this {
            self.variables.insert("this".to_owned(), previous_this);
        } else {
            self.variables.remove("this");
        }

        let cleanup_result = self.run_cleanups_since(cleanup_start);

        test_result?;
        cleanup_result?;

        Ok(Value::Undefined)
    }

    fn add_cleanup(&mut self, arguments: Vec<Value>) -> Result<Value, JsError> {
        if arguments.len() != 1 {
            return Err(JsError::runtime(
                "TypeError: add_cleanup expects one callback",
            ));
        }

        let Value::Function(callback) = arguments.into_iter().next().unwrap() else {
            return Err(JsError::runtime(
                "TypeError: add_cleanup expects a function callback",
            ));
        };

        if !callback.parameters.is_empty() {
            return Err(JsError::runtime(
                "TypeError: cleanup callbacks cannot accept parameters",
            ));
        }

        let mut captured = HashMap::new();

        for scope in &self.function_locals {
            for name in scope {
                if let Some(value) = self.variables.get(name) {
                    captured.insert(name.clone(), value.clone());
                }
            }
        }

        self.cleanups.push(Cleanup { callback, captured });

        Ok(Value::Undefined)
    }

    fn run_cleanups_since(&mut self, cleanup_start: usize) -> Result<(), JsError> {
        let mut first_error = None;

        while self.cleanups.len() > cleanup_start {
            let cleanup = self.cleanups.pop().unwrap();
            let mut replaced_bindings = Vec::new();

            for (name, value) in cleanup.captured {
                let previous = self.variables.insert(name.clone(), value);
                replaced_bindings.push((name, previous));
            }

            let result = self.call_function(&cleanup.callback, Vec::new());

            for (name, previous) in replaced_bindings.into_iter().rev() {
                if let Some(value) = previous {
                    self.variables.insert(name, value);
                } else {
                    self.variables.remove(&name);
                }
            }

            if let Err(error) = result {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }

        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn call_function(
        &mut self,
        function: &UserFunction,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        if function.parameters.len() != arguments.len() {
            return Err(JsError::runtime(format!(
                "TypeError: function expects {} arguments, got {}",
                function.parameters.len(),
                arguments.len(),
            )));
        }

        let mut replaced_bindings = Vec::new();
        let mut local_names = Vec::new();

        for (parameter, argument) in function.parameters.iter().zip(arguments) {
            let previous = self.variables.insert(parameter.clone(), argument);
            replaced_bindings.push((parameter.clone(), previous));
            local_names.push(parameter.clone());
        }

        for name in function_local_names(&function.body) {
            if local_names.iter().any(|local_name| local_name == &name) {
                continue;
            }

            let previous = self.variables.insert(name.clone(), Value::Undefined);
            replaced_bindings.push((name.clone(), previous));
            local_names.push(name.clone());
        }

        self.function_locals.push(local_names);

        let result = self.execute(&function.body);

        self.function_locals.pop();

        for (name, previous) in replaced_bindings.into_iter().rev() {
            if let Some(value) = previous {
                self.variables.insert(name, value);
            } else {
                self.variables.remove(&name);
            }
        }

        result.map(Completion::into_value)
    }

    fn get_element_by_id(
        &mut self,
        document: DocumentState,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        if arguments.len() != 1 {
            return Err(JsError::runtime(
                "TypeError: document.getElementById expects one argument",
            ));
        }

        let id = arguments[0].to_js_string();

        Ok(document
            .get_element_by_id(&id)
            .map(Value::Element)
            .unwrap_or(Value::Null))
    }

    fn get_elements_by_tag_name(&mut self, arguments: Vec<Value>) -> Result<Value, JsError> {
        if arguments.len() != 1 {
            return Err(JsError::runtime(
                "TypeError: document.getElementsByTagName expects one argument",
            ));
        }

        Ok(Value::HtmlCollection(arguments[0].to_js_string()))
    }

    fn create_document_element(&mut self, arguments: Vec<Value>) -> Result<Value, JsError> {
        if arguments.len() != 1 {
            return Err(JsError::runtime(
                "TypeError: document.createElement expects one argument",
            ));
        }

        let tag_name = arguments[0].to_js_string().to_ascii_lowercase();

        if tag_name.is_empty() {
            return Err(JsError::runtime(
                "TypeError: document.createElement requires a tag name",
            ));
        }

        let node_id = self.current_document_mut()?.create_element(&tag_name);

        Ok(Value::Element(node_id))
    }

    fn get_document_element_attribute(
        &self,
        node_id: u64,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        if arguments.len() != 1 {
            return Err(JsError::runtime(
                "TypeError: Element.getAttribute expects one argument",
            ));
        }

        let name = arguments[0].to_js_string().to_ascii_lowercase();

        let Some(element) = self.current_document_element(node_id) else {
            return Err(JsError::runtime(
                "TypeError: element is no longer available",
            ));
        };

        Ok(element
            .attributes
            .iter()
            .find(|attribute| attribute.name == name)
            .map(|attribute| Value::String(attribute.value.clone()))
            .unwrap_or(Value::Null))
    }

    fn set_document_element_attribute(
        &mut self,
        node_id: u64,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        if arguments.len() != 2 {
            return Err(JsError::runtime(
                "TypeError: Element.setAttribute expects two arguments",
            ));
        }

        let name = arguments[0].to_js_string().to_ascii_lowercase();
        let value = arguments[1].to_js_string();

        if self
            .current_document_mut()?
            .set_node_attribute(node_id, &name, value)
        {
            Ok(Value::Undefined)
        } else {
            Err(JsError::runtime(
                "TypeError: element is no longer available",
            ))
        }
    }

    fn remove_document_element_attribute(
        &mut self,
        node_id: u64,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        if arguments.len() != 1 {
            return Err(JsError::runtime(
                "TypeError: Element.removeAttribute expects one argument",
            ));
        }

        let name = arguments[0].to_js_string().to_ascii_lowercase();

        if self
            .current_document_mut()?
            .remove_node_attribute(node_id, &name)
        {
            Ok(Value::Undefined)
        } else {
            Err(JsError::runtime(
                "TypeError: element is no longer available",
            ))
        }
    }

    fn append_document_child(
        &mut self,
        parent_node_id: u64,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        if arguments.len() != 1 {
            return Err(JsError::runtime(
                "TypeError: Element.appendChild expects one argument",
            ));
        }

        let Value::Element(child_node_id) = arguments[0] else {
            return Err(JsError::runtime(
                "TypeError: Element.appendChild expects an Element",
            ));
        };

        if self
            .current_document_mut()?
            .append_child(parent_node_id, child_node_id)
        {
            Ok(Value::Element(child_node_id))
        } else {
            Err(JsError::runtime("TypeError: cannot append this element"))
        }
    }

    fn insert_document_child_before(
        &mut self,
        parent_node_id: u64,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        if arguments.len() != 2 {
            return Err(JsError::runtime(
                "TypeError: Element.insertBefore expects two arguments",
            ));
        }

        let Value::Element(child_node_id) = arguments[0] else {
            return Err(JsError::runtime(
                "TypeError: Element.insertBefore expects an Element to insert",
            ));
        };

        let Value::Element(reference_node_id) = arguments[1] else {
            return Err(JsError::runtime(
                "TypeError: Element.insertBefore expects an Element reference",
            ));
        };

        if self.current_document_mut()?.insert_before(
            parent_node_id,
            child_node_id,
            reference_node_id,
        ) {
            Ok(Value::Element(child_node_id))
        } else {
            Err(JsError::runtime(
                "TypeError: cannot insert this element before the reference node",
            ))
        }
    }

    fn remove_document_child(
        &mut self,
        parent_node_id: u64,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        if arguments.len() != 1 {
            return Err(JsError::runtime(
                "TypeError: Element.removeChild expects one argument",
            ));
        }

        let Value::Element(child_node_id) = arguments[0] else {
            return Err(JsError::runtime(
                "TypeError: Element.removeChild expects an Element",
            ));
        };

        if self
            .current_document_mut()?
            .remove_child(parent_node_id, child_node_id)
        {
            Ok(Value::Element(child_node_id))
        } else {
            Err(JsError::runtime("TypeError: cannot remove this element"))
        }
    }

    fn assertion_description(arguments: &[Value], index: usize) -> String {
        arguments
            .get(index)
            .map(Value::to_js_string)
            .filter(|message| !message.is_empty())
            .map(|message| format!(": {message}"))
            .unwrap_or_default()
    }

    fn assert_equals(&mut self, arguments: Vec<Value>) -> Result<Value, JsError> {
        if !(2..=3).contains(&arguments.len()) {
            return Err(JsError::runtime(
                "TypeError: assert_equals expects two or three arguments",
            ));
        }

        let actual = &arguments[0];
        let expected = &arguments[1];

        if actual == expected {
            return Ok(Value::Undefined);
        }

        let description = Self::assertion_description(&arguments, 2);

        Err(JsError::runtime(format!(
            "AssertionError{description}; expected {expected:?}, got {actual:?}"
        )))
    }

    fn assert_true(&mut self, arguments: Vec<Value>) -> Result<Value, JsError> {
        if !(1..=2).contains(&arguments.len()) {
            return Err(JsError::runtime(
                "TypeError: assert_true expects one or two arguments",
            ));
        }

        if arguments[0] == Value::Boolean(true) {
            return Ok(Value::Undefined);
        }

        let description = Self::assertion_description(&arguments, 1);

        Err(JsError::runtime(format!(
            "AssertionError{description}; expected true, got {:?}",
            arguments[0]
        )))
    }

    fn assert_false(&mut self, arguments: Vec<Value>) -> Result<Value, JsError> {
        if !(1..=2).contains(&arguments.len()) {
            return Err(JsError::runtime(
                "TypeError: assert_false expects one or two arguments",
            ));
        }

        if arguments[0] == Value::Boolean(false) {
            return Ok(Value::Undefined);
        }

        let description = Self::assertion_description(&arguments, 1);

        Err(JsError::runtime(format!(
            "AssertionError{description}; expected false, got {:?}",
            arguments[0]
        )))
    }

    fn assert_not_equals(&mut self, arguments: Vec<Value>) -> Result<Value, JsError> {
        if !(2..=3).contains(&arguments.len()) {
            return Err(JsError::runtime(
                "TypeError: assert_not_equals expects two or three arguments",
            ));
        }

        if arguments[0] != arguments[1] {
            return Ok(Value::Undefined);
        }

        let description = Self::assertion_description(&arguments, 2);

        Err(JsError::runtime(format!(
            "AssertionError{description}; values must differ, got {:?}",
            arguments[0]
        )))
    }

    fn construct(&mut self, constructor: &str, arguments: &[Expression]) -> Result<Value, JsError> {
        if constructor != "URL" {
            return Err(JsError::runtime(format!(
                "ReferenceError: constructor {constructor} is not defined"
            )));
        }

        if !(1..=2).contains(&arguments.len()) {
            return Err(JsError::runtime(
                "TypeError: URL constructor expects one or two arguments",
            ));
        }

        let input = self.evaluate_expression(&arguments[0])?.to_js_string();

        let url = if arguments.len() == 1 {
            Url::parse(&input)
        } else {
            let base = self.evaluate_expression(&arguments[1])?.to_js_string();

            Url::parse(&base).and_then(|base_url| base_url.join(&input))
        }
        .map_err(|error| JsError::runtime(format!("TypeError: invalid URL: {error}")))?;

        Ok(Value::Url(url))
    }
}

#[cfg(test)]
mod tests {
    use super::{DocumentState, TokenKind, Value, evaluate, evaluate_with_document, tokenize};

    #[test]
    fn tokenizes_url_constructor_syntax() {
        let tokens = tokenize("const url = new URL(test.input, test.base);").unwrap();

        let kinds: Vec<TokenKind> = tokens.into_iter().map(|token| token.kind).collect();

        assert_eq!(
            kinds,
            vec![
                TokenKind::Const,
                TokenKind::Identifier("url".to_owned()),
                TokenKind::Assign,
                TokenKind::New,
                TokenKind::Identifier("URL".to_owned()),
                TokenKind::LeftParen,
                TokenKind::Identifier("test".to_owned()),
                TokenKind::Dot,
                TokenKind::Identifier("input".to_owned()),
                TokenKind::Comma,
                TokenKind::Identifier("test".to_owned()),
                TokenKind::Dot,
                TokenKind::Identifier("base".to_owned()),
                TokenKind::RightParen,
                TokenKind::Semicolon,
                TokenKind::EndOfFile,
            ]
        );
    }

    #[test]
    fn skips_comments_and_decodes_escapes() {
        let tokens = tokenize(
            r#"
// WPT-style input
let base = "https:\/\/example.test\u002Fdocs";
/* ignored */
"#,
        )
        .unwrap();

        let kinds: Vec<TokenKind> = tokens.into_iter().map(|token| token.kind).collect();

        assert_eq!(
            kinds,
            vec![
                TokenKind::Let,
                TokenKind::Identifier("base".to_owned()),
                TokenKind::Assign,
                TokenKind::String("https://example.test/docs".to_owned()),
                TokenKind::Semicolon,
                TokenKind::EndOfFile,
            ]
        );
    }

    #[test]
    fn rejects_unclosed_block_comments() {
        let error = tokenize("/* unfinished").unwrap_err();

        assert_eq!(error.message, "unterminated block comment");
    }

    #[test]
    fn evaluates_url_constructor_and_href() {
        let result = evaluate(
            r##"
                const base = "http://example.org/foo/bar";
                const url = new URL("#", base);
                url.href;
            "##,
        )
        .unwrap();

        assert_eq!(
            result,
            Value::String("http://example.org/foo/bar#".to_owned())
        );
    }

    #[test]
    fn runs_wpt_style_url_assertion() {
        let result = evaluate(
            r##"
                const base = "http://example.org/foo/bar";

                assert_equals(
                    new URL("#", base).href,
                    "http://example.org/foo/bar#",
                    "empty fragment must be preserved",
                );
            "##,
        )
        .unwrap();

        assert_eq!(result, Value::Undefined);
    }

    #[test]
    fn changes_document_title_through_javascript() {
        let mut document = DocumentState::with_title("Before");

        let result = evaluate_with_document(
            r#"
                document.title = "FreeDOM";
                assert_equals(document.title, "FreeDOM");
            "#,
            &mut document,
        )
        .unwrap();

        assert_eq!(result, Value::Undefined);
        assert_eq!(document.title, "FreeDOM");
    }

    #[test]
    fn finds_an_element_by_id_and_reads_its_properties() {
        let mut document = DocumentState::default();
        document.register_element("welcome", "h1", "Welcome home".to_owned());

        evaluate_with_document(
            r#"
                const heading = document.getElementById("welcome");

                assert_equals(heading.id, "welcome");
                assert_equals(heading.tagName, "H1");
                assert_equals(heading.textContent, "Welcome home");
                assert_equals(document.getElementById("missing"), null);
            "#,
            &mut document,
        )
        .unwrap();
    }

    #[test]
    fn evaluates_arithmetic_comparisons_and_string_concatenation() {
        let result = evaluate(
            r#"
                assert_equals(1 + 2 * 3, 7);
                assert_equals("Free" + "DOM", "FreeDOM");
                assert_equals(10 / 2 - 1, 4);
                assert_equals(3 < 5, true);
                assert_equals(3 === 3, true);
                assert_equals(3 !== 4, true);

                2 + 2;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Number(4.0));
    }

    #[test]
    fn runs_synchronous_wpt_style_test_callbacks() {
        let result = evaluate(
            r#"
                test(() => {
                    assert_equals(1 + 2 * 3, 7);
                }, "arrow callback");

                test(function() {
                    assert_equals("Free" + "DOM", "FreeDOM");
                }, "function callback");
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Undefined);
    }

    #[test]
    fn passes_test_context_to_wpt_style_callbacks() {
        let result = evaluate(
            r#"
                test(t => {
                    assert_equals(t === t, true);
                    assert_equals(2 + 2, 4);
                }, "single arrow parameter");

                test(function(t) {
                    assert_equals(t === t, true);
                    assert_equals("Free" + "DOM", "FreeDOM");
                }, "single function parameter");
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Undefined);
    }

    #[test]
    fn runs_common_testharness_assertions() {
        let result = evaluate(
            r#"
                test(t => {
                    assert_true(t === t);
                    assert_false(2 < 1);
                    assert_not_equals("FreeDOM", "Chrome");
                }, "common synchronous assertions");
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Undefined);
    }

    #[test]
    fn runs_wpt_cleanup_callbacks_with_this_binding() {
        let mut document = DocumentState::with_title("Before");

        evaluate_with_document(
            r#"
                test(function() {
                    assert_true(this === this);

                    this.add_cleanup(function() {
                        document.title = "After cleanup";
                    });

                    document.title = "During test";
                }, "cleanup callbacks run after the test body");

                assert_equals(document.title, "After cleanup");
            "#,
            &mut document,
        )
        .unwrap();

        assert_eq!(document.title, "After cleanup");
    }

    #[test]
    fn preserves_document_changes_from_cleanups_after_a_failed_test() {
        let mut document = DocumentState::with_title("Before");

        let error = evaluate_with_document(
            r#"
                test(function() {
                    this.add_cleanup(function() {
                        document.title = "Cleaned after failure";
                    });

                    assert_equals(1, 2, "intentional failure");
                }, "cleanup on failure");
            "#,
            &mut document,
        )
        .unwrap_err();

        assert!(error.message.contains("intentional failure"));
        assert_eq!(document.title, "Cleaned after failure");
    }

    #[test]
    fn creates_attaches_and_removes_dynamic_elements() {
        let mut document = DocumentState::default();
        document.register_body("", String::new());

        evaluate_with_document(
            r#"
                const element = document.createElement("div");

                element.setAttribute("id", "dynamic");
                document.body.appendChild(element);

                assert_equals(document.getElementById("dynamic"), element);
                assert_true(element instanceof HTMLDivElement);

                document.body.removeChild(element);

                assert_equals(document.getElementById("dynamic"), null);
            "#,
            &mut document,
        )
        .unwrap();
    }

    #[test]
    fn connects_and_disconnects_an_entire_dynamic_subtree() {
        let mut document = DocumentState::default();
        document.register_body("", String::new());

        evaluate_with_document(
            r#"
                const parent = document.createElement("div");
                const child = document.createElement("div");

                child.setAttribute("id", "nested");
                parent.appendChild(child);

                assert_equals(document.getElementById("nested"), null);

                document.body.appendChild(parent);
                assert_equals(document.getElementById("nested"), child);

                document.body.removeChild(parent);
                assert_equals(document.getElementById("nested"), null);
            "#,
            &mut document,
        )
        .unwrap();
    }

    #[test]
    fn resolves_duplicate_ids_in_actual_tree_order() {
        let mut document = DocumentState::default();
        document.register_body("", String::new());

        evaluate_with_document(
            r#"
                const first_created = document.createElement("div");
                const second_created = document.createElement("div");

                first_created.id = "duplicate";
                second_created.id = "duplicate";

                document.body.appendChild(second_created);
                document.body.appendChild(first_created);

                assert_equals(
                    document.getElementById("duplicate"),
                    second_created
                );
            "#,
            &mut document,
        )
        .unwrap();
    }

    #[test]
    fn exposes_mutable_attributes_in_insertion_order() {
        let mut document = DocumentState::default();
        document.register_body("", String::new());

        evaluate_with_document(
            r#"
                const element = document.createElement("div");

                element.setAttribute("id", "before");
                element.setAttribute("data-kind", "example");
                document.body.appendChild(element);

                assert_equals(element.attributes[0].value, "before");
                assert_equals(element.attributes[1].value, "example");

                element.attributes[0].value = "after";

                assert_equals(element.id, "after");
                assert_equals(element.getAttribute("id"), "after");
                assert_equals(document.getElementById("before"), null);
                assert_equals(document.getElementById("after"), element);
            "#,
            &mut document,
        )
        .unwrap();
    }

    #[test]
    fn replaces_inner_html_children_and_updates_id_lookup() {
        let mut document = DocumentState::default();
        document.register_body("", String::new());

        evaluate_with_document(
            r#"
                const container = document.createElement("div");
                document.body.appendChild(container);

                container.innerHTML = "<div id='first'><span id='nested'></span></div>";

                assert_not_equals(document.getElementById("first"), null);
                assert_not_equals(document.getElementById("nested"), null);

                container.innerHTML = "<p id='second'></p>";

                assert_equals(document.getElementById("first"), null);
                assert_equals(document.getElementById("nested"), null);
                assert_equals(document.getElementById("second").tagName, "P");

                container.innerHTML = "";

                assert_equals(document.getElementById("second"), null);
            "#,
            &mut document,
        )
        .unwrap();
    }

    #[test]
    fn function_local_declarations_do_not_overwrite_global_bindings() {
        evaluate(
            r#"
                test(function() {
                    var test = "local value";
                    assert_equals(test, "local value");
                });

                test(function() {
                    assert_true(true);
                });
            "#,
        )
        .unwrap();
    }

    #[test]
    fn replaces_outer_html_and_disconnects_the_old_subtree() {
        let mut document = DocumentState::default();
        document.register_body("", String::new());

        evaluate_with_document(
            r#"
                const original = document.createElement("div");
                original.id = "old";
                document.body.appendChild(original);

                original.outerHTML = "<p id='new'></p>";

                assert_equals(document.getElementById("old"), null);
                assert_equals(document.getElementById("new").tagName, "P");

                const replacement = document.getElementById("new");
                replacement.outerHTML = "";

                assert_equals(document.getElementById("new"), null);
            "#,
            &mut document,
        )
        .unwrap();
    }

    #[test]
    fn exposes_live_document_tag_collections() {
        let mut document = DocumentState::default();
        document.register_body("", String::new());

        evaluate_with_document(
            r#"
                const elements = document.getElementsByTagName("div");

                assert_equals(elements.length, 0);

                const first = document.createElement("div");
                const second = document.createElement("div");

                document.body.appendChild(first);

                assert_equals(elements.length, 1);
                assert_equals(elements[0], first);

                document.body.insertBefore(second, first);

                assert_equals(elements.length, 2);
                assert_equals(elements[0], second);
                assert_equals(elements[1], first);

                document.body.removeChild(first);

                assert_equals(elements.length, 1);
                assert_equals(elements[0], second);

                const all = document.getElementsByTagName("*");
                assert_equals(all.length, 2);
            "#,
            &mut document,
        )
        .unwrap();
    }

    #[test]
    fn evaluates_conditionals_and_returns_from_nested_branches() {
        let result = evaluate(
            r#"
                const choose = function(value) {
                    if (!value) {
                        return "missing";
                    }

                    if (value > 3) {
                        return "large";
                    } else {
                        return "small";
                    }
                };

                assert_equals(choose(0), "missing");
                assert_equals(choose(4), "large");
                assert_equals(choose(1), "small");
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Undefined);
    }

    #[test]
    fn short_circuits_logical_operations() {
        let result = evaluate(
            r#"
                const skipped_and = false && missing_value;
                const skipped_or = true || missing_value;

                assert_equals(skipped_and, false);
                assert_equals(skipped_or, true);

                assert_equals("left" && "right", "right");
                assert_equals("" || "fallback", "fallback");
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Undefined);
    }

    #[test]
    fn evaluates_while_loops_and_returns_from_them() {
        let result = evaluate(
            r#"
                let count = 0;

                while (count < 3) {
                    count = count + 1;
                }

                assert_equals(count, 3);

                const find_two = function() {
                    let value = 0;

                    while (true) {
                        value = value + 1;

                        if (value === 2) {
                            return value;
                        }
                    }
                };

                assert_equals(find_two(), 2);
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Undefined);
    }

    #[test]
    fn evaluates_for_loops_updates_and_breaks() {
        let result = evaluate(
            r#"
                const sum_before_four = function() {
                    let sum = 0;

                    for (let index = 0; index < 10; index++) {
                        if (index === 4) {
                            break;
                        }

                        sum = sum + index;
                    }

                    return sum;
                };

                assert_equals(sum_before_four(), 6);

                let value = 0;
                ++value;
                value--;

                assert_equals(value, 0);

                let value = 0;
                assert_equals(++value, 1);
                assert_equals(value--, 1);
                assert_equals(value, 0);
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Undefined);
    }

    #[test]
    fn evaluates_continue_in_loops() {
        evaluate(
            r#"
                let sum = 0;

                for (let index = 0; index < 5; index++) {
                    if (index === 2) {
                        continue;
                    }

                    sum = sum + index;
                }

                assert_equals(sum, 8);

                let current = 0;
                let count = 0;

                while (current < 5) {
                    current++;

                    if (current === 3) {
                        continue;
                    }

                    count = count + 1;
                }

                assert_equals(count, 4);
            "#,
        )
        .unwrap();
    }
}
