use crate::freedom_url::Url;
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElementState {
    id: String,
    tag_name: String,
    text_content: String,
    text_content_was_set: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DocumentState {
    pub title: String,
    elements_by_id: HashMap<String, ElementState>,
}

impl DocumentState {
    pub fn with_title(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            elements_by_id: HashMap::new(),
        }
    }

    pub fn register_element(&mut self, id: &str, tag_name: &str, text_content: String) {
        self.elements_by_id
            .entry(id.to_owned())
            .or_insert_with(|| ElementState {
                id: id.to_owned(),
                tag_name: tag_name.to_owned(),
                text_content,
                text_content_was_set: false,
            });
    }

    fn get_element_by_id(&self, id: &str) -> Option<ElementState> {
        self.elements_by_id.get(id).cloned()
    }

    pub fn set_element_text_content(&mut self, id: &str, text_content: String) -> bool {
        let Some(element) = self.elements_by_id.get_mut(id) else {
            return false;
        };

        element.text_content = text_content;
        element.text_content_was_set = true;
        true
    }

    pub fn text_content_override(&self, id: &str) -> Option<&str> {
        let element = self.elements_by_id.get(id)?;

        if element.text_content_was_set {
            Some(element.text_content.as_str())
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum DocumentMethod {
    GetElementById(DocumentState),
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
    Element(ElementState),
    DocumentMethod(DocumentMethod),
    Function(UserFunction),
    TestContext,
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
            Self::DocumentMethod(_) => "function".to_owned(),
            Self::Function(_) => "function".to_owned(),
            Self::TestContext => "[object TestContext]".to_owned(),
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
    Declaration { name: String, value: Expression },
    Expression(Expression),
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
}

#[derive(Clone, Debug, PartialEq)]
struct UserFunction {
    parameters: Vec<String>,
    body: Vec<Statement>,
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
    Interpreter::new().execute(&program)
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

    let result = interpreter.execute(&program)?;

    let Some(Value::Document(updated_document)) = interpreter.variables.remove("document") else {
        return Err(JsError::runtime(
            "TypeError: global document binding must remain a Document",
        ));
    };

    *document = updated_document;

    Ok(result)
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

        let expression = self.parse_equality_expression()?;

        if self.consume_optional(&TokenKind::Assign) {
            let value = self.parse_assignment_expression()?;

            return Ok(Expression::Assignment {
                target: Box::new(expression),
                value: Box::new(value),
            });
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
        let mut expression = self.parse_postfix_expression()?;

        loop {
            let operator = if self.consume_optional(&TokenKind::Star) {
                BinaryOperator::Multiply
            } else if self.consume_optional(&TokenKind::Slash) {
                BinaryOperator::Divide
            } else {
                break;
            };

            let right = self.parse_postfix_expression()?;

            expression = Expression::Binary {
                left: Box::new(expression),
                operator,
                right: Box::new(right),
            };
        }

        Ok(expression)
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

            if self.consume_optional(&TokenKind::LeftParen) {
                let arguments = self.parse_arguments()?;

                expression = Expression::Call {
                    callee: Box::new(expression),
                    arguments,
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
}

impl Interpreter {
    fn new() -> Self {
        let mut variables = HashMap::new();

        variables.insert(
            "assert_equals".to_owned(),
            Value::NativeFunction(NativeFunction::AssertEquals),
        );

        variables.insert(
            "test".to_owned(),
            Value::NativeFunction(NativeFunction::Test),
        );

        Self { variables }
    }

    fn execute(&mut self, program: &[Statement]) -> Result<Value, JsError> {
        let mut result = Value::Undefined;

        for statement in program {
            result = match statement {
                Statement::Declaration { name, value } => {
                    let value = self.evaluate_expression(value)?;
                    self.variables.insert(name.clone(), value);
                    Value::Undefined
                }

                Statement::Expression(expression) => self.evaluate_expression(expression)?,
            };
        }

        Ok(result)
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
                    (Value::Url(url), "href") => Ok(Value::String(url.to_string())),

                    (Value::Url(url), "protocol") => {
                        Ok(Value::String(format!("{}:", url.scheme())))
                    }

                    (Value::Document(document), "title") => Ok(Value::String(document.title)),

                    (Value::Document(document), "getElementById") => Ok(Value::DocumentMethod(
                        DocumentMethod::GetElementById(document),
                    )),

                    (Value::Element(element), "id") => Ok(Value::String(element.id)),

                    (Value::Element(element), "tagName") => {
                        Ok(Value::String(element.tag_name.to_ascii_uppercase()))
                    }

                    (Value::Element(element), "textContent") => {
                        let text_content = self
                            .current_document_element(&element.id)
                            .map(|current| current.text_content)
                            .unwrap_or(element.text_content);

                        Ok(Value::String(text_content))
                    }

                    (Value::Url(_), property) => Err(JsError::runtime(format!(
                        "URL.{property} is not implemented yet"
                    ))),

                    (Value::Document(_), property) => Err(JsError::runtime(format!(
                        "Document.{property} is not implemented yet"
                    ))),

                    (Value::Element(_), property) => Err(JsError::runtime(format!(
                        "Element.{property} is not implemented yet"
                    ))),

                    (_, property) => Err(JsError::runtime(format!(
                        "cannot read property {property:?} from this value"
                    ))),
                }
            }

            Expression::Function(function) => Ok(Value::Function(function.clone())),

            Expression::Assignment { target, value } => {
                let value = self.evaluate_expression(value)?;
                self.assign(target, value.clone())?;
                Ok(value)
            }

            Expression::Call { callee, arguments } => self.evaluate_call(callee, arguments),

            Expression::Binary {
                left,
                operator,
                right,
            } => {
                let left = self.evaluate_expression(left)?;
                let right = self.evaluate_expression(right)?;

                self.evaluate_binary(left, *operator, right)
            }
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
            | Value::DocumentMethod(_)
            | Value::Function(_)
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

    fn current_document_element(&self, id: &str) -> Option<ElementState> {
        let Value::Document(document) = self.variables.get("document")? else {
            return None;
        };

        document.get_element_by_id(id)
    }

    fn set_document_element_text_content(
        &mut self,
        id: &str,
        text_content: String,
    ) -> Result<(), JsError> {
        let Some(Value::Document(document)) = self.variables.get_mut("document") else {
            return Err(JsError::runtime(
                "TypeError: document binding is unavailable",
            ));
        };

        if document.set_element_text_content(id, text_content) {
            Ok(())
        } else {
            Err(JsError::runtime(format!(
                "TypeError: element #{id:?} is no longer in the document"
            )))
        }
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
                    Value::Element(element) if property == "textContent" => {
                        self.set_document_element_text_content(&element.id, value.to_js_string())
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

            Value::DocumentMethod(DocumentMethod::GetElementById(document)) => {
                self.get_element_by_id(document, values)
            }

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

        let arguments = if callback.parameters.is_empty() {
            Vec::new()
        } else {
            vec![Value::TestContext]
        };

        self.call_function(callback, arguments)
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

        for (parameter, argument) in function.parameters.iter().zip(arguments) {
            let previous = self.variables.insert(parameter.clone(), argument);
            replaced_bindings.push((parameter.clone(), previous));
        }

        let result = self.execute(&function.body);

        for (name, previous) in replaced_bindings.into_iter().rev() {
            if let Some(value) = previous {
                self.variables.insert(name, value);
            } else {
                self.variables.remove(&name);
            }
        }

        result
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

        let description = arguments
            .get(2)
            .map(Value::to_js_string)
            .filter(|message| !message.is_empty())
            .map(|message| format!(": {message}"))
            .unwrap_or_default();

        Err(JsError::runtime(format!(
            "AssertionError{description}; expected {expected:?}, got {actual:?}"
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
}
