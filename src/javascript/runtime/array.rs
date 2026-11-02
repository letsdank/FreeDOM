//! Array property and method operations used by the JavaScript interpreter.
//!
//! <https://tc39.es/ecma262/#sec-array-objects>
//!
//! Storage records holes created by array literals and indexed assignment.
//! Length assignment and property descriptors are not implemented.

use super::{Interpreter, JsError, NativeFunction, Value};
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

    pub(super) fn array_get(receiver: ArrayStorage, name: &str) -> Option<Value> {
        if name == "length" {
            return Some(Value::Number(receiver.borrow().values.len() as f64));
        }

        if let Some(index) = Self::array_index(name) {
            let array = receiver.borrow();

            return array
                .present
                .get(index)
                .copied()
                .unwrap_or(false)
                .then(|| array.values[index].clone());
        }

        let own = receiver.borrow().properties.get(name).cloned();

        if let Some(value) = own {
            return Some(value);
        }

        if name == "map" {
            return Some(Value::NativeFunction(NativeFunction::ArrayMap));
        }

        if name == "toString" {
            return Some(Value::NativeFunction(NativeFunction::ArrayToString));
        }

        let method = match name {
            "push" => ArrayMethodKind::Push,
            "pop" => ArrayMethodKind::Pop,
            "join" => ArrayMethodKind::Join,
            _ => return None,
        };

        Some(Value::ArrayMethod { receiver, method })
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

                array
                    .present
                    .try_reserve(additional)
                    .map_err(|_| JsError::runtime("array allocation failed"))?;

                array.values.resize(length, Value::Undefined);
                array.present.resize(length, false);
            }

            array.values[index] = value;
            array.present[index] = true;
        } else {
            array.properties.set(name.to_owned(), value);
        }

        Ok(())
    }

    pub(super) fn array_push(receiver: ArrayStorage, values: Vec<Value>) -> Result<Value, JsError> {
        let mut receiver = receiver.borrow_mut();

        receiver
            .present
            .extend(std::iter::repeat_n(true, values.len()));

        receiver.values.extend(values);

        Ok(Value::Number(receiver.values.len() as f64))
    }

    pub(super) fn array_pop(receiver: ArrayStorage, values: Vec<Value>) -> Result<Value, JsError> {
        if !values.is_empty() {
            return Err(JsError::runtime(
                "TypeError: Array.pop expects no arguments",
            ));
        }

        let mut receiver = receiver.borrow_mut();

        receiver.present.pop();

        Ok(receiver.values.pop().unwrap_or(Value::Undefined))
    }

    pub(super) fn array_join(
        &self,
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

        let length = receiver.borrow().values.len();
        let mut parts = Vec::with_capacity(length);

        for index in 0..length {
            let value = self.array_property(receiver.clone(), &index.to_string());

            parts.push(match value {
                Value::Undefined | Value::Null => String::new(),
                value => value.to_js_string(),
            });
        }

        Ok(Value::String(parts.join(&separator)))
    }

    pub(super) fn array_property(&self, receiver: ArrayStorage, name: &str) -> Value {
        if let Some(value) = Self::array_get(receiver, name) {
            return value;
        }

        match crate::javascript::value::ObjectState::lookup(&self.object_prototype, name) {
            crate::javascript::value::PropertyLookup::Found(value) => value,

            crate::javascript::value::PropertyLookup::Absent
            | crate::javascript::value::PropertyLookup::Unknown => Value::Undefined,
        }
    }

    pub(super) fn array_has_own(receiver: &ArrayStorage, name: &str) -> bool {
        if name == "length" {
            return true;
        }

        let array = receiver.borrow();

        if let Some(index) = Self::array_index(name) {
            return array.present.get(index).copied().unwrap_or(false);
        }

        array.properties.get(name).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::{Interpreter, Value};
    use crate::javascript::evaluate;

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
            Some(Value::Number(7.0))
        );

        assert_eq!(
            Interpreter::array_get(storage.clone(), "0"),
            Some(Value::Number(1.0))
        );

        assert_eq!(
            Interpreter::array_get(storage, "length"),
            Some(Value::Number(1.0))
        );
    }

    #[test]
    fn has_own_distinguishes_holes_and_undefined() {
        let value = evaluate(
            r#"
                let array = [undefined];
                array[3] = undefined;

                array.hasOwnProperty(0) &&
                    !array.hasOwnProperty(1) &&
                    !array.hasOwnProperty(2) &&
                    array.hasOwnProperty(3) &&
                    !array.hasOwnProperty(4) &&
                    array.hasOwnProperty('length') &&
                    !array.hasOwnProperty('push');
            "#,
        )
        .unwrap();

        assert_eq!(value, Value::Boolean(true));
    }

    #[test]
    fn updating_and_popping_holes_keeps_presence_consistent() {
        let value = evaluate(
            r#"
                let array = [];

                array[3] = 3;
                array[1] = undefined;
                array.pop();

                let before =
                    !array.hasOwnProperty(2) &&
                    array.hasOwnProperty(1);

                array.push(undefined);

                before &&
                    array.hasOwnProperty(3) &&
                    !array.hasOwnProperty(2);
            "#,
        )
        .unwrap();

        assert_eq!(value, Value::Boolean(true));
    }

    #[test]
    fn array_inherits_and_can_shadow_object_prototype_properties() {
        let value = evaluate(
            r#"
                Object.prototype['1'] = 7;

                let array = [];
                array[2] = 9;

                let inherited =
                    array[1] === 7 &&
                    !array.hasOwnProperty(1);

                array[1] = undefined;

                inherited &&
                    array[1] === undefined &&
                    array.hasOwnProperty(1);
            "#,
        )
        .unwrap();

        assert_eq!(value, Value::Boolean(true));
    }

    #[test]
    fn array_has_own_named_properties_without_inheriting_them() {
        let value = evaluate(
            r#"
                let array = [];

                array['01'] = undefined;
                array['4294967295'] = 1;

                array.hasOwnProperty('01') &&
                    array.hasOwnProperty('4294967295') &&
                    !array.hasOwnProperty(1) &&
                    !array.hasOwnProperty('hasOwnProperty');
            "#,
        )
        .unwrap();

        assert_eq!(value, Value::Boolean(true));
    }

    #[test]
    fn evaluates_array_has_own_property_fragment_from_testharness() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        assert!(source.contains("if (array.hasOwnProperty(i))"));

        let value = evaluate(
            r#"
                let array = [];
                array[1] = undefined;

                let i = 1;
                let found = false;

                if (array.hasOwnProperty(i)) {
                    found = true;
                }

                found;
            "#,
        )
        .unwrap();

        assert_eq!(value, Value::Boolean(true));
    }
}
