//! Property and indexed reads for the interpreter's supported values.
//!
//! ECMAScript property access:
//! <https://tc39.es/ecma262/#sec-property-accessors>
//!
//! Host-offset bindings and property semantics are incomplete.

use super::{ElementMethod, Interpreter, JsError, NativeFunction, TestContextMethod, Value};
use crate::javascript::DocumentMethod;
use crate::javascript::value::{ObjectState, PropertyLookup};

impl Interpreter {
    pub(super) fn member_value(&self, object: Value, property: &str) -> Result<Value, JsError> {
        match (object, property) {
            (Value::Null | Value::Undefined, _) => Err(JsError::type_error(
                "cannot read property of null or undefined",
            )),

            (Value::TestContext, "add_cleanup") => {
                Ok(Value::TestContextMethod(TestContextMethod::AddCleanup))
            }

            (Value::Array(values), property) => Ok(Self::array_get(values, property)),

            (Value::Object(values), property) => Ok(match ObjectState::lookup(&values, property) {
                PropertyLookup::Found(value) => value,

                PropertyLookup::Absent | PropertyLookup::Unknown => Value::Undefined,
            }),

            (Value::Url(url), "href") => Ok(Value::String(url.to_string())),

            (Value::Url(url), "protocol") => Ok(Value::String(format!("{}:", url.scheme()))),

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

                    "removeAttribute" => Ok(Value::ElementMethod(ElementMethod::RemoveAttribute(
                        node_id,
                    ))),

                    "appendChild" => Ok(Value::ElementMethod(ElementMethod::AppendChild(node_id))),

                    "removeChild" => Ok(Value::ElementMethod(ElementMethod::RemoveChild(node_id))),

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

            (Value::ObjectConstructor, "create") => {
                Ok(Value::NativeFunction(NativeFunction::ObjectCreate))
            }

            (Value::Url(_), property) => Err(JsError::runtime(format!(
                "URL.{property} is not implemented yet"
            ))),

            (Value::Document(_), property) => Err(JsError::runtime(format!(
                "Document.{property} is not implemented yet"
            ))),

            (Value::RegExp(regexp), "lastIndex") => Ok(regexp.inner.borrow().last_index.clone()),

            (Value::RegExp(regexp), "test") => Ok(Value::RegExpTest(regexp)),

            (Value::RegExp(regexp), "exec") => Ok(Value::RegExpExec(regexp)),

            (Value::NativeError(error), property) => Ok(error.get(property)),

            (_, property) => Err(JsError::runtime(format!(
                "cannot read property {property:?} from this value"
            ))),
        }
    }

    pub(super) fn index_value(&self, object: Value, index: Value) -> Result<Value, JsError> {
        if matches!(object, Value::Null | Value::Undefined) {
            return Err(JsError::type_error(
                "cannot read property of null or undefined",
            ));
        }

        if let Value::Array(values) = object {
            return Ok(Self::array_get(values, &index.to_js_string()));
        }

        if let Value::Object(values) = object {
            return Ok(match ObjectState::lookup(&values, &index.to_js_string()) {
                PropertyLookup::Found(value) => value,

                PropertyLookup::Absent | PropertyLookup::Unknown => Value::Undefined,
            });
        }

        if let Value::NativeError(error) = object {
            return Ok(error.get(&index.to_js_string()));
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
}

#[cfg(test)]
mod tests {
    use crate::javascript::{DocumentState, Value, evaluate, evaluate_with_document};

    #[test]
    fn reads_object_properties_using_dot_and_brackets() {
        let result = evaluate("let obj = {name: 'FreeDOM'}; obj.name + obj['name'];").unwrap();

        assert_eq!(result, Value::String("FreeDOMFreeDOM".to_owned()));
    }

    #[test]
    fn evaluates_member_receiver_once() {
        let result = evaluate(
            "let count = 0; \
            const values = [5]; \
            const get = function() { \
                count = count + 1; \
                return values; \
            }; \
            get().length + count;",
        )
        .unwrap();

        assert_eq!(result, Value::Number(2.0));
    }

    #[test]
    fn reads_url_protocol() {
        let result =
            evaluate("const url = new URL('https://example.org/'); url.protocol;").unwrap();

        assert_eq!(result, Value::String("https:".to_owned()));
    }

    #[test]
    fn reads_element_property_through_document() {
        let mut document = DocumentState::default();
        document.register_element("welcome", "h1", "Hello".to_owned());

        let result = evaluate_with_document(
            "document.getElementById('welcome').textContent;",
            &mut document,
        )
        .unwrap();

        assert_eq!(result, Value::String("Hello".to_owned()));
    }

    #[test]
    fn null_and_undefined_property_reads_throw_catchable_type_error() {
        for expression in [
            "null.foo",
            "undefined.foo",
            "null['x']",
            "undefined[undefined]",
        ] {
            let script = format!(
                "let name = ''; \
                try {{ {expression}; }} \
                catch (e) {{ name = e.name; }} \
                name;"
            );

            assert_eq!(
                evaluate(&script).unwrap(),
                Value::String("TypeError".to_owned()),
                "{expression}"
            );
        }
    }

    #[test]
    fn computed_index_is_evaluated_before_null_property_failure() {
        let result = evaluate(
            "let count = 0; \
            try { null[count = count + 1]; } \
            catch (e) {} \
            count;",
        )
        .unwrap();

        assert_eq!(result, Value::Number(1.0));
    }

    #[test]
    fn index_expression_error_precedes_null_property_failure() {
        let result = evaluate(
            "let name = ''; \
            try { null[missing]; } \
            catch (e) { name = e.name; } \
            name;",
        )
        .unwrap();

        assert_eq!(result, Value::String("ReferenceError".to_owned()));
    }

    #[test]
    fn typeof_does_not_suppress_null_property_error() {
        let result = evaluate(
            "let name = ''; \
            try { typeof null.foo; } \
            catch (e) { name = e.name; } \
            name;",
        )
        .unwrap();

        assert_eq!(result, Value::String("TypeError".to_owned()));
    }

    #[test]
    fn finally_runs_after_uncaught_null_property_error() {
        let mut document = DocumentState::default();

        let error = evaluate_with_document(
            "try { null.foo; } \
            finally { document.title = 'clean'; }",
            &mut document,
        )
        .unwrap_err();

        assert!(matches!(
            error.kind,
            crate::javascript::JsErrorKind::NativeException(Value::NativeError(_))
        ));
        assert_eq!(document.title, "clean");
    }

    #[test]
    fn missing_property_on_object_still_returns_undefined() {
        assert_eq!(
            evaluate("let object = {}; object.missing;").unwrap(),
            Value::Undefined,
        );
    }
}
