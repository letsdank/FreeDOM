//! Value comparison and conversion helpers for the JavaScript interpreter.
//!
//! <https://tc39.es/ecma262/#sec-type-conversion>
//! <https://tc39.es/ecma262/#sec-testing-and-comparison-operations>
//!
//! These operations currently implement only a subset of ECMAScript semantics.

use super::{Interpreter, JsError, Value};
use crate::value::PropertyLookup;

#[derive(Clone, Copy)]
pub(super) enum PrimitiveHint {
    String,
    Number,
}

impl Interpreter {
    // Partial ToPropertyKey for computed references and property lookups.
    // https://tc39.es/ecma262/2026/#sec-topropertykey
    pub(super) fn property_key_for_index(&mut self, index: &Value) -> Result<Value, JsError> {
        let primitive = match index {
            Value::Object(_) | Value::NativeError(_) => {
                self.ordinary_to_primitive(index, PrimitiveHint::String)?
            }
            _ => index.clone(),
        };

        match primitive {
            Value::Number(number) => Ok(match crate::number::number_property_key(number) {
                Some(key) => Value::String(key),
                None => Value::Number(number),
            }),
            value => Ok(value),
        }
    }

    pub(super) fn property_key_for_lookup(&mut self, value: &Value) -> Result<String, JsError> {
        match self.property_key_for_index(value)? {
            Value::String(value) => Ok(value),
            Value::Undefined => Ok("undefined".to_owned()),
            Value::Null => Ok("null".to_owned()),
            Value::Boolean(value) => Ok(value.to_string()),
            Value::Number(_) => Err(JsError::unsupported(
                "numeric property key conversion is not implemented",
            )),
            _ => Err(JsError::unsupported(
                "object property key conversion is not implemented",
            )),
        }
    }

    // Array, ordinary object, function and native error values compare by identity.
    // https://tc39.es/ecma262/#sec-isstrictlyequal
    pub(super) fn strictly_equals(&self, left: &Value, right: &Value) -> bool {
        match (left, right) {
            (Value::Array(left), Value::Array(right)) => std::rc::Rc::ptr_eq(left, right),

            (Value::Document(left), Value::Document(right)) => std::rc::Rc::ptr_eq(left, right),

            (Value::Object(left), Value::Object(right)) => {
                self.host_roots.same_ordinary_object(left, right)
            }

            (Value::Function(left), Value::Function(right)) => {
                left.same_identity(&self.host_roots, right)
            }

            (Value::NativeError(left), Value::NativeError(right)) => {
                left.same_identity(&self.host_roots, right)
            }

            _ => left == right,
        }
    }

    // WPT's assert_equals and assert_not_equals use SameValue, not ===.
    // https://tc39.es/ecma262/#sec-samevalue
    pub(super) fn same_value(&self, left: &Value, right: &Value) -> bool {
        match (left, right) {
            (Value::Number(left), Value::Number(right)) if left.is_nan() && right.is_nan() => true,

            (Value::Number(left), Value::Number(right)) if *left == 0.0 && *right == 0.0 => {
                left.is_sign_negative() == right.is_sign_negative()
            }

            _ => self.strictly_equals(left, right),
        }
    }

    pub(super) fn loosely_equals(&self, left: &Value, right: &Value) -> bool {
        match (left, right) {
            (Value::Undefined, Value::Null) | (Value::Null, Value::Undefined) => true,

            (Value::String(_), Value::Number(_)) | (Value::Number(_), Value::String(_)) => {
                self.primitive_to_number(left) == self.primitive_to_number(right)
            }

            _ => self.strictly_equals(left, right),
        }
    }

    pub(super) fn numeric_operands(
        &mut self,
        left: &Value,
        right: &Value,
    ) -> Result<(f64, f64), JsError> {
        let left = self.to_number(left)?;
        let right = self.to_number(right)?;
        Ok((left, right))
    }

    pub(super) fn is_truthy(&self, value: &Value) -> bool {
        match value {
            Value::Undefined | Value::Null | Value::Boolean(false) => false,
            Value::Number(value) => *value != 0.0 && !value.is_nan(),
            Value::String(value) => !value.is_empty(),
            _ => true,
        }
    }

    pub(super) fn type_of(value: &Value) -> &'static str {
        match value {
            Value::Undefined => "undefined",

            Value::Null
            | Value::Array(_)
            | Value::Object(_)
            | Value::NativeError(_)
            | Value::Url(_)
            | Value::Document(_)
            | Value::Element(_)
            | Value::HtmlCollection(_)
            | Value::ElementChildren(_)
            | Value::Attributes(_)
            | Value::Attribute { .. }
            | Value::NodeList(_)
            | Value::ClassList(_)
            | Value::RegExp(_)
            | Value::Arguments(_)
            | Value::TestContext => "object",

            Value::Boolean(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",

            Value::NativeFunction(_)
            | Value::DocumentMethod(_)
            | Value::ElementMethod(_)
            | Value::Function(_)
            | Value::ElementConstructor(_)
            | Value::ObjectConstructor
            | Value::TestContextMethod(_)
            | Value::RegExpTest(_)
            | Value::RegExpExec(_)
            | Value::ArrayMethod { .. } => "function",
        }
    }

    pub(super) fn primitive_to_number(&self, value: &Value) -> Option<f64> {
        match value {
            Value::Number(value) => Some(*value),

            Value::Boolean(true) => Some(1.0),

            Value::Boolean(false) | Value::Null => Some(0.0),

            Value::String(value) => Some(crate::number::string_to_number(value)),

            Value::Undefined => Some(f64::NAN),

            Value::Array(_)
            | Value::Object(_)
            | Value::NativeError(_)
            | Value::Url(_)
            | Value::NativeFunction(_)
            | Value::Document(_)
            | Value::Element(_)
            | Value::HtmlCollection(_)
            | Value::ElementChildren(_)
            | Value::Attributes(_)
            | Value::Attribute { .. }
            | Value::DocumentMethod(_)
            | Value::Function(_)
            | Value::ElementConstructor(_)
            | Value::TestContextMethod(_)
            | Value::ElementMethod(_)
            | Value::NodeList(_)
            | Value::ClassList(_)
            | Value::ArrayMethod { .. }
            | Value::ObjectConstructor
            | Value::RegExp(_)
            | Value::RegExpTest(_)
            | Value::RegExpExec(_)
            | Value::Arguments(_)
            | Value::TestContext => None,
        }
    }

    // https://tc39.es/ecma262/#sec-tonumber
    #[expect(
        clippy::wrong_self_convention,
        reason = "named after the ToNumber abstract operation; `sels` is the interpreter"
    )]
    pub(super) fn to_number(&mut self, value: &Value) -> Result<f64, JsError> {
        let primitive = self.ordinary_to_primitive(value, PrimitiveHint::Number)?;
        self.primitive_to_number(&primitive).ok_or_else(|| {
            JsError::unsupported("number conversion for this primitive is not implemented")
        })
    }

    pub(super) fn compare_values(
        &mut self,
        left: &Value,
        right: &Value,
        comparison: impl FnOnce(f64, f64) -> bool,
    ) -> Result<bool, JsError> {
        let left = self.ordinary_to_primitive(left, PrimitiveHint::Number)?;
        let right = self.ordinary_to_primitive(right, PrimitiveHint::Number)?;
        if let (Value::String(left), Value::String(right)) = (&left, &right) {
            use std::cmp::Ordering;
            let order = left.encode_utf16().cmp(right.encode_utf16());
            let delta = match order {
                Ordering::Less => -1.0,
                Ordering::Equal => 0.0,
                Ordering::Greater => 1.0,
            };
            return Ok(comparison(delta, 0.0));
        }
        let (left, right) = self.numeric_operands(&left, &right)?;
        Ok(comparison(left, right))
    }

    // Partial OrdinaryToPrimitive; @@toPrimitive is not implemented.
    // https://tc39.es/ecma262/#sec-ordinarytoprimitive
    pub(super) fn ordinary_to_primitive(
        &mut self,
        value: &Value,
        hint: PrimitiveHint,
    ) -> Result<Value, JsError> {
        match value {
            Value::Undefined
            | Value::Null
            | Value::Boolean(_)
            | Value::Number(_)
            | Value::String(_) => return Ok(value.clone()),

            Value::Object(_) | Value::NativeError(_) | Value::Array(_) => {}

            _ => {
                return Err(JsError::unsupported(
                    "object-to-primitive conversion for this value is not implemented",
                ));
            }
        }

        // User-defined conversion methods may re-enter the interpreter.
        // Keep the receiver rooted for the entire two-method lookup and call sequence.
        self.with_frame_value(value.clone(), |interpreter, index| {
            interpreter.ordinary_to_primitive_rooted(index, hint)
        })
    }

    fn ordinary_to_primitive_rooted(
        &mut self,
        index: usize,
        hint: PrimitiveHint,
    ) -> Result<Value, JsError> {
        let value = self.frame_value(index).clone();
        let methods = match hint {
            PrimitiveHint::String => ["toString", "valueOf"],
            PrimitiveHint::Number => ["valueOf", "toString"],
        };

        for name in methods {
            let lookup = match &value {
                Value::Object(object) => self.host_roots.lookup_ordinary_property(object, name),
                Value::NativeError(error) => error.lookup_method(&self.host_roots, name),

                Value::Array(array) => match Self::array_get(array.clone(), name) {
                    Some(method) => PropertyLookup::Found(method),

                    None => self
                        .host_roots
                        .lookup_ordinary_property(&self.object_prototype, name),
                },

                _ => unreachable!(),
            };
            let method = match lookup {
                PropertyLookup::Found(method) => method,

                PropertyLookup::Absent => continue,

                PropertyLookup::Unknown => {
                    return Err(JsError::unsupported(
                        "object-to-primitive method lookup is not implemented",
                    ));
                }
            };

            if Self::type_of(&method) != "function" {
                continue;
            }

            let result = self.dispatch_call(method, Some(value.clone()), Vec::new())?;

            if matches!(
                result,
                Value::Undefined
                    | Value::Null
                    | Value::Boolean(_)
                    | Value::Number(_)
                    | Value::String(_)
            ) {
                return Ok(result);
            }
        }

        Err(JsError::type_error(
            "cannot convert object to primitive value",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{Interpreter, Value};

    #[test]
    fn strict_and_same_value_handle_numbers_and_identity() {
        let interpreter = Interpreter::new();
        let nan = Value::Number(f64::NAN);
        let positive_zero = Value::Number(0.0);
        let negative_zero = Value::Number(-0.0);

        assert!(!interpreter.strictly_equals(&nan, &nan));
        assert!(interpreter.same_value(&nan, &nan));

        assert!(interpreter.strictly_equals(&positive_zero, &negative_zero));
        assert!(!interpreter.same_value(&positive_zero, &negative_zero));

        let first = Value::array(vec![Value::Number(1.0)]);
        let alias = first.clone();
        let second = Value::array(vec![Value::Number(1.0)]);

        assert!(interpreter.strictly_equals(&first, &alias));
        assert!(!interpreter.strictly_equals(&first, &second));

        let first = Value::object([]);
        let alias = first.clone();
        let second = Value::object([]);
        assert!(interpreter.strictly_equals(&first, &alias));
        assert!(interpreter.same_value(&first, &alias));
        assert!(!interpreter.strictly_equals(&first, &second));
        assert!(!interpreter.same_value(&first, &second));
    }

    #[test]
    fn converts_supported_primitive_values() {
        let mut interpreter = Interpreter::new();

        assert!(!interpreter.is_truthy(&Value::Undefined));
        assert!(!interpreter.is_truthy(&Value::Number(f64::NAN)));
        assert!(interpreter.is_truthy(&Value::String("0".to_owned())));

        assert_eq!(
            interpreter
                .to_number(&Value::String(" 1.5 ".to_owned()))
                .unwrap(),
            1.5
        );

        assert_eq!(interpreter.to_number(&Value::Null).unwrap(), 0.0);
        assert_eq!(Interpreter::type_of(&Value::Null), "object");
        assert!(interpreter.loosely_equals(&Value::Null, &Value::Undefined));
    }
}
