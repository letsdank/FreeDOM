//! Partial Function.prototype.apply support.
//!
//! <https://tc39.es/ecma262/#sec-function.prototype.apply>
//!
//! Only arrays and a subset of ordinary array-like objects are supported.

use super::{Interpreter, JsError, Value};

impl Interpreter {
    pub(super) fn function_apply(
        &mut self,
        target: Option<Value>,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        match target.as_ref().unwrap_or(&Value::Undefined) {
            Value::Function(_) | Value::NativeFunction(_) => {}

            Value::ArrayMethod { .. }
            | Value::DocumentMethod(_)
            | Value::ElementMethod(_)
            | Value::TestContextMethod(_)
            | Value::RegExpTest(_)
            | Value::RegExpExec(_)
            | Value::ObjectConstructor
            | Value::ElementConstructor(_) => {
                return Err(JsError::unsupported(
                    "Function.apply for this callable is not implemented",
                ));
            }

            _ => {
                return Err(JsError::type_error(
                    "Function.prototype.apply receiver is not callable",
                ));
            }
        }

        let this_arg = arguments.first().cloned().unwrap_or(Value::Undefined);
        let mut forwarded = vec![this_arg];

        match arguments.get(1).unwrap_or(&Value::Undefined) {
            Value::Null | Value::Undefined => {}

            value => forwarded.extend(self.create_list_from_array_like(value)?),
        }

        self.function_call(target, forwarded)
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::{JsErrorKind, Value, evaluate};

    #[test]
    fn applies_array_arguments_and_nullish_argument_lists() {
        let result = evaluate(
            r#"
                function add(a, b) {
                    return this.value + a + b;
                }

                function get() {
                    return this.value;
                }

                let obj = { value: 10 };

                add.apply(obj, [2, 3]) === 15 &&
                    get.apply(obj, null) === 10 &&
                    get['apply'](obj) === 10;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn applies_holes_as_undefined_or_inherited_values() {
        let result = evaluate(
            r#"
                function read(a, b) {
                    return a === undefined && b === 7;
                }

                Object.prototype['1'] = 7;

                let args = [, ,];

                read.apply({}, args);
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn applies_to_native_functions_and_composes_with_call() {
        let result = evaluate(
            r#"
                function add(a, b) {
                    return this.value + a + b;
                }

                let obj = { value: 3 };
                let keys = Object.keys.apply(null, [{ x: 1 }]);

                keys[0] === 'x' &&
                    add.call.apply(add, [obj, 4, 5]) === 12;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn checks_callable_before_argument_list_and_rejects_primitives() {
        for source in [
            "let f = function() {}; let o = { run: f.apply }; o.run(null, {});",
            "let f = function() {}; f.apply({}, 5);",
        ] {
            let error = evaluate(source).unwrap_err();

            assert!(
                error.message.starts_with("TypeError:"),
                "{source}: {error:?}"
            );
        }
    }

    #[test]
    fn reports_unimplemented_argument_lists() {
        let error = evaluate("let f = function() {}; f.apply({}, {length: {}});").unwrap_err();

        assert_eq!(error.kind, JsErrorKind::UnsupportedFeature);
    }

    #[test]
    fn matches_apply_pattern_in_testharness() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        assert!(source.contains("func.apply(outer_this, args);"));

        let result = evaluate(
            r#"
                let outer_this = { value: 4 };
                let args = [2, 3];

                let func = function(a, b) {
                    return this.value + a + b;
                };

                func.apply(outer_this, args);
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Number(9.0));
    }
}
