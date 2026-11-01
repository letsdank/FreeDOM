//! Partial CreateListFromArrayLike support for calls.
//!
//! <https://tc39.es/ecma262/#sec-createlistfromarraylike>
//! <https://tc39.es/ecma262/#sec-tolength>
//!
//! Getters, object-to-primitive conversion and other object kinds are incomplete.

use super::{Interpreter, JsError, ObjectStorage, Value};
use crate::javascript::value::{ObjectState, PropertyLookup};

// This implementation limit avoids unbounded argument allocation.
const MAX_ARGUMENTS: usize = 65_536;

fn ordinary_property(object: &ObjectStorage, name: &str) -> Result<Value, JsError> {
    match ObjectState::lookup(object, name) {
        PropertyLookup::Found(value) => Ok(value),

        PropertyLookup::Absent => Ok(Value::Undefined),

        PropertyLookup::Unknown => Err(JsError::unsupported(
            "array-like property lookup reached an unimplemented prototype",
        )),
    }
}

impl Interpreter {
    pub(super) fn create_list_from_array_like(&self, value: &Value) -> Result<Vec<Value>, JsError> {
        let length = match value {
            Value::Array(array) => array.borrow().values.len(),

            Value::Object(object) => {
                let length = ordinary_property(object, "length")?;

                let number = self.to_number(&length).ok_or_else(|| {
                    JsError::unsupported("array-like length conversion is not implemented")
                })?;

                if number.is_nan() || number <= 0.0 {
                    0
                } else {
                    let integer = number.floor();

                    if !integer.is_finite() || integer > MAX_ARGUMENTS as f64 {
                        return Err(JsError::unsupported("array-like argument limit exceeded"));
                    }

                    integer as usize
                }
            }

            Value::Undefined
            | Value::Null
            | Value::Boolean(_)
            | Value::Number(_)
            | Value::String(_) => {
                return Err(JsError::type_error("argument list must be an object"));
            }

            _ => {
                return Err(JsError::unsupported(
                    "this array-like object is not implemented",
                ));
            }
        };

        if length > MAX_ARGUMENTS {
            return Err(JsError::unsupported("array-like argument limit exceeded"));
        }

        let mut result = Vec::new();

        result
            .try_reserve_exact(length)
            .map_err(|_| JsError::runtime("argument allocation failed"))?;

        for index in 0..length {
            let name = index.to_string();

            let value = match value {
                Value::Array(array) => self.array_property(array.clone(), &name),

                Value::Object(object) => ordinary_property(object, &name)?,

                _ => unreachable!("array-like type was already checked"),
            };

            result.push(value);
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::{JsErrorKind, Value, evaluate};

    #[test]
    fn applies_an_ordinary_array_like_object() {
        let result = evaluate(
            r#"
                let object = { 0: 4, 1: 5, length: 2 };

                function add(a, b) {
                    return this.value + a + b;
                }

                add.apply({ value: 3 }, object);
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Number(12.0));
    }

    #[test]
    fn reads_inherited_and_missing_indices_in_order() {
        let result = evaluate(
            r#"
                let proto = Object.create(null);
                proto.length = 3;
                proto['1'] = 'b';

                let list = Object.create(proto);
                list['0'] = 'a';

                function check(a, b, c) {
                    return a === 'a' &&
                        b === 'b' &&
                        c === undefined;
                }

                check.apply({}, list);
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn converts_supported_length_values() {
        let result = evaluate(
            r#"
                function first(a, b) {
                    return a === 'x' && b === undefined;
                }

                function none(a) {
                    return a === undefined;
                }

                let list = Object.create(null);
                list['0'] = 'x';
                list['1'] = 'y';

                list.length = '1.9';
                let fractional = first.apply({}, list);

                list.length = -2;
                let negative = none.apply({}, list);

                list.length = 'invalid';
                let nan = none.apply({}, list);

                fractional && negative && nan;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn treats_missing_length_as_zero_for_null_prototype() {
        let result = evaluate(
            r#"
                let list = Object.create(null);
                list['0'] = 3;

                function none(a) {
                    return a === undefined;
                }

                none.apply({}, list);
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn reports_unsupported_lengths_and_excessive_sizes() {
        for source in [
            "function f() {} f.apply({}, {length: {}});",
            "function f() {} f.apply({}, {length: 65537});",
            "function f() {} f.apply({}, {length: 'Infinity'});",
        ] {
            let error = evaluate(source).unwrap_err();

            assert_eq!(error.kind, JsErrorKind::UnsupportedFeature, "{source}");
        }
    }

    #[test]
    fn reports_a_genuinely_unknown_prototype() {
        let object = Value::object([("length".to_owned(), Value::Number(1.0))]);

        let error = super::Interpreter::new()
            .create_list_from_array_like(&object)
            .unwrap_err();

        assert_eq!(error.kind, JsErrorKind::UnsupportedFeature);
    }
}
