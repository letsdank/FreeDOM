//! Partial Array.prototype.slice for arrays and unmapped arguments indices.
//!
//! <https://tc39.es/ecma262/#sec-array.prototype.slice>
//!
//! Generic array-like receivers, object coercion and custom species are incomplete.

use super::{Interpreter, JsError, Value};
use crate::javascript::value::{ObjectState, PropertyLookup};

impl Interpreter {
    fn slice_index(&self, value: &Value, length: usize) -> Result<usize, JsError> {
        let number = self.to_number(value).ok_or_else(|| {
            JsError::unsupported("Array.prototype.slice index conversion is not implemented")
        })?;

        if number.is_nan() || number == 0.0 || number == f64::NEG_INFINITY {
            return Ok(0);
        }

        if number == f64::INFINITY {
            return Ok(length);
        }

        let number = number.trunc();

        if number < 0.0 {
            Ok(((length as f64) + number).max(0.0) as usize)
        } else {
            Ok(number.min(length as f64) as usize)
        }
    }

    pub(super) fn array_slice(
        &mut self,
        receiver: Option<Value>,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        let receiver = receiver.unwrap_or(Value::Undefined);

        let length = match &receiver {
            Value::Array(array) => array.borrow().values.len(),

            Value::Arguments(arguments) => arguments.length(),

            Value::Null | Value::Undefined => {
                return Err(JsError::type_error(
                    "Array.prototype.slice receiver is null or undefined",
                ));
            }

            _ => {
                return Err(JsError::unsupported(
                    "Array.prototype.slice receiver is not implemented",
                ));
            }
        };

        let start = self.slice_index(arguments.first().unwrap_or(&Value::Undefined), length)?;

        let end = match arguments.get(1) {
            None | Some(Value::Undefined) => length,
            Some(value) => self.slice_index(value, length)?,
        };

        let count = end.saturating_sub(start);

        if count > 65_536 {
            return Err(JsError::unsupported(
                "Array.prototype.slice allocation limit exceeded",
            ));
        }

        if let Value::Array(array) = &receiver {
            if array.borrow().properties.get("constructor").is_some() {
                return Err(JsError::unsupported(
                    "Array.prototype.slice species constructors are not implemented",
                ));
            }
        }

        let mut slots = Vec::with_capacity(count);

        for index in start..start + count {
            let value = match &receiver {
                Value::Arguments(arguments) => Some(arguments.get_index(index)?),

                Value::Array(array) => {
                    let key = index.to_string();

                    match Self::array_get(array.clone(), &key) {
                        Some(value) => Some(value),

                        None => match ObjectState::lookup(&self.object_prototype, &key) {
                            PropertyLookup::Found(value) => Some(value),

                            PropertyLookup::Absent => None,

                            PropertyLookup::Unknown => {
                                return Err(JsError::unsupported(
                                    "Array.prototype.slice encountered an unimplemented prototype",
                                ));
                            }
                        },
                    }
                }

                _ => unreachable!(),
            };

            slots.push(value);
        }

        Ok(Value::array_from_slots(slots))
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::{JsErrorKind, Value, evaluate};

    #[test]
    fn shared_array_prototype_and_slice() {
        let result = evaluate(
            r#"
                typeof Array.prototype === 'object' &&
                    Array.prototype === Array.prototype &&
                    Array.prototype.constructor === Array &&
                    Array.prototype.hasOwnProperty('slice') &&
                    Array.prototype.slice === [].slice;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn slices_arrays_and_preserves_holes() {
        let result = evaluate(
            r#"
                let source = [1, , 3, 4];
                let a = source.slice(1, -1);
                let b = source.slice(-2);

                a.length === 2 &&
                    !a.hasOwnProperty('0') &&
                    a[1] === 3 &&
                    b.length === 2 &&
                    b[0] === 3 &&
                    b[1] === 4 &&
                    b !== source;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn slice_reads_unmapped_arguments_and_respects_boundaries() {
        let result = evaluate(
            r#"
                function take(a, b) {
                    let result = Array.prototype.slice.call(arguments, 2);

                    return result.length === 2 &&
                        result[0] === 'x' &&
                        result[1] === 'y';
                }

                take(1, 2, 'x', 'y') &&
                    [1, 2].slice(-100, 100).length === 2 &&
                    [1, 2].slice(1, undefined).length === 1 &&
                    [1, 2].slice(2, 1).length === 0;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));

        let error = evaluate(
            r#"
                function f(a) {
                    return Array.prototype.slice.call(arguments);
                }

                f(1);
            "#,
        )
        .unwrap_err();

        assert_eq!(error.kind, JsErrorKind::UnsupportedFeature);
    }

    #[test]
    fn slice_observes_inherited_array_indices() {
        let result = evaluate(
            r#"
                Object.prototype['1'] = 7;

                let copy = [1, , 3].slice();

                copy.hasOwnProperty('1') && copy[1] === 7;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn unsupported_prototype_mutation_and_species_are_explicit() {
        for source in [
            "Array.prototype.foo = 1;",
            "Array.prototype[0] = 1;",
            "Array.prototype.push(1);",
            "Array.prototype.pop();",
            "let a = [1]; a.constructor = {}; a.slice();",
            "[].slice.call({ length: 1 }, 0);",
        ] {
            assert_eq!(
                evaluate(source).unwrap_err().kind,
                JsErrorKind::UnsupportedFeature,
                "{source}"
            );
        }
    }
}
