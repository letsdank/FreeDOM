//! Partial Object constructor methods.
//!
//! <https://tc39.es/ecma262/#sec-object.create>
//! <https://tc39.es/ecma262/#sec-object.prototype.hasownproperty>

use super::{Interpreter, JsError, Value};

impl Interpreter {
    pub(super) fn object_has_own_property(
        receiver: Option<Value>,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        let key = match arguments.first().unwrap_or(&Value::Undefined) {
            Value::String(value) => value.clone(),

            Value::Undefined => "undefined".to_owned(),

            Value::Null => "null".to_owned(),

            Value::Boolean(value) => value.to_string(),

            Value::Number(value) => crate::javascript::number::integer_property_key(*value)
                .ok_or_else(|| {
                    JsError::unsupported("numeric property key conversion is not implemented")
                })?,

            _ => {
                return Err(JsError::unsupported(
                    "object property key conversion is not implemented",
                ));
            }
        };

        match receiver.unwrap_or(Value::Undefined) {
            Value::Object(object) => Ok(Value::Boolean(object.borrow().get(&key).is_some())),

            Value::Array(array) => Ok(Value::Boolean(Self::array_has_own(&array, &key))),

            Value::NativeError(error) => Ok(Value::Boolean(error.has_own_property(&key))),

            Value::Null | Value::Undefined => Err(JsError::type_error(
                "Object.prototype.hasOwnProperty requires a non-null value",
            )),

            _ => Err(JsError::unsupported(
                "Object.prototype.hasOwnProperty is only implemented for ordinary objects",
            )),
        }
    }

    pub(super) fn object_create(arguments: Vec<Value>) -> Result<Value, JsError> {
        let prototype = match arguments.first().unwrap_or(&Value::Undefined) {
            Value::Null => None,

            Value::Object(object) => Some(object.clone()),

            Value::Undefined | Value::Boolean(_) | Value::Number(_) | Value::String(_) => {
                return Err(JsError::type_error(
                    "Object.create prototype must be an object or null",
                ));
            }

            _ => {
                return Err(JsError::unsupported(
                    "Object.create with an object prototype is not implemented",
                ));
            }
        };

        if !matches!(arguments.get(1), None | Some(Value::Undefined)) {
            return Err(JsError::unsupported(
                "Object.create property descriptors are not implemented",
            ));
        }

        Ok(match prototype {
            Some(object) => Value::object_with_prototype(object),
            None => Value::object_with_null_prototype(),
        })
    }

    // https://tc39.es/ecma262/#sec-object.entries
    pub(super) fn object_entries(arguments: Vec<Value>) -> Result<Value, JsError> {
        let object = match arguments.first().unwrap_or(&Value::Undefined) {
            Value::Null | Value::Undefined => {
                return Err(JsError::type_error(
                    "Object.entries requires a non-null value",
                ));
            }

            Value::Object(object) => object,

            _ => {
                return Err(JsError::unsupported(
                    "Object.entries is only implemented for ordinary objects",
                ));
            }
        };

        let object = object.borrow();

        let entries = object
            .keys()
            .into_iter()
            .map(|key| {
                let value = object.get(&key).expect("own key must exist").clone();

                Value::array(vec![Value::String(key), value])
            })
            .collect();

        Ok(Value::array(entries))
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::{JsErrorKind, Value, evaluate};

    #[test]
    fn supports_null_prototype_property_lookup() {
        let result = evaluate(
            "let o = Object.create(null); \
            let before = 'x' in o; \
            o.x = undefined; \
            before === false && \
            ('x' in o) && \
            ('toString' in o) === false;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn validates_prototype_and_accepts_undefined_descriptors() {
        for prototype in ["undefined", "true", "1", "'x'"] {
            let script = format!(
                "let name = ''; \
                try {{ Object.create({prototype}); }} \
                catch (e) {{ name = e.name; }} \
                name;"
            );

            assert_eq!(
                evaluate(&script).unwrap(),
                Value::String("TypeError".to_owned())
            );
        }

        assert_eq!(
            evaluate("'x' in Object.create(null, undefined);").unwrap(),
            Value::Boolean(false)
        );
    }

    #[test]
    fn unsupported_prototypes_and_descriptors_are_not_silently_ignored() {
        for source in ["Object.create([]);", "Object.create(null, {});"] {
            let error = evaluate(source).unwrap_err();

            assert_eq!(error.kind, JsErrorKind::UnsupportedFeature);
        }
    }

    #[test]
    fn executes_a_statement_from_the_real_testharness() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        let fragment = "var seen_func_name = Object.create(null);";

        assert!(source.contains(fragment));

        let script = format!("{fragment} 'x' in seen_func_name;");

        assert_eq!(evaluate(&script).unwrap(), Value::Boolean(false));
    }

    #[test]
    fn inherits_properties_and_observes_prototype_changes() {
        let result = evaluate(
            "let root = Object.create(null); \
            root.x = 1; \
            let middle = Object.create(root); \
            let leaf = Object.create(middle); \
            let before = leaf.x; \
            root.x = 2; \
            let after = leaf['x']; \
            leaf.x = 3; \
            before === 1 && \
            after === 2 && \
            root.x === 2 && \
            leaf.x === 3 && \
            ('x' in leaf) && \
            ('missing' in leaf) === false;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn unknown_built_in_prototype_remains_explicit_for_in() {
        let result = evaluate(
            "let p = { x: undefined }; \
            let o = Object.create(p); \
            'x' in o;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));

        let error = evaluate("'valueOf' in Object.create({});").unwrap_err();

        assert_eq!(error.kind, JsErrorKind::UnsupportedFeature);
    }

    #[test]
    fn entries_returns_own_keys_and_values_in_property_order() {
        let result = evaluate(
            "let p = Object.create(null); \
            p.inherited = 99; \
            let o = Object.create(p); \
            o['10'] = 'ten'; \
            o['2'] = 'two'; \
            o['01'] = 'zero-one'; \
            o.x = undefined; \
            let e = Object.entries(o); \
            e.length === 4 && \
            e[0][0] === '2' && \
            e[0][1] === 'two' && \
            e[1][0] === '10' && \
            e[2][0] === '01' && \
            e[3][0] === 'x' && \
            e[3][1] === undefined && \
            Object.keys(o).join(',') === '2,10,01,x';",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn entries_preserves_object_identity_and_creates_fresh_arrays() {
        let result = evaluate(
            "let v = {}; \
            let o = {v}; \
            let a = Object.entries(o); \
            let b = Object.entries(o); \
            a !== b && a[0] !== b[0] && a[0][1] === v;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn entries_validates_target_and_evaluates_extra_arguments() {
        for target in ["null", "undefined"] {
            let script = format!(
                "let name = ''; \
                try {{ Object.entries({target}); }} \
                catch (e) {{ name = e.name; }} \
                name;"
            );

            assert_eq!(
                evaluate(&script).unwrap(),
                Value::String("TypeError".to_owned())
            );
        }

        let error = evaluate("Object.entries([]);").unwrap_err();

        assert_eq!(error.kind, JsErrorKind::UnsupportedFeature);

        let result = evaluate(
            "let n = 0; \
            let e = Object.entries({}, n = 1); \
            n === 1 && e.length === 0;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn runs_object_entries_expression_from_testharness() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        assert!(source.contains("Object.entries(required_props)"));

        let result = evaluate(
            "const required_props = { \
                code: 22, \
                name: 'QuotaExceededError' \
            }; \
            let entries = Object.entries(required_props); \
            entries[0][0] === 'code' && \
            entries[0][1] === 22 && \
            entries[1][0] === 'name' && \
            entries[1][1] === 'QuotaExceededError';",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn ordinary_literals_share_a_mutable_object_prototype() {
        let result = evaluate(
            "let o = {}; \
            let prototype = Object.prototype; \
            prototype.marker = 7; \
            o.marker === 7 && \
            ('marker' in o) && \
            Object['prototype'] === prototype && \
            Object.keys(o).length === 0;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn function_prototype_objects_inherit_shared_object_prototype() {
        let result = evaluate(
            "function F() {} \
            let p = F.prototype; \
            Object.prototype.marker = 7; \
            let before = p.marker; \
            p.marker = 9; \
            before === 7 && \
            Object.prototype.marker === 7 && \
            p.marker === 9;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn null_prototype_objects_remain_unaffected() {
        let result = evaluate(
            "let bare = Object.create(null); \
            Object.prototype.marker = 7; \
            ('marker' in bare) === false && \
            bare.marker === undefined;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn prototype_is_per_interpreter() {
        let first = super::Interpreter::new();
        let second = super::Interpreter::new();

        assert!(!std::rc::Rc::ptr_eq(
            &first.object_prototype,
            &second.object_prototype
        ));
    }

    #[test]
    fn testharness_global_scope_probe_detects_absent_document() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        assert!(source.contains("'document' in global_scope"));

        let result = evaluate(
            "let global_scope = {}; \
            'document' in global_scope;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(false));
    }

    #[test]
    fn object_prototype_constructor_is_inherited_but_not_enumerated() {
        let result = evaluate(
            "let o = {}; \
            Object.prototype.constructor === Object && \
            o.constructor === Object && \
            ('constructor' in o) && \
            Object.keys(Object.prototype).length === 0 && \
            Object.entries(Object.prototype).length === 0;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn replacing_constructor_preserves_enumerability_and_allows_shadowing() {
        let result = evaluate(
            "let o = {}; \
            Object.prototype.constructor = 7; \
            let inherited = o.constructor; \
            o.constructor = 8; \
            inherited === 7 && \
            o.constructor === 8 && \
            Object.prototype.constructor === 7 && \
            Object.keys(Object.prototype).length === 0 && \
            Object.keys(o).join(',') === 'constructor';",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn null_prototype_has_no_constructor_and_realms_are_isolated() {
        let result = evaluate(
            "let bare = Object.create(null); \
            bare.constructor === undefined && \
            ('constructor' in bare) === false;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));

        evaluate("Object.prototype.constructor = 1").unwrap();

        assert_eq!(
            evaluate("Object.prototype.constructor === Object;").unwrap(),
            Value::Boolean(true)
        );
    }

    #[test]
    fn runs_constructor_comparison_derived_from_testharness() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        assert!(source.contains("e.constructor === constructor"));

        let result = evaluate(
            "let e = {}; \
            let constructor = Object; \
            e.constructor === constructor;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn has_own_property_does_not_follow_the_prototype_chain() {
        let result = evaluate(
            r#"
                let parent = { inherited: 1 };
                let child = Object.create(parent);
                child.own = undefined;

                child.hasOwnProperty('own') &&
                    !child.hasOwnProperty('inherited') &&
                    !child.hasOwnProperty('missing');
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn has_own_property_is_shared_and_not_enumerable() {
        let result = evaluate(
            r#"
                let object = {};

                object.hasOwnProperty === Object.prototype.hasOwnProperty &&
                    !object.hasOwnProperty('hasOwnProperty') &&
                    Object.prototype.hasOwnProperty('hasOwnProperty') &&
                    Object.keys(Object.prototype).length === 0 &&
                    ('hasOwnProperty' in object);
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn has_own_property_converts_supported_keys_and_uses_undefined_by_default() {
        let result = evaluate(
            r#"
                let object = {};

                object['undefined'] = 1;
                object['null'] = 2;
                object['true'] = 3;
                object['0'] = 4;
                object['12'] = 5;

                object.hasOwnProperty() &&
                    object.hasOwnProperty(null) &&
                    object.hasOwnProperty(true) &&
                    object.hasOwnProperty(-0) &&
                    object.hasOwnProperty(12);
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn detached_has_own_property_requires_a_receiver() {
        let result = evaluate(
            r#"
                let f = Object.prototype.hasOwnProperty;
                let name = '';

                try {
                    f('x');
                } catch (error) {
                    name = error.name;
                }

                name;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::String("TypeError".to_owned()));
    }

    #[test]
    fn unsupported_has_own_property_keys_are_not_silently_coerced() {
        for source in ["({}).hasOwnProperty({});", "({}).hasOwnProperty(1.5);"] {
            let error = evaluate(source).unwrap_err();

            assert_eq!(error.kind, JsErrorKind::UnsupportedFeature);
        }
    }

    #[test]
    fn runs_has_own_property_condition_from_testharness() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        assert!(source.contains("if (properties.hasOwnProperty(\"message_events\"))"));

        let result = evaluate(
            r#"
                let properties = { message_events: undefined };
                let found = false;

                if (properties.hasOwnProperty("message_events")) {
                    found = true;
                }

                found;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }
}
