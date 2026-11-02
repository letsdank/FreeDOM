//! Partial Object.prototype.toString support.
//!
//! <https://tc39.es/ecma262/#sec-object.prototype.tostring>
//!
//! Symbol.toStringTag and most host object tags are not implemented.

use super::{Interpreter, JsError, Value};

impl Interpreter {
    pub(super) fn object_to_string(receiver: Option<Value>) -> Result<Value, JsError> {
        let tag = match receiver.unwrap_or(Value::Undefined) {
            Value::Undefined => "Undefined",
            Value::Null => "Null",
            Value::Boolean(_) => "Boolean",
            Value::Number(_) => "Number",
            Value::String(_) => "String",
            Value::Object(_) => "Object",
            Value::Array(_) => "Array",
            Value::NativeError(_) => "Error",
            Value::RegExp(_) => "RegExp",
            Value::Arguments(_) => "Arguments",

            Value::Function(_)
            | Value::NativeFunction(_)
            | Value::ArrayMethod { .. }
            | Value::DocumentMethod(_)
            | Value::ElementMethod(_)
            | Value::TestContextMethod(_)
            | Value::RegExpTest(_)
            | Value::RegExpExec(_)
            | Value::ObjectConstructor
            | Value::ElementConstructor(_) => "Function",

            _ => {
                return Err(JsError::unsupported(
                    "Object.prototype.toString for this host object is not implemented",
                ));
            }
        };

        Ok(Value::String(format!("[object {tag}]")))
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::{JsErrorKind, Value, evaluate};

    #[test]
    fn returns_supported_builtin_tags_through_call() {
        let result = evaluate(
            r#"
                let t = Object.prototype.toString;

                t.call(undefined) === '[object Undefined]' &&
                    t.call(null) === '[object Null]' &&
                    t.call(true) === '[object Boolean]' &&
                    t.call(42) === '[object Number]' &&
                    t.call('x') === '[object String]' &&
                    t.call({}) === '[object Object]' &&
                    t.call(Object.create(null)) === '[object Object]' &&
                    t.call([]) === '[object Array]' &&
                    t.call(function() {}) === '[object Function]' &&
                    t.call(new RegExp('a')) === '[object RegExp]';
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn builtin_is_inherited_non_enumerable_and_can_be_shadowed() {
        let result = evaluate(
            r#"
                let object = {};

                let inherited =
                    object.toString() === '[object Object]' &&
                    ('toString' in object) &&
                    !object.hasOwnProperty('toString') &&
                    Object.keys(Object.prototype).length === 0;

                object.toString = function() {
                    return 'custom';
                };

                inherited && object.toString() === 'custom';
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn preserves_array_to_string_as_distinct_method() {
        let result = evaluate("[1, 2].toString();").unwrap();

        assert_eq!(result, Value::String("1,2".to_owned()));

        let result = evaluate(
            r#"
                let array = [];

                array.toString = function() {
                    return 'own';
                };

                array.toString();
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::String("own".to_owned()));
    }

    #[test]
    fn unsupported_host_tags_are_not_guessed() {
        let error = evaluate("Object.prototype.toString.call(new URL('https://example.org/'));")
            .unwrap_err();

        assert_eq!(error.kind, JsErrorKind::UnsupportedFeature);
    }

    #[test]
    fn executes_class_string_expression_from_testharness() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        assert!(source.contains("var actual = {}.toString.call(object);"));

        let result = evaluate(
            r#"
                let object = [1];
                var actual = {}.toString.call(object);

                actual;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::String("[object Array]".to_owned()));
    }
}
