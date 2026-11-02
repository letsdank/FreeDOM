//! Partial Array.prototype.some support for ordinary arrays.
//!
//! <https://tc39.es/ecma262/#sec-array.prototype.some>
//!
//! Generic array-like receivers, accessors, and full this binding are not implemented.

use super::{Interpreter, JsError, Value};
use crate::javascript::{
    FunctionKind,
    value::{ObjectState, PropertyLookup},
};

impl Interpreter {
    pub(super) fn array_some(
        &mut self,
        receiver: Option<Value>,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        let array = match receiver.unwrap_or(Value::Undefined) {
            Value::Array(array) => array,

            Value::Undefined | Value::Null => {
                return Err(JsError::type_error(
                    "Array.prototype.some receiver is null or undefined",
                ));
            }

            _ => {
                return Err(JsError::unsupported(
                    "Array.prototype.some requires an array receiver",
                ));
            }
        };

        let callback = arguments.first().cloned().unwrap_or(Value::Undefined);

        match &callback {
            Value::Function(_)
            | Value::NativeFunction(_)
            | Value::ArrayMethod { .. }
            | Value::DocumentMethod(_)
            | Value::ElementMethod(_)
            | Value::TestContextMethod(_)
            | Value::RegExpTest(_)
            | Value::RegExpExec(_) => {}

            Value::ObjectConstructor | Value::ElementConstructor(_) => {
                return Err(JsError::unsupported(
                    "Array.prototype.some callback of this callable type is not implemented",
                ));
            }

            _ => {
                return Err(JsError::type_error(
                    "Array.prototype.some callback is not callable",
                ));
            }
        }

        let this_arg = arguments.get(1).cloned().unwrap_or(Value::Undefined);
        let length = array.borrow().values.len();

        for index in 0..length {
            let key = index.to_string();

            let value = match Self::array_get(array.clone(), &key) {
                Some(value) => Some(value),

                None => match ObjectState::lookup(&self.object_prototype, &key) {
                    PropertyLookup::Found(value) => Some(value),

                    PropertyLookup::Absent => None,

                    PropertyLookup::Unknown => {
                        return Err(JsError::unsupported(
                            "Array.prototype.some encountered an unimplemented prototype",
                        ));
                    }
                },
            };

            let Some(value) = value else {
                continue;
            };

            if let Value::Function(function) = &callback {
                let references_this =
                    super::references::referenced_names(function.definition()).contains("this");

                let supported_receiver = matches!(
                    this_arg,
                    Value::Object(_) | Value::Array(_) | Value::Function(_)
                );

                if !function.is_bound()
                    && references_this
                    && (function.definition().kind == FunctionKind::Arrow || !supported_receiver)
                {
                    return Err(JsError::unsupported(
                        "Array.prototype.some callback this binding is not implemented",
                    ));
                }
            }

            let result = self.dispatch_call(
                callback.clone(),
                Some(this_arg.clone()),
                vec![
                    value,
                    Value::Number(index as f64),
                    Value::Array(array.clone()),
                ],
            )?;

            if self.is_truthy(&result) {
                return Ok(Value::Boolean(true));
            }
        }

        Ok(Value::Boolean(false))
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::{JsErrorKind, Value, evaluate};

    #[test]
    fn short_circuits_on_truthy_and_passes_arguments() {
        let result = evaluate(
            r#"
                let values = [1, 2, 3];
                let seen = '';

                let found = values.some(function(value, index, array) {
                    seen += index;
                    return value === 2 && array === values ? 'yes' : '';
                });

                found === true &&
                    seen === '01' &&
                    [].some(function() { return true; }) === false;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn skips_holes_and_observes_inherited_indices() {
        let result = evaluate(
            r#"
                let seen = '';

                [1, , 3].some(function(value, index) {
                    seen += index;
                    return false;
                });

                Object.prototype['1'] = 7;

                let found = [1, , 3].some(function(value) {
                    return value === 7;
                });

                seen === '02' && found;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn snapshots_length_and_observes_mutation() {
        let result = evaluate(
            r#"
                let values = [1, , 3];
                let seen = '';

                let found = values.some(function(value, index, array) {
                    seen += index;

                    if (index === 0) {
                        array[1] = 4;
                        array.push(5);
                    }

                    return false;
                });

                found === false &&
                    seen === '012' &&
                    values.length === 4;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn uses_this_arg_and_propagates_exceptions() {
        let result = evaluate(
            r#"
                let context = { value: 3 };

                let found = [3].some(function(value) {
                    return value === this.value;
                }, context);

                let caught = '';

                try {
                    [1].some(function() {
                        throw 'boom';
                    });
                } catch (error) {
                    caught = error;
                }

                found && caught === 'boom';
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn validates_callback_and_reports_unsupported_receivers() {
        for source in [
            "[].some();",
            "[].some(1);",
            "let f = [].some; f(function() {});",
        ] {
            assert!(
                matches!(
                    evaluate(source).unwrap_err().kind,
                    JsErrorKind::NativeException(_)
                ),
                "{source}"
            );
        }

        for source in [
            "[].some.call({ length: 1 }, function() {});",
            "[1].some(function() { return this.x; });",
        ] {
            assert_eq!(
                evaluate(source).unwrap_err().kind,
                JsErrorKind::UnsupportedFeature,
                "{source}"
            );
        }

        assert_eq!(
            evaluate("[].some(function() { return this.x });").unwrap(),
            Value::Boolean(false)
        );
    }
}
