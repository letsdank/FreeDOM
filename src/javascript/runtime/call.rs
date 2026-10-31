//! Call dispatch for the interpreter's currently supported callable values.
//!
//! ECMAScript call evaluation:
//! <https://tc39.es/ecma262/#sec-evaluatecall>
//!
//! Host bindings and supported call semantics are incomplete.

use super::{
    ArrayMethodKind, DocumentMethod, ElementMethod, Expression, Interpreter, JsError,
    NativeFunction, TestContextMethod, Value,
};

impl Interpreter {
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

    pub(super) fn evaluate_call(
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

            Value::NativeFunction(NativeFunction::ObjectCreate) => Self::object_create(values),

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

            _ => Err(JsError::type_error("this value is not callable")),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::{Value, evaluate};

    #[test]
    fn dispatches_array_methods() {
        let result = evaluate("let items = [1]; items.push(2); items.join(',');").unwrap();

        assert_eq!(result, Value::String("1,2".to_owned()));
    }

    #[test]
    fn dispatches_regexp_test() {
        let result = evaluate("const r = new RegExp('b'); r.test('abc');").unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn dispatches_wpt_shim_and_callback() {
        let result = evaluate("test(function() { assert_equals(2, 2); }, 'dispatch');").unwrap();

        assert_eq!(result, Value::Undefined);
    }

    #[test]
    fn reports_non_callable_value() {
        let error = evaluate("let n = 1; n();").unwrap_err();

        assert_eq!(error.message, "TypeError: this value is not callable");
    }
}
