//! Array property and method operations used by the JavaScript interpreter.
//!
//! <https://tc39.es/ecma262/#sec-array-objects>
//!
//! Storage records holes created by array literals and indexed assignment.
//! Array property descriptors and object-to-number conversion are incomplete.

use super::{Interpreter, JsError, NativeFunction, Value};
use crate::javascript::{ArrayMethodKind, ArrayStorage};

const MAX_ARRAY_LENGTH: usize = 65_536;

impl Interpreter {
    /// Implements the supported cases of Array and new Array.
    ///
    /// <https://tc39.es/ecma262/#sec-array-constructor>
    pub(super) fn array_constructor(arguments: Vec<Value>) -> Result<Value, JsError> {
        if let [Value::Number(length)] = arguments.as_slice() {
            if !length.is_finite()
                || *length < 0.0
                || length.fract() != 0.0
                || *length > u32::MAX as f64
            {
                return Err(JsError::unsupported(
                    "Array invalid-length RangeError is not implemented",
                ));
            }

            if *length > MAX_ARRAY_LENGTH as f64 {
                return Err(JsError::unsupported("Array allocation limit exceeded"));
            }

            return Ok(Value::array_from_slots(vec![None; *length as usize]));
        }

        Ok(Value::array(arguments))
    }

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

        if name == "some" {
            return Some(Value::NativeFunction(NativeFunction::ArraySome));
        }

        if name == "slice" {
            return Some(Value::NativeFunction(NativeFunction::ArraySlice));
        }

        if name == "constructor" {
            return Some(Value::NativeFunction(NativeFunction::ArrayConstructor));
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

    pub(super) fn ensure_array_prototype_is_not_mutated(
        &self,
        array: &ArrayStorage,
    ) -> Result<(), JsError> {
        if std::rc::Rc::ptr_eq(array, &self.array_prototype) {
            return Err(JsError::unsupported(
                "Array.prototype mutation is not implemented",
            ));
        }

        Ok(())
    }

    pub(super) fn set_array_property(
        &self,
        receiver: ArrayStorage,
        name: &str,
        value: Value,
    ) -> Result<(), JsError> {
        self.ensure_array_prototype_is_not_mutated(&receiver)?;

        if name != "length" {
            return Self::array_set(receiver, name, value);
        }

        let number = self.to_number(&value).ok_or_else(|| {
            JsError::unsupported("Array.length object-to-number conversion is not implemented")
        })?;

        if !number.is_finite() || number < 0.0 || number.fract() != 0.0 || number > u32::MAX as f64
        {
            return Err(JsError::unsupported(
                "Array.length invalid-length RangeError is not implemented",
            ));
        }

        let length = number as usize;
        let mut array = receiver.borrow_mut();
        let current_length = array.values.len();

        if length > current_length {
            if length > MAX_ARRAY_LENGTH {
                return Err(JsError::unsupported(
                    "Array.length allocation limit exceeded",
                ));
            }

            let additional = length - current_length;

            array
                .values
                .try_reserve(additional)
                .map_err(|_| JsError::unsupported("Array.length allocation failed"))?;

            array
                .present
                .try_reserve(additional)
                .map_err(|_| JsError::unsupported("Array.length allocation failed"))?;
        }

        array.values.resize(length, Value::Undefined);
        array.present.resize(length, false);

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
    use crate::javascript::{JsErrorKind, evaluate};

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

    #[test]
    fn constructs_array_elements_and_holes() {
        let result = evaluate(
            r#"
                let a = Array(2);
                let b = new Array('x', 'y');
                let c = Array(undefined);
                let d = new Array();

                typeof Array === 'function' &&
                    a.length === 2 &&
                    !a.hasOwnProperty('0') &&
                    b.length === 2 &&
                    b[1] === 'y' &&
                    c.length === 1 &&
                    c.hasOwnProperty('0') &&
                    d.length === 0;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn array_constructor_reports_limits_and_unimplemented_prototype() {
        for source in ["Array(-1);", "new Array(1.5);", "Array(65537);"] {
            assert_eq!(
                evaluate(source).unwrap_err().kind,
                JsErrorKind::UnsupportedFeature,
                "{source}"
            );
        }
    }

    #[test]
    fn sets_length_shrinks_and_regrows_without_restoring_elements() {
        let result = evaluate(
            r#"
                let a = [10, 20, 30];
                a.extra = 9;

                let assigned = (a.length = 1);
                a['length'] = '3';

                assigned === 1 &&
                    a.length === 3 &&
                    a[0] === 10 &&
                    !a.hasOwnProperty('1') &&
                    !a.hasOwnProperty('2') &&
                    a.extra === 9;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn sets_length_from_supported_primitive_values() {
        let result = evaluate(
            r#"
                let a = [7, 8];

                a.length = true;
                let first = a.length === 1 && a[0] === 7;

                a.length = null;
                a.length = 2;

                first &&
                    a.length === 2 &&
                    !a.hasOwnProperty('0') &&
                    !a.hasOwnProperty('1');
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn rejects_invalid_or_unsupported_lengths_without_mutation() {
        let interpreter = Interpreter::new();

        let Value::Array(array) = Value::array(vec![Value::Number(1.0), Value::Number(2.0)]) else {
            unreachable!();
        };

        for value in [
            Value::Number(-1.0),
            Value::Number(1.5),
            Value::Number(f64::NAN),
            Value::Number(f64::INFINITY),
            Value::Number(4_294_967_296.0),
            Value::Number(65_537.0),
            Value::Undefined,
            Value::object([]),
        ] {
            let error = interpreter
                .set_array_property(array.clone(), "length", value)
                .unwrap_err();

            assert_eq!(error.kind, JsErrorKind::UnsupportedFeature);

            let state = array.borrow();

            assert_eq!(state.values, vec![Value::Number(1.0), Value::Number(2.0)]);
            assert_eq!(state.present, vec![true, true]);
        }
    }
}
