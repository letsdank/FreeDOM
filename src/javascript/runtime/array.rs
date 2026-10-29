//! Array property and method operations used by the JavaScript interpreter.
//!
//! <https://tc39.es/ecma262/#sec-array-objects>
//!
//! This implementation uses dense storage and does not yet implement array
//! length assignment, property descriptors, or sparse array holes.

use super::{Interpreter, JsError, Value};
use crate::javascript::{ArrayMethodKind, ArrayStorage};

impl Interpreter {
    // ECMAScript array indices exclude 2^32 - 1 and non-canonical strings.
    // https://tc39.es/ecma262/#array-index
    fn array_index(name: &str) -> Option<usize> {
        if name.is_empty()
            || (name.len() > 1 && name.starts_with('0'))
            || !name.bytes().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }

        let index = name.parse::<u32>().ok()?;

        if index == u32::MAX {
            return None;
        }

        usize::try_from(index).ok()
    }

    pub(super) fn array_get(receiver: ArrayStorage, name: &str) -> Value {
        if name == "length" {
            return Value::Number(receiver.borrow().values.len() as f64);
        }

        if let Some(index) = Self::array_index(name) {
            return receiver
                .borrow()
                .values
                .get(index)
                .cloned()
                .unwrap_or(Value::Undefined);
        }

        let own = receiver.borrow().properties.get(name).cloned();

        if let Some(value) = own {
            return value;
        }

        let method = match name {
            "push" => ArrayMethodKind::Push,
            "pop" => ArrayMethodKind::Pop,
            "join" => ArrayMethodKind::Join,
            _ => return Value::Undefined,
        };

        Value::ArrayMethod { receiver, method }
    }

    pub(super) fn array_set(
        receiver: ArrayStorage,
        name: &str,
        value: Value,
    ) -> Result<(), JsError> {
        if name == "length" {
            return Err(JsError::runtime(
                "Array.length assignment is not implemented yet",
            ));
        }

        let mut array = receiver.borrow_mut();

        if let Some(index) = Self::array_index(name) {
            let length = index
                .checked_add(1)
                .ok_or_else(|| JsError::runtime("array index is too large"))?;

            if length > array.values.len() {
                let additional = length - array.values.len();

                array
                    .values
                    .try_reserve(additional)
                    .map_err(|_| JsError::runtime("array allocation failed"))?;

                array.values.resize(length, Value::Undefined);
            }

            array.values[index] = value;
        } else {
            array.properties.set(name.to_owned(), value);
        }

        Ok(())
    }

    pub(super) fn array_push(receiver: ArrayStorage, values: Vec<Value>) -> Result<Value, JsError> {
        let mut receiver = receiver.borrow_mut();

        receiver.values.extend(values);

        Ok(Value::Number(receiver.values.len() as f64))
    }

    pub(super) fn array_pop(receiver: ArrayStorage, values: Vec<Value>) -> Result<Value, JsError> {
        if !values.is_empty() {
            return Err(JsError::runtime(
                "TypeError: Array.pop expects no arguments",
            ));
        }

        Ok(receiver
            .borrow_mut()
            .values
            .pop()
            .unwrap_or(Value::Undefined))
    }

    pub(super) fn array_join(
        receiver: ArrayStorage,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        if arguments.len() > 1 {
            return Err(JsError::runtime(
                "TypeError: Array.join expects zero or one argument",
            ));
        }

        let separator = match arguments.first() {
            None | Some(Value::Undefined) => ",".to_owned(),
            Some(value) => value.to_js_string(),
        };

        let joined = receiver
            .borrow()
            .values
            .iter()
            .map(|value| match value {
                Value::Undefined | Value::Null => String::new(),
                value => value.to_js_string(),
            })
            .collect::<Vec<_>>()
            .join(&separator);

        Ok(Value::String(joined))
    }
}

#[cfg(test)]
mod tests {
    use super::{Interpreter, Value};

    #[test]
    fn recognizes_canonical_array_indices() {
        for (name, expected) in [
            ("0", Some(0)),
            ("1", Some(1)),
            ("01", None),
            ("-0", None),
            ("1.0", None),
            ("4294967295", None),
            ("", None),
        ] {
            assert_eq!(Interpreter::array_index(name), expected, "{name:?}");
        }

        assert_eq!(
            Interpreter::array_index("4294967294"),
            usize::try_from(u32::MAX - 1).ok(),
        );
    }

    #[test]
    fn keeps_named_properties_separate_from_elements() {
        let Value::Array(storage) = Value::array(vec![Value::Number(1.0)]) else {
            unreachable!();
        };

        Interpreter::array_set(storage.clone(), "01", Value::Number(7.0)).unwrap();

        assert_eq!(
            Interpreter::array_get(storage.clone(), "01"),
            Value::Number(7.0)
        );

        assert_eq!(
            Interpreter::array_get(storage.clone(), "0"),
            Value::Number(1.0)
        );

        assert_eq!(
            Interpreter::array_get(storage, "length"),
            Value::Number(1.0)
        );
    }
}
