//! Partial construction for ordinary user-defined functions.
//!
//! <https://tc39.es/ecma262/#sec-ecmascript-function-objects-construct-argumentslist-newtarget>
//!
//! Exotic prototype objects, constructor properties and new.target are incomplete.

use super::{Expression, Interpreter, JsError, NativeFunction, Value};
use crate::javascript::FunctionKind;

impl Interpreter {
    pub(super) fn construct_user(
        &mut self,
        name: &str,
        arguments: &[Expression],
    ) -> Result<Value, JsError> {
        let constructor = self
            .variables
            .get(name)
            .map(|value| value.clone())
            .ok_or_else(|| JsError::reference_error(name))?;

        let mut values = Vec::with_capacity(arguments.len());

        for argument in arguments {
            values.push(self.evaluate_expression(argument)?);
        }

        let function = match constructor {
            Value::Function(function) if function.is_bound() => {
                return Err(JsError::unsupported(
                    "bound function construction is not implemented",
                ));
            }

            Value::Function(function) if function.definition().kind == FunctionKind::Ordinary => {
                function
            }

            Value::NativeFunction(NativeFunction::PromiseConstructor) => {
                return Err(JsError::unsupported(
                    "Promise construction is not implemented",
                ));
            }

            Value::NativeFunction(NativeFunction::StringConstructor) => {
                return Err(JsError::unsupported(
                    "String object construction is not implemented",
                ));
            }

            Value::NativeFunction(NativeFunction::ErrorConstructor) => {
                return self.construct_error(values);
            }

            Value::ObjectConstructor | Value::ElementConstructor(_) => {
                return Err(JsError::unsupported(
                    "construction with this built-in is not implemented",
                ));
            }

            _ => {
                return Err(JsError::type_error("this value is not a constructor"));
            }
        };

        let prototype = match function.get("prototype") {
            Value::Object(object) => object,

            Value::Undefined
            | Value::Null
            | Value::Boolean(_)
            | Value::Number(_)
            | Value::String(_) => self.object_prototype.clone(),

            _ => {
                return Err(JsError::unsupported(
                    "constructor prototype of this object type is not implemented",
                ));
            }
        };

        let instance = Value::object_with_prototype(prototype);

        let result =
            self.call_function_value_with_receiver(&function, values, Some(instance.clone()))?;

        match result {
            Value::Undefined
            | Value::Null
            | Value::Boolean(_)
            | Value::Number(_)
            | Value::String(_) => Ok(instance),

            object => Ok(object),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::{JsErrorKind, Value, evaluate};

    #[test]
    fn creates_instance_with_own_and_inherited_properties() {
        let result = evaluate(
            r#"
                function Person(name) {
                    this.name = name;
                }

                Person.prototype.kind = 'human';

                let person = new Person('Ada');

                person.name === 'Ada' &&
                    person.kind === 'human' &&
                    person.hasOwnProperty('name') &&
                    !person.hasOwnProperty('kind');
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn handles_object_and_primitive_returns() {
        let result = evaluate(
            r#"
                function ReturnsObject() {
                    this.own = 1;
                    return { other: 2 };
                }

                function ReturnsPrimitive() {
                    this.own = 3;
                    return 10;
                }

                function ReturnsNull() {
                    this.own = 4;
                    return null;
                }

                function ReturnsArray() {
                    return [5];
                }

                let a = new ReturnsObject();
                let b = new ReturnsPrimitive();
                let c = new ReturnsNull();
                let d = new ReturnsArray();

                a.other === 2 &&
                    !a.hasOwnProperty('own') &&
                    b.own === 3 &&
                    c.own === 4 &&
                    d[0] === 5;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn uses_replaced_prototype_and_falls_back_for_primitive() {
        let result = evaluate(
            r#"
                function F() {}

                F.prototype = { flag: 7 };
                let a = new F();

                F.prototype = null;
                Object.prototype.fallback = 8;
                let b = new F();

                a.flag === 7 &&
                    b.fallback === 8 &&
                    !b.hasOwnProperty('fallback');
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn resolves_constructor_before_evaluating_arguments() {
        let result = evaluate(
            r#"
                function A(x) {
                    this.name = 'A';
                }

                function B(x) {
                    this.name = 'B';
                }

                function swap() {
                    A = B;
                    return 1;
                }

                let instance = new A(swap());

                instance.name === 'A';
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn reports_non_constructors_after_evaluating_arguments() {
        let result = evaluate(
            r#"
                let value = 1;
                let count = 0;
                let name = '';

                try {
                    new value(count = count + 1);
                } catch (e) {
                    name = e.name;
                }

                count === 1 && name === 'TypeError';
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));

        let error = evaluate("let arrow = () => 1; new arrow()").unwrap_err();

        assert!(error.message.starts_with("TypeError:"));

        let error = evaluate("new Missing();").unwrap_err();

        assert!(error.message.starts_with("ReferenceError:"));
    }

    #[test]
    fn user_bindings_override_builtin_constructor_names() {
        let result = evaluate(
            r#"
                function URL() {
                    this.custom = 7;
                }

                let x = new URL();

                x.custom;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Number(7.0));
    }

    #[test]
    fn unsupported_constructor_prototypes_are_explicit() {
        for source in ["function F() {} F.prototype = []; new F();"] {
            let error = evaluate(source).unwrap_err();

            assert_eq!(error.kind, JsErrorKind::UnsupportedFeature, "{source}");
        }
    }

    #[test]
    fn supports_constructor_pattern_used_in_testharness() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        assert!(source.contains("function ShellTestEnvironment()"));
        assert!(source.contains("return new ShellTestEnvironment();"));

        let result = evaluate(
            r#"
                function ShellTestEnvironment() {
                    this.name_counter = 0;
                }

                function create_test_environment() {
                    return new ShellTestEnvironment();
                }

                let environment = create_test_environment();

                environment.name_counter;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Number(0.0));
    }
}
