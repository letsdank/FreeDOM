//! Value comparison and conversion helpers for the JavaScript interpreter.
//!
//! <https://tc39.es/ecma262/#sec-type-conversion>
//! <https://tc39.es/ecma262/#sec-testing-and-comparison-operations>
//!
//! These operations currently implement only a subset of ECMAScript semantics.

use super::{Interpreter, JsError, Value};

impl Interpreter {
    // The heap-backed Array and Object variants compare by identity.
    // https://tc39.es/ecma262/#sec-isstrictlyequal
    pub(super) fn strictly_equals(left: &Value, right: &Value) -> bool {
        match (left, right) {
            (Value::Array(left), Value::Array(right)) => std::rc::Rc::ptr_eq(left, right),

            (Value::Object(left), Value::Object(right)) => std::rc::Rc::ptr_eq(left, right),

            _ => left == right,
        }
    }

    // WPT's assert_equals and assert_not_equals use SameValue, not ===.
    // https://tc39.es/ecma262/#sec-samevalue
    pub(super) fn same_value(left: &Value, right: &Value) -> bool {
        match (left, right) {
            (Value::Number(left), Value::Number(right)) if left.is_nan() && right.is_nan() => true,

            (Value::Number(left), Value::Number(right)) if *left == 0.0 && *right == 0.0 => {
                left.is_sign_negative() == right.is_sign_negative()
            }

            _ => Self::strictly_equals(left, right),
        }
    }

    pub(super) fn loosely_equals(&self, left: &Value, right: &Value) -> bool {
        match (left, right) {
            (Value::Undefined, Value::Null) | (Value::Null, Value::Undefined) => true,

            (Value::String(_), Value::Number(_)) | (Value::Number(_), Value::String(_)) => {
                self.to_number(left) == self.to_number(right)
            }

            _ => Self::strictly_equals(left, right),
        }
    }

    pub(super) fn numeric_operands(
        &self,
        left: &Value,
        right: &Value,
    ) -> Result<(f64, f64), JsError> {
        let Some(left) = self.to_number(left) else {
            return Err(JsError::runtime(
                "TypeError: operator requires numeric operands",
            ));
        };

        let Some(right) = self.to_number(right) else {
            return Err(JsError::runtime(
                "TypeError: operator requires numeric operands",
            ));
        };

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

    pub(super) fn to_number(&self, value: &Value) -> Option<f64> {
        match value {
            Value::Number(value) => Some(*value),

            Value::Boolean(true) => Some(1.0),

            Value::Boolean(false) | Value::Null => Some(0.0),

            Value::String(value) => Some(crate::javascript::number::string_to_number(value)),

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
            | Value::TestContext => None,
        }
    }

    pub(super) fn compare_values(
        &self,
        left: &Value,
        right: &Value,
        comparison: impl FnOnce(f64, f64) -> bool,
    ) -> Result<bool, JsError> {
        let (left, right) = self.numeric_operands(left, right)?;

        Ok(comparison(left, right))
    }
}

#[cfg(test)]
mod tests {
    use super::{Interpreter, Value};

    #[test]
    fn strict_and_same_value_handle_numbers_and_identity() {
        let nan = Value::Number(f64::NAN);
        let positive_zero = Value::Number(0.0);
        let negative_zero = Value::Number(-0.0);

        assert!(!Interpreter::strictly_equals(&nan, &nan));
        assert!(Interpreter::same_value(&nan, &nan));

        assert!(Interpreter::strictly_equals(&positive_zero, &negative_zero));
        assert!(!Interpreter::same_value(&positive_zero, &negative_zero));

        let first = Value::array(vec![Value::Number(1.0)]);
        let alias = first.clone();
        let second = Value::array(vec![Value::Number(1.0)]);

        assert!(Interpreter::strictly_equals(&first, &alias));
        assert!(!Interpreter::strictly_equals(&first, &second));
    }

    #[test]
    fn converts_supported_primitive_values() {
        let interpreter = Interpreter::new();

        assert!(!interpreter.is_truthy(&Value::Undefined));
        assert!(!interpreter.is_truthy(&Value::Number(f64::NAN)));
        assert!(interpreter.is_truthy(&Value::String("0".to_owned())));

        assert_eq!(
            interpreter.to_number(&Value::String(" 1.5 ".to_owned())),
            Some(1.5)
        );

        assert_eq!(interpreter.to_number(&Value::Null), Some(0.0));

        assert_eq!(Interpreter::type_of(&Value::Null), "object");

        assert!(interpreter.loosely_equals(&Value::Null, &Value::Undefined));
    }
}
