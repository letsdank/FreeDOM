//! Property and indexed reads for the interpreter's supported values.
//!
//! ECMAScript property access:
//! <https://tc39.es/ecma262/#sec-property-accessors>
//!
//! Host-offset bindings and property semantics are incomplete.

use super::{ElementMethod, Interpreter, JsError, NativeFunction, TestContextMethod, Value};
use crate::javascript::DocumentMethod;
use crate::javascript::value::PropertyLookup;

impl Interpreter {
    pub(super) fn member_value(&self, object: Value, property: &str) -> Result<Value, JsError> {
        match (object, property) {
            (Value::Null | Value::Undefined, _) => Err(JsError::type_error(
                "cannot read property of null or undefined",
            )),

            (Value::TestContext, "add_cleanup") => {
                Ok(Value::TestContextMethod(TestContextMethod::AddCleanup))
            }

            (Value::Array(values), property) => Ok(self.array_property(values, property)),

            (Value::Function(function), property) => Ok(function.get(&self.host_roots, property)),

            (Value::NativeFunction(_), "call") => {
                Ok(Value::NativeFunction(NativeFunction::FunctionCall))
            }

            (Value::NativeFunction(_), "apply") => {
                Ok(Value::NativeFunction(NativeFunction::FunctionApply))
            }

            (Value::NativeFunction(_), "bind") => {
                Ok(Value::NativeFunction(NativeFunction::FunctionBind))
            }

            (Value::NativeFunction(NativeFunction::PromiseConstructor), "resolve") => {
                Ok(Value::NativeFunction(NativeFunction::PromiseResolve))
            }

            (Value::NativeFunction(NativeFunction::PromiseConstructor), "reject") => {
                Ok(Value::NativeFunction(NativeFunction::PromiseReject))
            }

            (Value::NativeFunction(NativeFunction::PromiseConstructor), "prototype") => {
                Ok(Value::Object(self.promise_prototype.clone()))
            }

            (Value::NativeFunction(NativeFunction::ArrayConstructor), "prototype") => {
                Ok(Value::Array(self.array_prototype.clone()))
            }

            (Value::String(_), "split") => Ok(Value::NativeFunction(NativeFunction::StringSplit)),

            (Value::String(_), "replace") => {
                Ok(Value::NativeFunction(NativeFunction::StringReplace))
            }

            (Value::NativeFunction(NativeFunction::StringConstructor), "fromCharCode") => {
                Ok(Value::NativeFunction(NativeFunction::StringFromCharCode))
            }

            (Value::NativeFunction(NativeFunction::ErrorConstructor), "prototype") => {
                Ok(Value::Object(self.error_prototype.clone()))
            }

            (Value::NativeFunction(NativeFunction::TypeErrorConstructor), "prototype") => {
                Ok(Value::Object(self.type_error_prototype.clone()))
            }

            (Value::NativeFunction(NativeFunction::ReferenceErrorConstructor), "prototype") => {
                Ok(Value::Object(self.reference_error_prototype.clone()))
            }

            (Value::Object(values), property) => Ok(
                match self.host_roots.lookup_ordinary_property(&values, property) {
                    PropertyLookup::Found(value) => value,

                    PropertyLookup::Absent | PropertyLookup::Unknown => Value::Undefined,
                },
            ),

            (Value::Arguments(arguments), "length") => Ok(Value::Number(arguments.length() as f64)),

            (Value::Arguments(_), _) => Err(JsError::unsupported(
                "arguments property is not implemented",
            )),

            (Value::Url(url), "href") => Ok(Value::String(url.to_string())),

            (Value::Url(url), "protocol") => Ok(Value::String(format!("{}:", url.scheme()))),

            (Value::Document(document), "title") => {
                Ok(Value::String(document.borrow().title.clone()))
            }

            (Value::Document(document), "body") => Ok(document
                .borrow()
                .body_node()
                .map(Value::Element)
                .unwrap_or(Value::Null)),

            (Value::Document(_), "getElementById") => {
                Ok(Value::DocumentMethod(DocumentMethod::GetElementById))
            }

            (Value::DocumentMethod(DocumentMethod::GetElementById), "call") => {
                Ok(Value::NativeFunction(NativeFunction::FunctionCall))
            }

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

            (collection @ (Value::HtmlCollection(_) | Value::ElementChildren(_)), "length") => Ok(
                Value::Number(self.html_collection_nodes(&collection)?.len() as f64),
            ),

            (Value::HtmlCollection(_) | Value::ElementChildren(_), "item") => {
                Ok(Value::NativeFunction(NativeFunction::HtmlCollectionItem))
            }

            (Value::HtmlCollection(_) | Value::ElementChildren(_), "namedItem") => Ok(
                Value::NativeFunction(NativeFunction::HtmlCollectionNamedItem),
            ),

            (collection @ (Value::HtmlCollection(_) | Value::ElementChildren(_)), property) => {
                if matches!(
                    property,
                    "constructor"
                        | "toString"
                        | "toLocaleString"
                        | "valueOf"
                        | "hasOwnProperty"
                        | "isPrototypeOf"
                        | "propertyIsEnumerable"
                        | "__proto__"
                        | "__defineGetter__"
                        | "__defineSetter__"
                        | "__lookupGetter__"
                        | "__lookupSetter__"
                ) {
                    return Err(JsError::unsupported(
                        "HTMLCollection prototype properties are not implemented",
                    ));
                }

                let nodes = self.html_collection_nodes(&collection)?;

                let Some(document) = self.current_document() else {
                    return Ok(Value::Undefined);
                };

                Ok(document
                    .collection_named_element(&nodes, property)
                    .map(Value::Element)
                    .unwrap_or(Value::Undefined))
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
                    "getContext" if element.tag_name.eq_ignore_ascii_case("canvas") => {
                        Ok(Value::ElementMethod(ElementMethod::GetContext(node_id)))
                    }

                    "width" | "height" if element.tag_name.eq_ignore_ascii_case("canvas") => {
                        let default = if property == "width" { 300 } else { 150 };

                        let dimension = element
                            .attributes
                            .iter()
                            .find(|attribute| attribute.name == property)
                            .and_then(|attribute| {
                                crate::graphics::canvas::parse_canvas_dimension(&attribute.value)
                            })
                            .unwrap_or(default);

                        Ok(Value::Number(dimension as f64))
                    }

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

            (Value::ObjectConstructor, "entries") => {
                Ok(Value::NativeFunction(NativeFunction::ObjectEntries))
            }

            (Value::ObjectConstructor, "create") => {
                Ok(Value::NativeFunction(NativeFunction::ObjectCreate))
            }

            (Value::ObjectConstructor, "prototype") => {
                Ok(Value::Object(self.object_prototype.clone()))
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
        if let Value::Arguments(arguments) = &object {
            if matches!(&index, Value::String(name) if name == "length") {
                return Ok(Value::Number(arguments.length() as f64));
            }

            let name = match &index {
                Value::String(name) => name.clone(),

                Value::Number(number) => crate::javascript::number::integer_property_key(*number)
                    .ok_or_else(|| {
                    JsError::unsupported("arguments index conversion is not implemented")
                })?,

                _ => {
                    return Err(JsError::unsupported(
                        "arguments index conversion is not implemented",
                    ));
                }
            };

            let position = name
                .parse::<usize>()
                .ok()
                .filter(|position| position.to_string() == name)
                .ok_or_else(|| {
                    JsError::unsupported("arguments property lookup is not implemented")
                })?;

            return arguments.get_index(position);
        }

        if matches!(object, Value::Null | Value::Undefined) {
            return Err(JsError::type_error(
                "cannot read property of null or undefined",
            ));
        }

        if matches!(
            &object,
            Value::ObjectConstructor
                | Value::Document(_)
                | Value::Element(_)
                | Value::Array(_)
                | Value::Object(_)
                | Value::Function(_)
                | Value::NativeFunction(_)
                | Value::NativeError(_)
                | Value::RegExp(_)
                | Value::Url(_)
                | Value::Attribute { .. }
                | Value::DocumentMethod(_)
                | Value::TestContext
                | Value::ClassList(_)
        ) {
            return self.member_value(object, &index.to_js_string());
        }

        if let Value::String(name) = &index {
            let is_named_property = match &object {
                Value::String(_) => matches!(name.as_str(), "split" | "replace"),

                Value::NodeList(_) | Value::HtmlCollection(_) | Value::ElementChildren(_) => {
                    name == "length"
                }

                _ => false,
            };

            if is_named_property {
                return self.member_value(object, name);
            }
        }

        let index = if matches!(
            &object,
            Value::Attributes(_)
                | Value::HtmlCollection(_)
                | Value::ElementChildren(_)
                | Value::NodeList(_)
        ) {
            let key = match &index {
                Value::String(value) => value.clone(),

                Value::Number(number) => {
                    if let Some(key) = crate::javascript::number::integer_property_key(*number) {
                        key
                    } else {
                        index.to_js_string()
                    }
                }

                Value::Undefined | Value::Null | Value::Boolean(_) => index.to_js_string(),

                _ => {
                    return Err(JsError::unsupported(
                        "collection object property key conversion is not implemented",
                    ));
                }
            };

            match crate::javascript::number::array_index(&key) {
                Some(index) => index,

                None if matches!(&object, Value::NodeList(_)) => {
                    return Ok(Value::Undefined);
                }

                None if matches!(
                    &object,
                    Value::HtmlCollection(_) | Value::ElementChildren(_)
                ) =>
                {
                    return self.member_value(object, &key);
                }

                None => {
                    return Err(JsError::unsupported(
                        "named collection properties are not implemented",
                    ));
                }
            }
        } else {
            let Some(index) = self.primitive_to_number(&index) else {
                return Err(JsError::runtime(
                    "TypeError: collection index must be numeric",
                ));
            };

            if !index.is_finite() || index < 0.0 || index.fract() != 0.0 {
                return Ok(Value::Undefined);
            }

            index as usize
        };

        match object {
            Value::Attributes(node_id) => Ok(self
                .current_document_element(node_id)
                .and_then(|element| {
                    element
                        .attributes
                        .get(index)
                        .map(|attribute| Value::Attribute {
                            node_id,
                            name: attribute.name.clone(),
                        })
                })
                .unwrap_or(Value::Undefined)),

            Value::HtmlCollection(tag_name) => Ok(self
                .current_document()
                .and_then(|document| {
                    document
                        .get_elements_by_tag_name(&tag_name)
                        .get(index)
                        .copied()
                })
                .map(Value::Element)
                .unwrap_or(Value::Undefined)),

            Value::ElementChildren(node_id) => Ok(self
                .current_document_element(node_id)
                .and_then(|element| element.children.get(index).copied())
                .map(Value::Element)
                .unwrap_or(Value::Undefined)),

            Value::NodeList(nodes) => Ok(nodes
                .get(index)
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
    fn canvas_numeric_property_propagates_user_conversion_errors() {
        let mut document = DocumentState::default();
        document.register_parsed_element("canvas", "drawing", String::new(), None);
        let result = evaluate_with_document(
            r#"
                let ctx = document.getElementById('drawing').getContext('2d');
                let calls = 0;
                ctx.lineWidth = { valueOf: function() { calls++; return 3; } };
                let caught = false;
                try { ctx.lineWidth = { valueOf: function() { throw 'boom'; } }; }
                catch (e) { caught = e === 'boom'; }
                ctx.lineWidth === 3 && calls === 1 && caught;
            "#,
            &mut document,
        )
        .unwrap();
        assert_eq!(result, Value::Boolean(true));
    }

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
            crate::javascript::JsErrorKind::NativeException(value)
                if matches!(value.as_test_value(), Value::NativeError(_))
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

    #[test]
    fn reads_supported_named_properties_through_brackets() {
        let result = evaluate(
            "let url = new URL('https://example.org/'); \
            url['protocol'] === url.protocol \
            && 'aba'['replace']('a', 'x') === 'xba';",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));

        let mut document = DocumentState::default();
        document.register_element("welcome", "h1", "Hello".to_owned());

        let result = evaluate_with_document(
            "let elements = document.getElementsByTagName('h1'); \
            elements['length'] === elements.length \
            && elements[0].id === 'welcome';",
            &mut document,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn node_list_indices_require_canonical_property_keys() {
        let mut document = DocumentState::default();
        document.register_element("first", "h1", "First".to_owned());
        document.register_element("second", "h1", "Second".to_owned());

        let result = evaluate_with_document(
            r#"
                let nodes = document.querySelectorAll('h1');

                nodes[0].id === 'first'
                    && nodes['1'].id === 'second'
                    && nodes[-0].id === 'first'
                    && nodes['01'] === undefined
                    && nodes['1.0'] === undefined
                    && nodes['1e0'] === undefined
                    && nodes[''] === undefined
                    && nodes[' 1 '] === undefined
                    && nodes[true] === undefined
                    && nodes[null] === undefined
                    && nodes['-0'] === undefined
                    && nodes['4294967295'] === undefined;
            "#,
            &mut document,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));

        let value =
            evaluate_with_document("document.getElementsByTagName('h1')['01'];", &mut document)
                .unwrap();

        assert_eq!(value, Value::Undefined);
    }

    #[test]
    fn html_collections_lookup_names_in_live_tree_order() {
        let mut document = DocumentState::default();

        let parent = document.register_parsed_element("div", "parent", String::new(), None);

        let first = document.register_parsed_element("span", "first", String::new(), Some(parent));

        let second =
            document.register_parsed_element("span", "shared", String::new(), Some(parent));

        document.register_parsed_element("span", "length", String::new(), Some(parent));

        document.register_element_attribute(first, "name", "shared".to_owned());
        document.register_element_attribute(second, "name", "01".to_owned());

        let result = evaluate_with_document(
            r#"
                let collection = document.getElementsByTagName('span');
                let children = document.getElementById('parent').children;

                let before = collection.shared.id === 'first'
                    && collection['01'].id === 'shared'
                    && collection['0'].id === 'first'
                    && children['01'].id === 'shared'
                    && collection.length === 3
                    && children['length'] === 3
                    && collection[''] === undefined
                    && collection['missing'] === undefined;

                collection.first.setAttribute('name', 'changed');

                before
                    && collection.shared.id === 'shared'
                    && collection.changed.id === 'first'
                    && children.changed.id === 'first';
            "#,
            &mut document,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn html_collection_methods_use_live_receivers_and_webidl_arguments() {
        let mut document = DocumentState::default();

        let parent = document.register_parsed_element("div", "parent", String::new(), None);

        let first = document.register_parsed_element("span", "first", String::new(), Some(parent));

        document.register_parsed_element("span", "length", String::new(), Some(parent));
        document.register_element_attribute(first, "name", "shared".to_owned());

        let result = evaluate_with_document(
            r#"
                let collection = document.getElementsByTagName('span');
                let children = document.getElementById('parent').children;
                let item = collection.item;
                let named = collection.namedItem;

                let detached = false;
                let missing = false;
                let missingName = false;
                let wrongThis = false;

                try { item(0); }
                catch (e) { detached = e.name === 'TypeError'; }

                try { collection.item(); }
                catch (e) { missing = e.name === 'TypeError'; }

                try { collection.namedItem(); }
                catch (e) { missingName = e.name === 'TypeError'; }

                try { named.call({}, 'shared'); }
                catch (e) { wrongThis = e.name === 'TypeError'; }

                let before = collection.item(0).id === 'first'
                    && collection.item(1.9).id === 'length'
                    && collection.item(4294967296).id === 'first'
                    && collection.item(undefined).id === 'first'
                    && collection.item(-1) === null
                    && collection.item(5) === null
                    && collection.namedItem('missing') === null
                    && collection['missing'] === undefined
                    && collection.namedItem('length').id === 'length'
                    && item.call(children, 0).id === 'first'
                    && named.call(children, 'shared').id === 'first';

                collection.first.setAttribute('name', 'changed');

                before
                    && collection.namedItem('shared') === null
                    && named.call(children, 'changed').id === 'first'
                    && detached && missing && missingName && wrongThis;
            "#,
            &mut document,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn get_element_by_id_uses_webidl_arguments_and_document_receiver() {
        let mut document = DocumentState::default();

        for id in ["0", "null", "undefined", "true", "NaN", "Infinity"] {
            document.register_element(id, "div", String::new());
        }

        let result = evaluate_with_document(
            r#"
                let calls = 0;
                let omitted = false;
                let wrongThis = false;
                let detached = false;

                let method = document.getElementById;

                try { document.getElementById(); }
                catch (e) { omitted = e.name === 'TypeError'; }

                try { method.call({}, '0'); }
                catch (e) { wrongThis = e.name === 'TypeError'; }

                try { method('0'); }
                catch (e) { detached = e.name === 'TypeError'; }

                document.getElementById(-0, calls = 1).id === '0'
                    && calls === 1
                    && document.getElementById(null).id === 'null'
                    && document.getElementById(undefined).id === 'undefined'
                    && document.getElementById(true).id === 'true'
                    && document.getElementById(0 / 0).id === 'NaN'
                    && document.getElementById(1 / 0).id === 'Infinity'
                    && method.call(document, '0').id === '0'
                    && document.getElementById('missing') === null
                    && omitted && wrongThis && detached;
            "#,
            &mut document,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));

        for source in ["document.getElementById(1.5);"] {
            let error = evaluate_with_document(source, &mut document).unwrap_err();

            assert_eq!(
                error.kind,
                crate::javascript::JsErrorKind::UnsupportedFeature
            );
        }
    }

    #[test]
    fn dom_string_calls_user_conversion_and_refreshes_live_collections() {
        let mut document = DocumentState::default();

        document.register_element("parent", "div", String::new());
        document.register_element("target", "div", String::new());
        document.register_element("[object Object]", "div", String::new());

        let result = evaluate_with_document(
            r#"
                let trace = '';

                let key = {
                    value: 'target',
                    toString: function() {
                        trace = trace + 's';
                        return this.value;
                    }
                };

                let fallback = {
                    toString: function() {
                        trace = trace + 'a';
                        return {};
                    },
                    valueOf: function() {
                        trace = trace + 'b';
                        return 'target';
                    }
                };

                let plain =
                    document.getElementById({}).id === '[object Object]';

                    let first = document.getElementById(key).id === 'target';
                    let second = document.getElementById(fallback).id === 'target';

                    let caught = false;

                    try {
                        document.getElementById({
                            toString: function() { throw 'boom'; }
                        });
                    } catch (e) {
                        caught = e === 'boom';
                    }

                    let invalid = Object.create(null);
                    invalid.toString = 1;
                    invalid.valueOf = 2;

                    let typeError = false;

                    try {
                        document.getElementById(invalid);
                    } catch (e) {
                        typeError = e.name === 'TypeError';
                    }

                    let collection = document.getElementsByTagName('span');

                    let dynamic = {
                        toString: function() {
                            let element = document.createElement('span');
                            element.id = 'late';

                            document.getElementById('parent').appendChild(element);

                            return 'late';
                        }
                    };

                    plain && first && second
                        && trace === 'sab'
                        && caught && typeError
                        && collection.namedItem(dynamic).id === 'late';
            "#,
            &mut document,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn collection_item_converts_objects_in_number_hint_order() {
        let mut document = DocumentState::default();
        document.register_element("parent", "div", String::new());

        let result = evaluate_with_document(
            r#"
                let collection = document.getElementsByTagName('span');
                let trace = '';

                let index = {
                    valueOf: function() {
                        trace = trace + 'v';

                        let item = document.createElement('span');
                        item.id = 'added';
                        document.getElementById('parent').appendChild(item);

                        return 0;
                    },

                    toString: function() {
                        trace = trace + 's';
                        return '0';
                    }
                };

                let found = collection.item(index).id === 'added'
                    && trace === 'v';

                let fallback = {
                    valueOf: function() {
                        trace = trace + 'a';
                        return {};
                    },

                    toString: function() {
                        trace = trace + 'b';
                        return '0';
                    },
                };

                let second = collection.item(fallback).id === 'added'
                    && trace === 'vab';

                let caught = false;

                try {
                    collection.item({
                        valueOf: function() { throw 'boom'; }
                    });
                } catch (e) {
                    caught = e === 'boom';
                }

                let object = {};

                let builtin =
                    Object.prototype.valueOf.call(object) === object
                    && 'valueOf' in object;

                found && second && caught && builtin;
            "#,
            &mut document,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }
}
