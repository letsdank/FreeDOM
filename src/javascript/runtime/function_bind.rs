//! Partial Function.prototype.bind support for ordinary user functions.
//!
//! <https://tc39.es/ecma262/#sec-function.prototype.bind>
//! <https://tc39.es/ecma262/#sec-bound-function-exotic-objects>
//!
//! Construction, lexical this, primitive this conversion and host callables
//! are incomplete.

use super::{FunctionValue, Interpreter, JsError, Value};
use crate::javascript::FunctionKind;

impl Interpreter {
    pub(super) fn function_bind(
        &mut self,
        target: Option<Value>,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        let function = match target.unwrap_or(Value::Undefined) {
            Value::Function(function) => function,

            Value::NativeFunction(_)
            | Value::ArrayMethod { .. }
            | Value::DocumentMethod(_)
            | Value::ElementMethod(_)
            | Value::TestContextMethod(_)
            | Value::RegExpTest(_)
            | Value::RegExpExec(_)
            | Value::ObjectConstructor
            | Value::ElementConstructor(_) => {
                return Err(JsError::unsupported(
                    "binding this callable is not implemented",
                ));
            }

            _ => {
                return Err(JsError::type_error(
                    "Function.prototype.bind receiver is not callable",
                ));
            }
        };

        if function.definition().kind != FunctionKind::Ordinary {
            return Err(JsError::unsupported(
                "binding arrow functions is not implemented",
            ));
        }

        let mut arguments = arguments.into_iter();

        let this_arg = arguments.next().unwrap_or(Value::Undefined);

        Ok(Value::Function(FunctionValue::bind(
            function,
            this_arg,
            arguments.collect(),
        )))
    }

    pub(super) fn call_bound_function(
        &mut self,
        function: &FunctionValue,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        let binding = function
            .binding()
            .expect("bound function must have a binding");

        if !binding.target.is_bound()
            && !matches!(
                binding.this_arg,
                Value::Object(_) | Value::Array(_) | Value::Function(_)
            )
        {
            return Err(JsError::unsupported(
                "bound function this conversion is not implemented",
            ));
        }

        let mut combined = binding.arguments.clone();
        combined.extend(arguments);

        self.dispatch_call(
            Value::Function(binding.target.clone()),
            Some(binding.this_arg.clone()),
            combined,
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::{JsErrorKind, Value, evaluate};

    #[test]
    fn preserves_receiver_and_prepends_arguments() {
        let result = evaluate(
            r#"
                function add(a, b) {
                    return this.value + a + b;
                }

                let object = { value: 10 };
                let bound = add.bind(object, 2);

                bound(3) === 15 &&
                    bound.call({ value: 100 }, 3) === 15 &&
                    bound.apply({ value: 200 }, [3]) === 15;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn rebinding_keeps_first_receiver_and_orders_bound_arguments() {
        let result = evaluate(
            r#"
                function combine(a, b, c) {
                    return this.value + a * 100 + b * 10 + c;
                }

                let first = combine.bind({ value: 1000 }, 1);
                let second = first.bind(null, 2);

                second(3);
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Number(1123.0));
    }

    #[test]
    fn bound_functions_have_fresh_identity_and_no_own_prototype() {
        let result = evaluate(
            r#"
                function f() {}

                let first = f.bind({});
                let alias = first;
                let second = f.bind({});

                typeof first === 'function' &&
                    first === alias &&
                    first !== f &&
                    first !== second &&
                    first.prototype === undefined;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn unsupported_bound_construction_and_callbacks_are_explicit() {
        for source in [
            "function f() {} let bound = f.bind({}); new bound();",
            "function f() {} let bound = f.bind({}); let x = {}; x instanceof bound;",
            "function f() {} let bound = f.bind({}); test(bound);",
            "function f() {} let bound = f.bind(null); bound();",
            "let f = () => 1; f.bind({});",
        ] {
            let error = evaluate(source).unwrap_err();

            assert_eq!(error.kind, JsErrorKind::UnsupportedFeature, "{source}");
        }
    }

    #[test]
    fn executes_bind_pattern_from_testharness() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        assert!(source.contains("}.bind(this))"));

        let result = evaluate(
            r#"
                let receiver = {
                    ready: false,

                    create: function() {
                        return function() {
                            this.ready = true;
                        }.bind(this);
                    }
                };

                let callback = receiver.create();
                callback();

                receiver.ready;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }
}
