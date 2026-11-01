//! Partial Function.prototype.call support.
//!
//! <https://tc39.es/ecma262/#sec-function.prototype.call>
//!
//! Function prototype objects and full this binding are not implemented yet.

use super::{Interpreter, JsError, Value};
use crate::javascript::FunctionKind;

impl Interpreter {
    pub(super) fn function_call(
        &mut self,
        target: Option<Value>,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        let mut arguments = arguments.into_iter();

        let this_arg = arguments.next().unwrap_or(Value::Undefined);
        let arguments: Vec<Value> = arguments.collect();

        match target.unwrap_or(Value::Undefined) {
            Value::Function(function) => {
                if function.is_bound() {
                    return self.dispatch_call(Value::Function(function), None, arguments);
                }

                if function.definition().kind != FunctionKind::Ordinary {
                    return Err(JsError::unsupported(
                        "Function.call on arrow function is not implemented",
                    ));
                }

                if !matches!(
                    this_arg,
                    Value::Object(_) | Value::Array(_) | Value::Function(_)
                ) {
                    return Err(JsError::unsupported(
                        "Function.call this binding is only implemented for objects",
                    ));
                }

                self.call_function_with_receiver(function.definition(), arguments, Some(this_arg))
            }

            Value::NativeFunction(function) => {
                self.dispatch_call(Value::NativeFunction(function), Some(this_arg), arguments)
            }

            Value::ArrayMethod { .. }
            | Value::DocumentMethod(_)
            | Value::ElementMethod(_)
            | Value::TestContextMethod(_)
            | Value::RegExpTest(_)
            | Value::RegExpExec(_)
            | Value::ObjectConstructor
            | Value::ElementConstructor(_) => Err(JsError::unsupported(
                "Function.call for this callable is not implemented",
            )),

            _ => Err(JsError::type_error(
                "Function.prototype.call receiver is not callable",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::{JsErrorKind, Value, evaluate};

    #[test]
    fn invokes_user_function_with_explicit_receiver_and_arguments() {
        let result = evaluate(
            r#"
                function add(a, b) {
                    return this.value + a + b;
                }

                let receiver = { value: 10 };

                add.call(receiver, 2, 3) === 15 &&
                    add['call'](receiver, 1, 2) === 13;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn calls_builtin_has_own_property_with_null_prototype_object() {
        let result = evaluate(
            r#"
                let object = Object.create(null);
                object.answer = undefined;

                Object.prototype.hasOwnProperty.call(object, 'answer') &&
                    !Object.prototype.hasOwnProperty['call'](object, 'missing');
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn call_is_shared_and_can_be_shadowed() {
        let result = evaluate(
            r#"
                let first = function() {};
                let second = function() {};

                let shared = first.call === second.call;

                first.call = undefined;

                shared &&
                    first.call === undefined &&
                    second.call !== undefined;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn rejects_missing_callable_and_preserves_native_errors() {
        let result = evaluate(
            r#"
                let f = function() {};
                let invoke = f.call;
                let name = '';

                try {
                    invoke({});
                } catch (e) {
                    name = e.name;
                }

                name;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::String("TypeError".into()));

        let result = evaluate(
            r#"
                let name = '';

                try {
                    Object.prototype.hasOwnProperty.call(null, 'x');
                } catch (e) {
                    name = e.name;
                }

                name;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::String("TypeError".into()));
    }

    #[test]
    fn unsupported_call_cases_are_not_silently_approximated() {
        for source in [
            "let f = function() {}; f.call(null);",
            "let f = () => 1; f.call({});",
        ] {
            let error = evaluate(source).unwrap_err();

            assert_eq!(error.kind, JsErrorKind::UnsupportedFeature, "{source}",);
        }
    }

    #[test]
    fn executes_call_pattern_used_in_testharness() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        assert!(source.contains("func.call(this);"));

        let result = evaluate(
            r#"
                let receiver = {
                    value: 7,
                    run: function() {
                        let func = function() {
                            this.value = this.value + 1;
                        };

                        func.call(this);
                    }
                };

                receiver.run();
                receiver.value;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Number(8.0));
    }

    #[test]
    fn calls_supported_native_functions_through_common_dispatch() {
        let result = evaluate(
            r#"
                let object = { alpha: 1, beta: 2 };

                let keys = Object.keys.call(null, object);
                let entries = Object.entries['call'](undefined, object);
                let derived = Object.create.call(null, object);

                keys.length === 2 &&
                    keys[0] === 'alpha' &&
                    entries.length === 2 &&
                    entries[1][1] === 2 &&
                    derived.beta === 2;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn call_can_invoke_call_itself() {
        let result = evaluate(
            r#"
                function add(n) {
                    return this.value + n;
                }

                let object = { value: 7 };

                add.call.call(add, object, 5);
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Number(12.0));
    }

    #[test]
    fn native_call_propagates_errors_from_target() {
        let result = evaluate(
            r#"
                let name = '';

                try {
                    Object.create.call(null, 1);
                } catch (error) {
                    name = error.name;
                }

                name;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::String("TypeError".to_owned()));
    }
}
