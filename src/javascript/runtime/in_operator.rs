//! Partial implementation of the ECMAScript `in` operator.
//!
//! <https://tc39.es/ecma262/#sec-relational-operators>
//!
//! Built-in prototypes and complete ToPropertyKey conversion are not implemented.

use super::{Interpreter, JsError, Value};
use crate::javascript::value::PropertyLookup;

impl Interpreter {
    pub(super) fn evaluate_in(&mut self, key: Value, target: Value) -> Result<Value, JsError> {
        match target {
            Value::Undefined
            | Value::Null
            | Value::Boolean(_)
            | Value::Number(_)
            | Value::String(_) => Err(JsError::type_error(
                "right-hand side of 'in' must be an object",
            )),

            Value::Object(object) => {
                self.with_frame_value(Value::Object(object), |interpreter, index| {
                    let key = interpreter.property_key_for_lookup(&key)?;
                    let Value::Object(object) = interpreter.frame_value(index).clone() else {
                        unreachable!("object receiver must remain rooted");
                    };

                    match interpreter
                        .host_roots
                        .lookup_ordinary_property(&object, &key)
                    {
                        PropertyLookup::Found(_) => Ok(Value::Boolean(true)),
                        PropertyLookup::Absent => Ok(Value::Boolean(false)),
                        PropertyLookup::Unknown => Err(JsError::unsupported(
                            "'in' lookup reached an unimplemented prototype",
                        )),
                    }
                })
            }

            _ => Err(JsError::unsupported(
                "'in' is not implemented for this object type",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::{JsErrorKind, Value, evaluate};

    #[test]
    fn finds_own_property_with_undefined_value() {
        assert_eq!(
            evaluate("'x' in { x: undefined };").unwrap(),
            Value::Boolean(true)
        );
    }

    #[test]
    fn rejects_primitive_right_operand_with_type_error() {
        for target in ["null", "undefined", "true", "1", "'text'"] {
            let script = format!(
                "let result = ''; \
                try {{ 'x' in {target}; }} \
                catch (e) {{ result = e.name; }} \
                result;"
            );

            assert_eq!(
                evaluate(&script).unwrap(),
                Value::String("TypeError".to_owned())
            );
        }
    }

    #[test]
    fn does_not_guess_the_result_without_prototype_lookup() {
        let error = evaluate("'isPrototypeOf' in {};").unwrap_err();

        assert_eq!(error.kind, JsErrorKind::UnsupportedFeature);
    }

    #[test]
    fn converts_supported_keys_and_rejects_unimplemented_numbers() {
        assert_eq!(
            evaluate("1 in { '1': true };").unwrap(),
            Value::Boolean(true)
        );
        assert_eq!(
            evaluate("1.5 in { '1.5': true };").unwrap(),
            Value::Boolean(true)
        );
        let error = evaluate("0.1 in { '0.1': true };").unwrap_err();

        assert_eq!(error.kind, JsErrorKind::UnsupportedFeature);
    }

    #[test]
    fn direct_and_object_number_keys_match_across_property_operations() {
        let result = evaluate(
            r#"
                let object = {'-1.5': 7, '0': 8, 'NaN': 9, 'Infinity': 10, '-Infinity': 11};
                let fraction = {toString: function() { return -1.5; }};
                let zero = {toString: function() { return -0; }};
                let nan = 0 / 0;
                let inf = 1 / 0;
                let notNumber = {toString: function() { return nan; }};
                let infinity = {toString: function() { return inf; }};

                object[-1.5] === 7 && object[fraction] === 7 &&
                    (-1.5 in object) && (fraction in object) &&
                    object.hasOwnProperty(-1.5) && object.hasOwnProperty(fraction) &&
                    object[0] === 8 && object[zero] === 8 &&
                    object[nan] === 9 && object[notNumber] === 9 &&
                    object[inf] === 10 && object[infinity] === 10 && object[-inf] == 11;
            "#,
        )
        .unwrap();
        assert_eq!(result, Value::Boolean(true));

        for source in [
            "0.1 in {};",
            "let k = {toString: function() { return 0.1; }}; k in {};",
            "({}).hasOwnProperty(0.1);",
            "let k = {toString: function() { return 0.1; }}; ({}).hasOwnProperty(k);",
        ] {
            assert_eq!(
                evaluate(source).unwrap_err().kind,
                JsErrorKind::UnsupportedFeature,
                "{source}"
            );
        }
    }

    #[test]
    fn property_key_conversion_is_shared_and_keeps_receivers_alive() {
        let result = evaluate(
            r#"
                let trace = '';
                let object = {x: 7};
                let key = {toString: function() { trace += 'k'; return 'x'; }};
                let marker = {};
                let failing = {toString: function() { trace += 'e'; throw marker; }};
                let exists = key in object;
                let own = object.hasOwnProperty(key);
                let read = object[key];
                let inError = '';
                let ownError = '';
                let caught = false;
                try { key in null; } catch (error) { inError = error.name; }
                try { Object.prototype.hasOwnProperty.call(null, key); }
                catch (error) { ownError = error.name; }
                try { failing in object; } catch (error) { caught = error === marker; }

                let target = {x: 1};
                let keyForIn = {toString: function() { target = null; return 'x'; }};
                let stillFound = keyForIn in target;
                let receiver = {x: 1};
                let keyForOwn = {toString: function() { receiver = null; return 'x'; }};
                let stillOwn = receiver.hasOwnProperty(keyForOwn);

                trace === 'kkkke' && exists && own && read === 7 && caught &&
                    inError === 'TypeError' && ownError === 'TypeError' &&
                    stillFound && target === null && stillOwn && receiver === null;
            "#,
        )
        .unwrap();
        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn evaluates_actual_testharness_in_statement() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        let fragment = "var has_selector = selector in w;";

        assert!(source.contains(fragment));

        let script = format!(
            "let selector = 'x'; \
            let w = {{ x: 1 }}; \
            {fragment} \
            has_selector;"
        );

        assert_eq!(evaluate(&script).unwrap(), Value::Boolean(true));
    }

    #[test]
    fn known_absent_keys_and_mutated_intrinsic_properties() {
        let result = evaluate(
            "let o = {}; \
            let before = 'document' in o; \
            Object.prototype.document = undefined; \
            before === false && ('document' in o);",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn unimplemented_intrinsic_names_remain_unknown() {
        for name in ["isPrototypeOf", "__proto__", "__defineGetter__"] {
            let source = format!("'{name}' in {{}};");

            assert_eq!(
                evaluate(&source).unwrap_err().kind,
                JsErrorKind::UnsupportedFeature,
                "{name}"
            );
        }
    }
}
