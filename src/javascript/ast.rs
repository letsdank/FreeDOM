use super::Value;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Statement {
    Declaration {
        name: String,
        value: Expression,
    },
    FunctionDeclaration {
        name: String,
        function: UserFunction,
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
    ForOf {
        binding: String,
        iterable: Expression,
        body: Vec<Statement>,
    },
    Break,
    Continue,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Expression {
    Literal(Value),
    RegExpLiteral {
        pattern: String,
        flags: String,
    },
    Array(Vec<Expression>),
    Object(Vec<(String, Expression)>),
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
    Conditional {
        condition: Box<Expression>,
        consequent: Box<Expression>,
        alternate: Box<Expression>,
    },
    TypeOf(Box<Expression>),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct UserFunction {
    pub(crate) parameters: Vec<String>,
    pub(crate) body: Vec<Statement>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum BinaryOperator {
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
