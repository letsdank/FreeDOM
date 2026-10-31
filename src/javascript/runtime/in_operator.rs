//! Partial implementation of the ECMAScript `in` operator.
//!
//! <https://tc39.es/ecma262/#sec-relational-operators>
//!
//! Built-in prototypes and general ToPropertyKey conversion are not implemented.

use super::{Interpreter, JsError, Value};
use crate::javascript::value::{ObjectState, PropertyLookup};

impl Interpreter {
    pub(super) fn evaluate_in(key: Value, target: Value) -> Result<Value, JsError> {
        match target {
            Value::Undefined
            | Value::Null
            | Value::Boolean(_)
            | Value::Number(_)
            | Value::String(_) => Err(JsError::type_error(
                "right-hand side of 'in' must be an object",
            )),

            Value::Object(object) => {
                let Value::String(key) = key else {
                    return Err(JsError::unsupported(
                        "'in' property key conversion is not implemented",
                    ));
                };

                match ObjectState::lookup(&object, &key) {
                    PropertyLookup::Found(_) => Ok(Value::Boolean(true)),

                    PropertyLookup::Absent => Ok(Value::Boolean(false)),

                    PropertyLookup::Unknown => Err(JsError::unsupported(
                        "'in' lookup reached an unimplemented prototype",
                    )),
                }
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
        let error = evaluate("'toString' in {};").unwrap_err();

        assert_eq!(error.kind, JsErrorKind::UnsupportedFeature);
    }

    #[test]
    fn does_not_guess_how_to_convert_property_keys() {
        let error = evaluate("1 in { '1': true };").unwrap_err();

        assert_eq!(error.kind, JsErrorKind::UnsupportedFeature);
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
}
