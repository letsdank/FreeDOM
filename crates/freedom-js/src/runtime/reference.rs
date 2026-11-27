//! Reference preparation, reading, and writing for the JavaScript interpreter.
//!
//! <https://tc39.es/ecma262/#sec-reference-record-specification-type>
//!
//! This is not a complete ECMAScript Reference Record implementation.

use super::{ExecutionFrame, Expression, Interpreter, JsError, PostfixStep, RuntimeRoot, Value};

#[derive(Clone)]
pub(super) enum Reference {
    Binding(String),
    Member { object: Value, property: String },
    Index { object: Value, index: Value },
}

impl Reference {
    pub(super) fn receiver(&self) -> Option<Value> {
        match self {
            Self::Binding(_) => None,

            Self::Member { object, .. } | Self::Index { object, .. } => Some(object.clone()),
        }
    }

    pub(super) fn visit_roots(&self, visit: &mut impl FnMut(RuntimeRoot<'_>)) {
        match self {
            Self::Binding(_) => {}

            Self::Member { object, .. } => {
                visit(RuntimeRoot::Value(object));
            }

            Self::Index { object, index } => {
                visit(RuntimeRoot::Value(object));
                visit(RuntimeRoot::Value(index));
            }
        }
    }
}

impl Interpreter {
    pub(super) fn prepare_reference(&mut self, target: &Expression) -> Result<Reference, JsError> {
        match target {
            Expression::Identifier(name) => Ok(Reference::Binding(name.clone())),

            Expression::PostfixChain { base, steps }
                if matches!(
                    steps.last(),
                    Some(PostfixStep::Member(_) | PostfixStep::Index(_))
                ) =>
            {
                self.evaluate_postfix_reference(base, steps)
            }

            _ => Err(JsError::runtime(
                "TypeError: assignment target is not supported",
            )),
        }
    }

    pub(super) fn read_reference(&mut self, reference: &Reference) -> Result<Value, JsError> {
        match reference {
            Reference::Binding(name) => self
                .variables
                .get(name)
                .map(|value| value.clone())
                .ok_or_else(|| JsError::reference_error(name)),

            Reference::Member {
                object, property, ..
            } => self.member_value(object.clone(), property),

            Reference::Index { object, index } => {
                if matches!(object, Value::Null | Value::Undefined) {
                    return self.index_value(object.clone(), index.clone());
                }
                let index = self.property_key_for_index(index)?;
                self.index_value(object.clone(), index)
            }
        }
    }

    pub(super) fn write_reference(
        &mut self,
        reference: Reference,
        value: Value,
    ) -> Result<(), JsError> {
        match reference {
            Reference::Binding(name) => {
                // Preserve the current implicit-global behavior for unresolved names.
                self.variables.assign_or_insert(name, value);

                Ok(())
            }

            Reference::Member { object, property } => match object {
                Value::Document(document) if property == "title" => {
                    document.borrow_mut().title = value.to_js_string(&self.host_roots);
                    Ok(())
                }

                Value::Null | Value::Undefined => Err(JsError::type_error(
                    "cannot write property of null or undefined",
                )),

                Value::Object(values) => {
                    if !self.set_canvas_property(&values, &property, &value)? {
                        self.host_roots
                            .write_ordinary_property(&values, property, value);
                    }

                    Ok(())
                }

                Value::Function(function) => {
                    function.set(&self.host_roots, property, value);
                    Ok(())
                }

                Value::NativeError(error) => {
                    error.set(&self.host_roots, property, value);
                    Ok(())
                }

                Value::Arguments(_) => Err(JsError::unsupported(
                    "arguments property mutation is not implemented",
                )),

                Value::Element(node_id) if property == "textContent" => self
                    .set_document_element_text_content(
                        node_id,
                        value.to_js_string(&self.host_roots),
                    ),

                Value::Element(node_id) if property == "innerHTML" => self
                    .set_document_element_inner_html(node_id, value.to_js_string(&self.host_roots)),

                Value::Element(node_id) if property == "outerHTML" => self
                    .set_document_element_outer_html(node_id, value.to_js_string(&self.host_roots)),

                Value::Element(node_id) if property == "id" => {
                    self.set_document_element_id(node_id, value.to_js_string(&self.host_roots))
                }

                Value::Element(node_id) if property == "className" => {
                    let class_name = value.to_js_string(&self.host_roots);

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

                Value::Element(node_id)
                    if matches!(property.as_str(), "width" | "height")
                        && self
                            .current_document_element(node_id)
                            .is_some_and(|element| {
                                element.tag_name().eq_ignore_ascii_case("canvas")
                            }) =>
                {
                    self.set_canvas_dimension(node_id, &property, value)
                }

                Value::Attribute { node_id, name } if property == "value" => {
                    let value = value.to_js_string(&self.host_roots);
                    if self
                        .current_document_mut()?
                        .set_node_attribute(node_id, &name, value)
                    {
                        self.reset_canvas_attribute(node_id, &name);
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

                Value::Array(values) => self.set_array_property(values, &property, value),

                Value::Element(_) => Err(JsError::runtime(format!(
                    "Element.{property} is not implemented yet"
                ))),

                _ => Err(JsError::runtime(
                    "TypeError: assignment target is not supported",
                )),
            },

            Reference::Index { object, index } => {
                if matches!(object, Value::Null | Value::Undefined) {
                    return Err(JsError::type_error(
                        "cannot write property of null or undefined",
                    ));
                }

                let property = self
                    .property_key_for_index(&index)?
                    .to_js_string(&self.host_roots);

                self.write_reference(Reference::Member { object, property }, value)
            }
        }
    }

    // GetValue updates a computed reference with its converted property key.
    // https://tc39.es/ecma262/2026/#sec-getvalue
    pub(super) fn read_active_reference(&mut self, index: usize) -> Result<Value, JsError> {
        let reference = self.active_reference(index).clone();
        if let Reference::Index { object, index: key } = reference {
            if matches!(object, Value::Null | Value::Undefined) {
                return self.index_value(object, key);
            }
            let key = self.property_key_for_index(&key)?;
            match self.execution_frames.get_mut(index) {
                Some(ExecutionFrame::Reference(reference)) => {
                    *reference = Reference::Index { object, index: key };
                }
                _ => unreachable!("reference frame must exist"),
            }
        }

        let reference = self.active_reference(index).clone();
        self.read_reference(&reference)
    }

    pub(super) fn with_active_reference<T>(
        &mut self,
        reference: Reference,
        operation: impl FnOnce(&mut Self, usize) -> Result<T, JsError>,
    ) -> Result<T, JsError> {
        let index = self.execution_frames.len();

        self.execution_frames
            .push(ExecutionFrame::Reference(reference));

        let result = operation(self, index);

        match self.execution_frames.pop() {
            Some(ExecutionFrame::Reference(_)) => {}
            _ => unreachable!("reference frame must exist"),
        }

        result
    }

    pub(super) fn active_reference(&self, index: usize) -> &Reference {
        match self.execution_frames.get(index) {
            Some(ExecutionFrame::Reference(reference)) => reference,
            _ => unreachable!("reference frame must exist"),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{DocumentState, Value, evaluate, evaluate_with_document};

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

    #[test]
    fn addition_assignment_returns_updated_value_and_is_right_associative() {
        let result = evaluate(
            "let a = 1; \
            let b = 2; \
            let result = a += b += 3; \
            a === 6 && b === 5 && result === 6;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));

        assert_eq!(
            evaluate("let s = 'hello'; s += ' world'; s;").unwrap(),
            Value::String("hello world".to_owned())
        );
    }

    #[test]
    fn addition_assignment_reads_before_rhs_and_evaluates_reference_once() {
        let result = evaluate(
            "let count = 0; \
            let object = { x: 1 }; \
            function get() { count++; return object; } \
            get().x += (object.x = 10); \
            count === 1 && object.x === 11;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));

        let result = evaluate(
            "let index = 0; \
            let values = [2]; \
            values[index++] += 3; \
            index === 1 && values[0] === 5;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn computed_object_keys_are_converted_in_reference_order() {
        let result = evaluate(
            r#"
                let trace = '';
                let object = {x: 1};
                let marker = {};
                let key = {toString: function() { trace += 'k'; return 'x'; }};
                let fallback = {
                    toString: function() { trace += 's'; return {}; },
                    valueOf: function() { trace += 'v'; return 'x'; }
                };
                let failing = {toString: function() { trace += 'e'; throw marker; }};
                let zeroKey = {toString: function() { return -0; }};
                let numbers = {'0': 5};
                function rhs() { trace += 'r'; return 2; }

                let compound = object[key] += rhs();
                let before = object[key]++;
                let assigned = object[key] = rhs();
                let read = object[fallback];
                let caught = false;
                let nullError = '';
                try { object[failing] += rhs(); } catch (error) { caught = error === marker; }
                try { null[key] = rhs(); } catch (error) { nullError = error.name; }

                trace === 'krkrksver' && compound === 3 && before === 3 &&
                    assigned === 2 && read === 2 && object.x === 2 &&
                    caught && nullError === 'TypeError' && numbers[zeroKey] === 5;
            "#,
        )
        .unwrap();
        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn addition_assignment_read_error_prevents_rhs_evaluation() {
        let result = evaluate(
            "let count = 0; \
            let name = ''; \
            try { null.x += (count = 1); } \
            catch (e) { name = e.name; } \
            count === 0 && name === 'TypeError';",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn supports_compound_arithmetic_assignments() {
        let result = evaluate(
            "let x = 20; \
            x -= 2; \
            x *= 3; \
            x /= 6; \
            x += 1; \
            x;",
        )
        .unwrap();

        assert_eq!(result, Value::Number(10.0));

        let result = evaluate(
            "let a = 24; \
            let b = 3; \
            let r = a /= b += 1; \
            a === 6 && b === 4 && r === 6;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn compound_assignment_evaluates_target_once_and_reads_before_rhs() {
        let result = evaluate(
            "let calls = 0; \
            let o = {x: 2}; \
            function get() { calls++; return o; } \
            get().x *= (o.x = 10); \
            calls === 1 && o.x === 20;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));

        let result = evaluate(
            "let a = [12]; \
            let i = 0; \
            a[i++] /= 3; \
            i === 1 && a[0] === 4;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn compound_read_failure_precedes_rhs() {
        let result = evaluate(
            "let count = 0; \
            let name = ''; \
            try { null.x *= (count = 1); } \
            catch (e) { name = e.name; } \
            count === 0 && name === 'TypeError';",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn divide_assignment_and_equals_regexp_are_distinct() {
        let result = evaluate(
            "let n = 8; \
            n /= 2; \
            let re = /=/; \
            n === 4 && re.test('=');",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn remainder_assignment_reads_target_once_before_rhs() {
        let result = evaluate(
            "let o = {x: 10}; \
            let calls = 0; \
            function get() { calls++; return o; } \
            get().x %= (o.x = 3); \
            calls === 1 && o.x === 1;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn assigns_existing_undefined_and_preserves_implicit_global_behavior() {
        let result = evaluate(
            r#"
                let existing = undefined;
                existing = 1;

                created_by_assignment = 2;

                existing === 1 && created_by_assignment === 2;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn compound_assignment_of_missing_name_fails_before_rhs() {
        let result = evaluate(
            r#"
                let calls = 0;
                let name = '';

                try {
                    missing += (calls = 1);
                } catch (error) {
                    name = error.name;
                }

                calls === 0 && name === 'ReferenceError';
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn parameter_updates_preserve_outer_binding() {
        let result = evaluate(
            r#"
                let value = 9;

                function f(value = 2) {
                    value = value + 1;
                    return value;
                }

                let inner = f(undefined);

                inner === 3 && value === 9;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn computed_property_writes_use_the_same_dom_setters() {
        let mut document = DocumentState::default();
        document.register_element("welcome", "h1", "Before".to_owned());

        let result = evaluate_with_document(
            r#"
                let element = document.getElementById('welcome');
                let property = 'textContent';

                element[property] = 'After';

                element.textContent;
            "#,
            &mut document,
        )
        .unwrap();

        assert_eq!(result, Value::String("After".to_owned()));
    }
}
