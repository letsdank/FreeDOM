//! Partial Web IDL argument conversion for host bindings.
//!
//! <https://webidl.spec.whatwg.org/#es-unsigned-long>
//! <https://webidl.spec.whatwg.org/#es-DOMString>
//!
//! Object conversion is limited to ordinary objects, native errors, and arrays;
//! symbols and complete number formatting are not implemented.

use super::value_ops::PrimitiveHint;
use super::{Interpreter, JsError, Value};

impl Interpreter {
    pub(super) fn webidl_unsigned_long(&mut self, value: &Value) -> Result<u32, JsError> {
        let number = self.to_number(value)?;

        Ok(if !number.is_finite() || number == 0.0 {
            0
        } else {
            number.trunc().rem_euclid(4_294_967_296.0) as u32
        })
    }

    pub(super) fn webidl_dom_string(&mut self, value: &Value) -> Result<String, JsError> {
        let value = self.ordinary_to_primitive(value, PrimitiveHint::String)?;

        let result = match &value {
            Value::Number(number) if *number == 0.0 => Some("0".to_owned()),

            Value::Number(number) if number.is_nan() => Some("NaN".to_owned()),

            Value::Number(number) if *number == f64::INFINITY => Some("Infinity".to_owned()),

            Value::Number(number) if *number == f64::NEG_INFINITY => Some("-Infinity".to_owned()),

            Value::Number(number)
                if number.fract() == 0.0 && number.abs() <= 9_007_199_254_740_991.0 =>
            {
                Some(number.to_string())
            }

            Value::String(_) | Value::Boolean(_) | Value::Undefined | Value::Null => {
                Some(value.to_js_string())
            }

            _ => None,
        };

        result.ok_or_else(|| {
            JsError::unsupported(
                "Web IDL DOMString conversion for this primitive is not implemented",
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Interpreter, Value};

    #[test]
    fn converts_supported_webidl_primitive_values() {
        let mut interpreter = Interpreter::new();

        for (input, expected) in [
            (Value::Number(1.9), 1),
            (Value::Number(-1.0), u32::MAX),
            (Value::Number(4_294_967_296.0), 0),
            (Value::Undefined, 0),
            (Value::Number(f64::INFINITY), 0),
            (Value::String("42".to_owned()), 42),
        ] {
            assert_eq!(interpreter.webidl_unsigned_long(&input).unwrap(), expected);
        }

        assert_eq!(
            interpreter.webidl_dom_string(&Value::Number(-0.0)).unwrap(),
            "0"
        );

        assert_eq!(
            interpreter.webidl_dom_string(&Value::Undefined).unwrap(),
            "undefined"
        );

        assert_eq!(
            interpreter
                .webidl_dom_string(&Value::Number(f64::INFINITY))
                .unwrap(),
            "Infinity"
        );

        assert_eq!(
            interpreter
                .webidl_unsigned_long(&Value::array(vec![]))
                .unwrap(),
            0
        );

        assert!(interpreter.webidl_dom_string(&Value::Number(1.5)).is_err());
    }
}
