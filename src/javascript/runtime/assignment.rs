//! Partial assignment references for the JavaScript interpreter.
//!
//! <https://tc39.es/ecma262/#sec-assignment-operators>
//!
//! This is not a complete ECMAScript Reference Record implementation.

use super::{Expression, Interpreter, JsError, Value};

pub(super) enum AssignmentReference {
    Binding(String),
    Member {
        object: Value,
        property: String,
        document_title: bool,
    },
    Index {
        object: Value,
        index: Value,
    },
}

impl Interpreter {
    pub(super) fn prepare_assignment(
        &mut self,
        target: &Expression,
    ) -> Result<AssignmentReference, JsError> {
        match target {
            Expression::Identifier(name) => Ok(AssignmentReference::Binding(name.clone())),

            Expression::Member { object, property } => {
                let is_document_title_syntax = matches!(
                    object.as_ref(),
                    Expression::Identifier(name)
                        if name == "document" && property == "title"
                );

                let object = self.evaluate_expression(object)?;

                let document_title =
                    is_document_title_syntax && matches!(&object, Value::Document(_));

                Ok(AssignmentReference::Member {
                    object,
                    property: property.clone(),
                    document_title,
                })
            }

            Expression::Index { object, index } => {
                let object = self.evaluate_expression(object)?;
                let index = self.evaluate_expression(index)?;

                Ok(AssignmentReference::Index { object, index })
            }

            _ => Err(JsError::runtime(
                "TypeError: assignment target is not supported",
            )),
        }
    }

    pub(super) fn read_assignment(
        &mut self,
        reference: &AssignmentReference,
    ) -> Result<Value, JsError> {
        match reference {
            AssignmentReference::Binding(name) => self
                .variables
                .get(name)
                .cloned()
                .ok_or_else(|| JsError::reference_error(name)),

            AssignmentReference::Member {
                object, property, ..
            } => self.member_value(object.clone(), property),

            AssignmentReference::Index { object, index } => {
                self.index_value(object.clone(), index.clone())
            }
        }
    }

    pub(super) fn write_assignment(
        &mut self,
        reference: AssignmentReference,
        value: Value,
    ) -> Result<(), JsError> {
        match reference {
            AssignmentReference::Binding(name) => {
                self.variables.insert(name, value);
                Ok(())
            }

            AssignmentReference::Member {
                object,
                property,
                document_title,
            } => {
                if document_title {
                    let Some(Value::Document(document)) = self.variables.get_mut("document") else {
                        return Err(JsError::runtime(
                            "TypeError: document binding is unavailable",
                        ));
                    };

                    document.title = value.to_js_string();
                    return Ok(());
                }

                match object {
                    Value::Null | Value::Undefined => Err(JsError::type_error(
                        "cannot write property of null or undefined",
                    )),

                    Value::Object(values) => {
                        values.borrow_mut().set(property, value);
                        Ok(())
                    }

                    Value::NativeError(error) => {
                        error.set(property, value);
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
                        if self.current_document_mut()?.set_node_attribute(
                            node_id,
                            &name,
                            value.to_js_string(),
                        ) {
                            Ok(())
                        } else {
                            Err(JsError::runtime(
                                "TypeError: attribute owner is no longer available",
                            ))
                        }
                    }

                    Value::RegExp(regexp) if property == "lastIndex" => {
                        regexp.inner.borrow_mut().last_index = value;
                        Ok(())
                    }

                    Value::Array(values) => Self::array_set(values, &property, value),

                    Value::Element(_) => Err(JsError::runtime(format!(
                        "Element.{property} is not implemented yet"
                    ))),

                    _ => Err(JsError::runtime(
                        "TypeError: assignment target is not supported",
                    )),
                }
            }

            AssignmentReference::Index { object, index } => match object {
                Value::Null | Value::Undefined => Err(JsError::type_error(
                    "cannot write property of null or undefined",
                )),

                Value::Object(values) => {
                    values.borrow_mut().set(index.to_js_string(), value);
                    Ok(())
                }

                Value::NativeError(error) => {
                    error.set(index.to_js_string(), value);
                    Ok(())
                }

                Value::Array(values) => Self::array_set(values, &index.to_js_string(), value),

                Value::RegExp(regexp) if index.to_js_string() == "lastIndex" => {
                    regexp.inner.borrow_mut().last_index = value;
                    Ok(())
                }

                _ => Err(JsError::runtime(
                    "TypeError: indexed assignment target is not supported",
                )),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::{Value, evaluate};

    #[test]
    fn evaluates_assignment_target_before_right_hand_side() {
        let result = evaluate(
            "let count = 0; \
            let object = {}; \
            function target() { count = count * 10 + 1; return object; } \
            function value() { count = count * 10 + 2; return 3; } \
            target().x = value(); \
            count;",
        )
        .unwrap();

        assert_eq!(result, Value::Number(12.0));
    }

    #[test]
    fn keeps_the_original_object_when_rhs_replaces_binding() {
        let result = evaluate(
            "let object = {}; \
            let original = object; \
            object.x = (object = {}); \
            original.x === object;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn update_evaluates_member_and_index_only_once() {
        let result = evaluate(
            "let count = 0; \
            let object = { x: 1 }; \
            function get() { count = count + 1; return object; } \
            let previous = get().x++; \
            let index = 0; \
            let array = [1]; \
            array[index++]++; \
            count === 1 && previous === 1 && object.x === 2 \
            && index === 1 && array[0] === 2;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn postfix_update_returns_converted_number() {
        let result = evaluate(
            "let value = '1'; \
            let before = value++; \
            before === 1 && value === 2;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn null_assignment_runs_index_and_rhs_before_type_error() {
        let result = evaluate(
            "let count = 0; \
            let name = ''; \
            try { null[count = 1] = (count = 2); } \
            catch (error) { name = error.name; } \
            count === 2 && name === 'TypeError';",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn rhs_error_precedes_null_write_failure() {
        let result = evaluate(
            "let count = 0; \
            let name = ''; \
            try { null[count = 1] = missing; } \
            catch (error) { name = error.name; } \
            count === 1 && name === 'ReferenceError';",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn target_error_prevents_rhs_evaluation() {
        let result = evaluate(
            "let count = 0; \
            try { missing.x = (count = 1); } \
            catch (error) {} \
            count;",
        )
        .unwrap();

        assert_eq!(result, Value::Number(0.0));
    }

    #[test]
    fn assigns_and_updates_title_of_an_object_named_document() {
        let result = evaluate(
            "let document = { title: 1 }; \
            document.title = 5; \
            let previous = document.title++; \
            previous === 5 && document.title === 6;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }
}
