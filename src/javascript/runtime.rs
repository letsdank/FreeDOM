use std::collections::HashMap;

use crate::url::Url;

use super::{
    ArrayMethodKind, ArrayStorage, BinaryOperator, DocumentMethod, DocumentState, ElementMethod,
    Expression, JsError, NativeFunction, Statement, TestContextMethod, UserFunction, Value,
};

#[path = "runtime/dom.rs"]
mod dom_bindings;

#[derive(Clone, Debug)]
struct Cleanup {
    callback: UserFunction,
    captured: HashMap<String, Value>,
}

pub(super) enum Completion {
    Normal(Value),
    Return(Value),
    Break,
    Continue,
}

impl Completion {
    pub(super) fn into_value(self) -> Value {
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

            Statement::ForOf { binding, body, .. } => {
                if !names.contains(binding) {
                    names.push(binding.clone());
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

pub(super) struct Interpreter {
    variables: HashMap<String, Value>,
    cleanups: Vec<Cleanup>,
    function_locals: Vec<Vec<String>>,
}

impl Interpreter {
    pub(super) fn new() -> Self {
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

        variables.insert("Object".to_owned(), Value::ObjectConstructor);

        Self {
            variables,
            cleanups: Vec::new(),
            function_locals: Vec::new(),
        }
    }

    pub(super) fn set_document(&mut self, document: DocumentState) {
        self.variables
            .insert("document".to_owned(), Value::Document(document));
    }

    pub(super) fn take_document(&mut self) -> Option<DocumentState> {
        let Value::Document(document) = self.variables.remove("document")? else {
            return None;
        };

        Some(document)
    }

    pub(super) fn execute(&mut self, program: &[Statement]) -> Result<Completion, JsError> {
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
                            Completion::Normal(_) => {}
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

                Statement::ForOf {
                    binding,
                    iterable,
                    body,
                } => {
                    let iterable = self.evaluate_expression(iterable)?;

                    let Value::Array(values) = iterable else {
                        return Err(JsError::runtime(
                            "TypeError: for-of currently requires an Array",
                        ));
                    };
                    let values = values.borrow().clone();

                    let previous_binding = self.variables.insert(binding.clone(), Value::Undefined);

                    let completion = (|| -> Result<Completion, JsError> {
                        for value in values {
                            self.variables.insert(binding.clone(), value);

                            match self.execute(body)? {
                                Completion::Normal(_) | Completion::Continue => {}
                                Completion::Break => {
                                    return Ok(Completion::Normal(Value::Undefined));
                                }
                                Completion::Return(value) => return Ok(Completion::Return(value)),
                            }
                        }

                        Ok(Completion::Normal(Value::Undefined))
                    })();

                    if let Some(value) = previous_binding {
                        self.variables.insert(binding.clone(), value);
                    } else {
                        self.variables.remove(binding);
                    }

                    match completion? {
                        Completion::Normal(value) => value,
                        Completion::Return(value) => return Ok(Completion::Return(value)),
                        Completion::Break => return Ok(Completion::Break),
                        Completion::Continue => return Ok(Completion::Continue),
                    }
                }
            };
        }

        Ok(Completion::Normal(result))
    }

    fn evaluate_expression(&mut self, expression: &Expression) -> Result<Value, JsError> {
        match expression {
            Expression::Literal(value) => Ok(value.clone()),

            Expression::Array(expressions) => {
                let mut values = Vec::with_capacity(expressions.len());

                for expression in expressions {
                    values.push(self.evaluate_expression(expression)?);
                }

                Ok(Value::array(values))
            }

            Expression::Object(properties) => {
                let mut values = Vec::with_capacity(properties.len());

                for (name, expression) in properties {
                    values.push((name.clone(), self.evaluate_expression(expression)?));
                }

                Ok(Value::object(values))
            }

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

                    (Value::Array(values), "length") => {
                        Ok(Value::Number(values.borrow().len() as f64))
                    }

                    (Value::Array(values), "push") => Ok(Value::ArrayMethod {
                        receiver: values,
                        method: ArrayMethodKind::Push,
                    }),

                    (Value::Array(values), "pop") => Ok(Value::ArrayMethod {
                        receiver: values,
                        method: ArrayMethodKind::Pop,
                    }),

                    (Value::Array(values), "join") => Ok(Value::ArrayMethod {
                        receiver: values,
                        method: ArrayMethodKind::Join,
                    }),

                    (Value::Object(values), property) => Ok(values
                        .borrow()
                        .get(property)
                        .cloned()
                        .unwrap_or(Value::Undefined)),

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

                    (Value::Document(_), "querySelector") => {
                        Ok(Value::DocumentMethod(DocumentMethod::QuerySelector))
                    }

                    (Value::Document(_), "querySelectorAll") => {
                        Ok(Value::DocumentMethod(DocumentMethod::QuerySelectorAll))
                    }

                    (Value::NodeList(nodes), "length") => Ok(Value::Number(nodes.len() as f64)),

                    (Value::ElementChildren(node_id), "length") => {
                        let length = self
                            .current_document_element(node_id)
                            .map(|element| element.children.len())
                            .unwrap_or(0);

                        Ok(Value::Number(length as f64))
                    }

                    (Value::HtmlCollection(tag_name), "length") => {
                        let length = self
                            .current_document()
                            .map(|document| document.get_elements_by_tag_name(&tag_name).len())
                            .unwrap_or(0);

                        Ok(Value::Number(length as f64))
                    }

                    (Value::ClassList(node_id), "length") => {
                        Ok(Value::Number(self.class_list_tokens(node_id)?.len() as f64))
                    }

                    (Value::ClassList(node_id), "contains") => Ok(Value::ElementMethod(
                        ElementMethod::ClassListContains(node_id),
                    )),

                    (Value::ClassList(node_id), "add") => {
                        Ok(Value::ElementMethod(ElementMethod::ClassListAdd(node_id)))
                    }

                    (Value::ClassList(node_id), "remove") => Ok(Value::ElementMethod(
                        ElementMethod::ClassListRemove(node_id),
                    )),

                    (Value::ClassList(node_id), "toggle") => Ok(Value::ElementMethod(
                        ElementMethod::ClassListToggle(node_id),
                    )),

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

                            "parentElement" | "parentNode" => {
                                Ok(element.parent.map(Value::Element).unwrap_or(Value::Null))
                            }

                            "firstElementChild" => Ok(element
                                .children
                                .first()
                                .copied()
                                .map(Value::Element)
                                .unwrap_or(Value::Null)),

                            "lastElementChild" => Ok(element
                                .children
                                .last()
                                .copied()
                                .map(Value::Element)
                                .unwrap_or(Value::Null)),

                            "children" => Ok(Value::ElementChildren(node_id)),

                            "childElementCount" => Ok(Value::Number(element.children.len() as f64)),

                            "isConnected" => Ok(Value::Boolean(element.connected)),

                            "previousElementSibling" => Ok(self
                                .current_document()
                                .and_then(|document| document.previous_element_sibling(node_id))
                                .map(Value::Element)
                                .unwrap_or(Value::Null)),

                            "nextElementSibling" => Ok(self
                                .current_document()
                                .and_then(|document| document.next_element_sibling(node_id))
                                .map(Value::Element)
                                .unwrap_or(Value::Null)),

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

                            "querySelector" => {
                                Ok(Value::ElementMethod(ElementMethod::QuerySelector(node_id)))
                            }

                            "querySelectorAll" => Ok(Value::ElementMethod(
                                ElementMethod::QuerySelectorAll(node_id),
                            )),

                            "className" => Ok(Value::String(
                                element
                                    .attributes
                                    .iter()
                                    .find(|attribute| attribute.name == "class")
                                    .map(|attribute| attribute.value.clone())
                                    .unwrap_or_default(),
                            )),

                            "classList" => Ok(Value::ClassList(node_id)),

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

                    (Value::ObjectConstructor, "keys") => {
                        Ok(Value::NativeFunction(NativeFunction::ObjectKeys))
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
        if let Value::Object(values) = object {
            return Ok(values
                .borrow()
                .get(&index.to_js_string())
                .cloned()
                .unwrap_or(Value::Undefined));
        }

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
                .borrow()
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

            Value::ElementChildren(node_id) => Ok(self
                .current_document_element(node_id)
                .and_then(|element| element.children.get(index as usize))
                .copied()
                .map(Value::Element)
                .unwrap_or(Value::Undefined)),

            Value::NodeList(nodes) => Ok(nodes
                .get(index as usize)
                .copied()
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
            | Value::ElementChildren(_)
            | Value::Attributes(_)
            | Value::Attribute { .. }
            | Value::DocumentMethod(_)
            | Value::Function(_)
            | Value::ElementConstructor(_)
            | Value::TestContextMethod(_)
            | Value::ElementMethod(_)
            | Value::NodeList(_)
            | Value::ClassList(_)
            | Value::ArrayMethod { .. }
            | Value::ObjectConstructor
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
                    Value::Object(values) => {
                        values.borrow_mut().set(property.clone(), value);
                        Ok(())
                    }

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

                    Value::Element(node_id) if property == "className" => {
                        let class_name = value.to_js_string();

                        if self
                            .current_document_mut()?
                            .set_node_attribute(node_id, "class", class_name)
                        {
                            Ok(())
                        } else {
                            Err(JsError::runtime(
                                "TypeError: element is no longer available",
                            ))
                        }
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

            Expression::Index { object, index } => {
                let object = self.evaluate_expression(object)?;
                let index = self.evaluate_expression(index)?;

                match object {
                    Value::Object(values) => {
                        values.borrow_mut().set(index.to_js_string(), value);

                        Ok(())
                    }

                    Value::Array(values) => {
                        let Some(index) = self.to_number(&index) else {
                            return Err(JsError::runtime("TypeError: array index must be numeric"));
                        };

                        if !index.is_finite() || index < 0.0 || index.fract() != 0.0 {
                            return Err(JsError::runtime(
                                "RangeError: array index must be a non-negative integer",
                            ));
                        }

                        let index = index as usize;
                        let mut values = values.borrow_mut();

                        if index >= values.len() {
                            values.resize(index + 1, Value::Undefined);
                        }

                        values[index] = value;

                        Ok(())
                    }

                    _ => Err(JsError::runtime(
                        "TypeError: indexed assignment target is not supported",
                    )),
                }
            }

            _ => Err(JsError::runtime(
                "TypeError: assignment target is not supported",
            )),
        }
    }

    fn array_push(receiver: ArrayStorage, values: Vec<Value>) -> Result<Value, JsError> {
        let mut receiver = receiver.borrow_mut();
        receiver.extend(values);

        Ok(Value::Number(receiver.len() as f64))
    }

    fn array_pop(receiver: ArrayStorage, values: Vec<Value>) -> Result<Value, JsError> {
        if !values.is_empty() {
            return Err(JsError::runtime(
                "TypeError: Array.pop expects no arguments",
            ));
        }

        Ok(receiver.borrow_mut().pop().unwrap_or(Value::Undefined))
    }

    fn object_keys(&self, arguments: Vec<Value>) -> Result<Value, JsError> {
        if arguments.len() != 1 {
            return Err(JsError::runtime(
                "TypeError: Object.keys expects one argument",
            ));
        }

        let Value::Object(object) = &arguments[0] else {
            return Err(JsError::runtime(
                "TypeError: Object.keys currently requires an object",
            ));
        };

        Ok(Value::array(
            object
                .borrow()
                .keys()
                .into_iter()
                .map(Value::String)
                .collect(),
        ))
    }

    fn array_join(receiver: ArrayStorage, arguments: Vec<Value>) -> Result<Value, JsError> {
        if arguments.len() > 1 {
            return Err(JsError::runtime(
                "TypeError: Array.join expects zero or one argument",
            ));
        }

        let separator = match arguments.first() {
            None | Some(Value::Undefined) => ",".to_owned(),
            Some(value) => value.to_js_string(),
        };

        let joined = receiver
            .borrow()
            .iter()
            .map(|value| match value {
                Value::Undefined | Value::Null => String::new(),
                value => value.to_js_string(),
            })
            .collect::<Vec<_>>()
            .join(&separator);

        Ok(Value::String(joined))
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
            Value::ArrayMethod {
                receiver,
                method: ArrayMethodKind::Push,
            } => Self::array_push(receiver, values),

            Value::ArrayMethod {
                receiver,
                method: ArrayMethodKind::Pop,
            } => Self::array_pop(receiver, values),

            Value::ArrayMethod {
                receiver,
                method: ArrayMethodKind::Join,
            } => Self::array_join(receiver, values),

            Value::NativeFunction(NativeFunction::AssertEquals) => self.assert_equals(values),

            Value::NativeFunction(NativeFunction::Test) => self.run_test(values),

            Value::NativeFunction(NativeFunction::AssertTrue) => self.assert_true(values),

            Value::NativeFunction(NativeFunction::AssertFalse) => self.assert_false(values),

            Value::NativeFunction(NativeFunction::AssertNotEquals) => {
                self.assert_not_equals(values)
            }

            Value::NativeFunction(NativeFunction::ObjectKeys) => self.object_keys(values),

            Value::DocumentMethod(DocumentMethod::GetElementById(document)) => {
                self.get_element_by_id(document, values)
            }

            Value::DocumentMethod(DocumentMethod::CreateElement) => {
                self.create_document_element(values)
            }

            Value::DocumentMethod(DocumentMethod::GetElementsByTagName) => {
                self.get_elements_by_tag_name(values)
            }

            Value::DocumentMethod(DocumentMethod::QuerySelector) => self.query_selector(values),

            Value::DocumentMethod(DocumentMethod::QuerySelectorAll) => {
                self.query_selector_all(values)
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

            Value::ElementMethod(ElementMethod::QuerySelector(node_id)) => {
                self.query_selector_within(node_id, values)
            }

            Value::ElementMethod(ElementMethod::QuerySelectorAll(node_id)) => {
                self.query_selector_all_within(node_id, values)
            }

            Value::ElementMethod(ElementMethod::ClassListContains(node_id)) => {
                self.class_list_contains(node_id, values)
            }

            Value::ElementMethod(ElementMethod::ClassListAdd(node_id)) => {
                self.class_list_add(node_id, values)
            }

            Value::ElementMethod(ElementMethod::ClassListRemove(node_id)) => {
                self.class_list_remove(node_id, values)
            }

            Value::ElementMethod(ElementMethod::ClassListToggle(node_id)) => {
                self.class_list_toggle(node_id, values)
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
