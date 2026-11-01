//! Partial Array.prototype.toString support.
//!
//! <https://tc39.es/ecma262/#sec-array.prototype.tostring>
//! <https://tc39.es/ecma262/#sec-array.prototype.join>
//!
//! Primitive boxing, host objects and general array-like joins are incomplete.

use super::{Interpreter, JsError, Value};
use crate::javascript::FunctionKind;
use crate::javascript::value::{ObjectState, PropertyLookup};

impl Interpreter {
    pub(super) fn array_to_string(&mut self, receiver: Option<Value>) -> Result<Value, JsError> {
        let receiver = receiver.unwrap_or(Value::Undefined);

        let join = match &receiver {
            Value::Array(array) => self.array_property(array.clone(), "join"),

            Value::Object(object) => match ObjectState::lookup(object, "join") {
                PropertyLookup::Found(value) => value,

                PropertyLookup::Absent => Value::Undefined,

                PropertyLookup::Unknown => {
                    return Err(JsError::unsupported(
                        "array toString join lookup reached an unimplemented prototype",
                    ));
                }
            },

            Value::Undefined | Value::Null => {
                return Err(JsError::type_error(
                    "Array.prototype.toString requires a non-null value",
                ));
            }

            _ => {
                return Err(JsError::unsupported(
                    "Array.prototype.toString receiver is not implemented",
                ));
            }
        };

        match join {
            Value::ArrayMethod { method, .. } => {
                let Value::Array(array) = &receiver else {
                    return Err(JsError::unsupported(
                        "array methods on array-like objects are not implemented",
                    ));
                };

                let join = Value::ArrayMethod {
                    receiver: array.clone(),
                    method,
                };

                self.dispatch_call(join, Some(receiver), Vec::new())
            }

            Value::Function(function) => {
                if function.definition().kind == FunctionKind::Arrow {
                    return Err(JsError::unsupported(
                        "arrow function join is not implemented",
                    ));
                }

                self.dispatch_call(Value::Function(function), Some(receiver), Vec::new())
            }

            Value::NativeFunction(function) => {
                self.dispatch_call(Value::NativeFunction(function), Some(receiver), Vec::new())
            }

            Value::DocumentMethod(_)
            | Value::ElementMethod(_)
            | Value::TestContextMethod(_)
            | Value::RegExpTest(_)
            | Value::RegExpExec(_)
            | Value::ObjectConstructor
            | Value::ElementConstructor(_) => Err(JsError::unsupported(
                "this join callable is not implemented",
            )),

            _ => Self::object_to_string(Some(receiver)),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::{JsErrorKind, Value, evaluate};

    #[test]
    fn default_array_to_string_uses_join_and_holes() {
        let result = evaluate(
            r#"
                let array = [1, , null, 4];

                array.toString()
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::String("1,,,4".to_owned()));
    }

    #[test]
    fn join_reads_inherited_indices() {
        let result = evaluate(
            r#"
                Object.prototype['1'] = 'inherited';

                let array = [1, , 3];

                array.join('|') === '1|inherited|3' &&
                    array.toString() === '1,inherited,3';
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn calls_custom_join_with_the_original_receiver() {
        let result = evaluate(
            r#"
                let array = [1];

                array.label = 'ok';
                array.join = function() {
                    return this.label;
                };

                array.toString();
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::String("ok".to_owned()));

        let result = evaluate(
            r#"
                let a = [1];
                let b = [2];

                a.join = b.join;

                a.toString();
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::String("1".to_owned()));
    }

    #[test]
    fn non_callable_join_falls_back_to_object_tag() {
        let result = evaluate(
            r#"
                let array = [1];
                array.join = 5;

                Object.prototype.toString = function() {
                    retunr 'overridden';
                };

                array.toString();
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::String("[object Array]".to_owned()));
    }

    #[test]
    fn supports_ordinary_object_receivers_with_custom_or_absent_join() {
        let result = evaluate(
            r#"
                let method = [].toString;

                let object = Object.create(null);
                object.value = 42;
                object.join = function() {
                    return this.value;
                };

                let joined = method.call(object);
                let empty = Object.create(null);

                joined === 42 &&
                    method.call(empty) === '[object Object]';
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn unknown_prototypes_and_unimplemented_boxing_are_explicit() {
        for source in [
            "let method = [].toString; method.call({});",
            "let method = [].toString; method.call(1);",
            "let a = []; a.join = () => 'x'; a.toString();",
        ] {
            let error = evaluate(source).unwrap_err();

            assert_eq!(error.kind, JsErrorKind::UnsupportedFeature, "{source}");
        }
    }
}
