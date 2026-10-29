use std::collections::HashMap;

use crate::url::Url;

use super::{
    ArrayMethodKind, BinaryOperator, DocumentMethod, DocumentState, ElementMethod, Expression,
    JsError, NativeFunction, Statement, TestContextMethod, UserFunction, Value,
};

#[path = "runtime/dom.rs"]
mod dom_bindings;

#[path = "runtime/regexp.rs"]
mod regexp_bindings;

#[path = "runtime/array.rs"]
mod array;

#[path = "runtime/value_ops.rs"]
mod value_ops;

#[path = "runtime/wpt_shim.rs"]
mod wpt_shim;

#[path = "runtime/statement.rs"]
mod statement;

#[path = "runtime/operators.rs"]
mod operators;

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
        self.execute_statements(program)
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

                    (Value::Array(values), property) => Ok(Self::array_get(values, property)),

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

                    (Value::RegExp(regexp), "lastIndex") => {
                        Ok(regexp.inner.borrow().last_index.clone())
                    }

                    (Value::RegExp(regexp), "test") => Ok(Value::RegExpTest(regexp)),

                    (Value::RegExp(regexp), "exec") => Ok(Value::RegExpExec(regexp)),

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

            Expression::Conditional {
                condition,
                consequent,
                alternate,
            } => {
                let condition = self.evaluate_expression(condition)?;

                if self.is_truthy(&condition) {
                    self.evaluate_expression(consequent)
                } else {
                    self.evaluate_expression(alternate)
                }
            }

            Expression::TypeOf(expression) => {
                let value = self.evaluate_expression(expression)?;
                Ok(Value::String(Self::type_of(&value).to_owned()))
            }
        }
    }

    fn index_value(&self, object: Value, index: Value) -> Result<Value, JsError> {
        if let Value::Array(values) = object {
            return Ok(Self::array_get(values, &index.to_js_string()));
        }

        if let Value::Object(values) = object {
            return Ok(values
                .borrow()
                .get(&index.to_js_string())
                .cloned()
                .unwrap_or(Value::Undefined));
        }

        if let Value::RegExp(regexp) = object {
            return match index.to_js_string().as_str() {
                "lastIndex" => Ok(regexp.inner.borrow().last_index.clone()),

                "test" => Ok(Value::RegExpTest(regexp)),

                "exec" => Ok(Value::RegExpExec(regexp)),

                property => Err(JsError::runtime(format!(
                    "cannot read property {property:?} from this value"
                ))),
            };
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

            Value::RegExpTest(regexp) => {
                let input = values.first().unwrap_or(&Value::Undefined).to_js_string();

                let units: Vec<u16> = input.encode_utf16().collect();

                let matched = Self::run_regexp(&regexp, &units)?.is_some();

                Ok(Value::Boolean(matched))
            }

            Value::RegExpExec(regexp) => {
                let input = values.first().unwrap_or(&Value::Undefined).to_js_string();

                let units: Vec<u16> = input.encode_utf16().collect();

                let has_indices = regexp.inner.borrow().regexp.has_indices();

                match Self::run_regexp(&regexp, &units)? {
                    None => Ok(Value::Null),

                    Some(result) => Self::regexp_exec_value(input, &units, result, has_indices),
                }
            }

            _ => Err(JsError::runtime("TypeError: this value is not callable")),
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

    fn construct(&mut self, constructor: &str, arguments: &[Expression]) -> Result<Value, JsError> {
        if constructor == "RegExp" {
            return self.construct_regexp(arguments);
        }

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
