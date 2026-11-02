//! Partial Array.prototype.map support for ordinary arrays.
//!
//! <https://tc39.es/ecma262/#sec-array.prototype.map>
//!
//! Generic array-like receivers, species constructors, accessors, and full
//! function this binding are not implemented.

use super::{Interpreter, JsError, Value};
use crate::javascript::{
    FunctionKind,
    value::{ObjectState, PropertyLookup},
};

impl Interpreter {
    pub(super) fn array_map(
        &mut self,
        receiver: Option<Value>,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        let array = match receiver.unwrap_or(Value::Undefined) {
            Value::Array(array) => array,

            Value::Undefined | Value::Null => {
                return Err(JsError::type_error(
                    "Array.prototype.map receiver is null or undefined",
                ));
            }

            _ => {
                return Err(JsError::unsupported(
                    "Array.prototype.map requires an array receiver",
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
                    "Array.prototype.map callback of this callable type is not implemented",
                ));
            }

            _ => {
                return Err(JsError::type_error(
                    "Array.prototype.map callback is not callable",
                ));
            }
        }

        let this_arg = arguments.get(1).cloned().unwrap_or(Value::Undefined);

        if let Value::Function(function) = &callback {
            let references_this =
                super::references::referenced_names(function.definition()).contains("this");

            if !function.is_bound() && references_this {
                let supported_receiver = matches!(
                    this_arg,
                    Value::Object(_) | Value::Array(_) | Value::Function(_)
                );

                if function.definition().kind == FunctionKind::Arrow || !supported_receiver {
                    return Err(JsError::unsupported(
                        "Array.prototype.map callback this binding is not implemented",
                    ));
                }
            }
        }

        let length = {
            let state = array.borrow();

            if state.properties.get("constructor").is_some() {
                return Err(JsError::unsupported(
                    "Array.prototype.map species constructors are not implemented",
                ));
            }

            state.values.len()
        };

        let mut mapped = Vec::with_capacity(length);

        for index in 0..length {
            let key = index.to_string();

            let value = match Self::array_get(array.clone(), &key) {
                Some(value) => Some(value),

                None => match ObjectState::lookup(&self.object_prototype, &key) {
                    PropertyLookup::Found(value) => Some(value),

                    PropertyLookup::Absent => None,

                    PropertyLookup::Unknown => {
                        return Err(JsError::unsupported(
                            "Array.prototype.map encountered an unimplemented prototype",
                        ));
                    }
                },
            };

            if let Some(value) = value {
                let result = self.dispatch_call(
                    callback.clone(),
                    Some(this_arg.clone()),
                    vec![
                        value,
                        Value::Number(index as f64),
                        Value::Array(array.clone()),
                    ],
                )?;

                mapped.push(Some(result));
            } else {
                mapped.push(None);
            }
        }

        Ok(Value::array_from_slots(mapped))
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::{JsErrorKind, Value, evaluate};

    #[test]
    fn maps_values_with_index_and_original_array() {
        let result = evaluate(
            r#"
                let source = [2, 3];

                let mapped = source.map(function(value, index, array) {
                    return value + index + (array === source ? 10 : 0);
                });

                mapped !== source &&
                    mapped.length === 2 &&
                    mapped[0] === 12 &&
                    mapped[1] === 14;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn preserves_holes_but_visits_inherited_indices() {
        let result = evaluate(
            r#"
                let visits = '';

                let first = [1, , 3].map(function(x, i) {
                    visits += i;
                    return x * 2;
                });

                let hole = !first.hasOwnProperty('1');

                Object.prototype['1'] = 7;

                let second = [1, , 3].map(function(x) {
                    return x * 2;
                });

                visits === '02' &&
                    first.length === 3 &&
                    hole &&
                    second.hasOwnProperty('1') &&
                    second[1] === 14;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn snapshots_length_but_observes_future_index_changes() {
        let result = evaluate(
            r#"
                let values = [1, , 3];
                let seen = '';

                let mapped = values.map(function(value, index, array) {
                    seen += index;

                    if (index === 0) {
                        array[1] = 4;
                        array.push(5);
                    }

                    return value;
                });

                seen === '012' &&
                    mapped.length === 3 &&
                    mapped[0] === 1 &&
                    mapped[1] === 4 &&
                    mapped[2] === 3 &&
                    values.length === 4;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn observes_removed_indices() {
        let result = evaluate(
            r#"
                let values = [1, 2];
                let seen = '';

                let mapped = values.map(function(value, index, array) {
                    seen += index;
                    array.pop();
                    return value;
                });

                seen === '0' &&
                    mapped.length === 2 &&
                    !mapped.hasOwnProperty('1');
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn supports_object_this_and_propagates_callback_exceptions() {
        let result = evaluate(
            r#"
                let context = { factor: 3 };

                let mapped = [2].map(function(x) {
                    return x * this.factor;
                }, context);

                let caught = '';

                try {
                    [1].map(function() {
                        throw 'boom';
                    });
                } catch (error) {
                    caught = error;
                }

                mapped[0] === 6 && caught === 'boom';
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn checks_callback_and_this_even_for_empty_arrays() {
        for source in [
            "[].map();",
            "[].map(1);",
            "let f = [].map; f(function() {});",
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
            "[1].map(function() { return this.x; });",
            "[1].map(x => this.x, {x: 1});",
            "let a = []; a.constructor = {}; a.map(function() {});",
        ] {
            assert_eq!(
                evaluate(source).unwrap_err().kind,
                JsErrorKind::UnsupportedFeature,
                "{source}"
            );
        }
    }

    #[test]
    fn executes_map_fragment_from_wpt_testharness() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        let fragment = r#"Object.keys(formatEscapeMap).map(k => k === "\\" ? "\\\\" : k).join("")"#;

        assert!(source.contains(fragment));

        let script = format!(
            r#"
                let formatEscapeMap = {{
                    'a': 1,
                    'b': 1
                }};

                {fragment};
            "#
        );

        assert_eq!(evaluate(&script).unwrap(), Value::String("ab".to_owned()));
    }
}
