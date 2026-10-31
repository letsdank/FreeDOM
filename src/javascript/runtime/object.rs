//! Partial Object constructor methods.
//!
//! <https://tc39.es/ecma262/#sec-object.create>

use super::{Interpreter, JsError, Value};

impl Interpreter {
    pub(super) fn object_create(arguments: Vec<Value>) -> Result<Value, JsError> {
        match arguments.first().unwrap_or(&Value::Undefined) {
            Value::Null => {}

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
        }

        if !matches!(arguments.get(1), None | Some(Value::Undefined)) {
            return Err(JsError::unsupported(
                "Object.create property descriptors are not implemented",
            ));
        }

        Ok(Value::object_with_null_prototype())
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
    fn object_prototypes_and_descriptors_are_not_silently_ignored() {
        for source in ["Object.create({});", "Object.create(null, {});"] {
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
}
