//! Partial Web IDL argument conversion for host bindings.
//!
//! <https://webidl.spec.whatwg.org/#es-unsigned-long>
//! <https://webidl.spec.whatwg.org/#es-DOMString>
//!
//! Object-to-primitive conversion, symbols, and complete number formatting
//! are not implemented.

use super::{Interpreter, Value};

impl Interpreter {
    pub(super) fn webidl_unsigned_long(&self, value: &Value) -> Option<u32> {
        let number = self.to_number(value)?;

        Some(if !number.is_finite() || number == 0.0 {
            0
        } else {
            number.trunc().rem_euclid(4_294_967_296.0) as u32
        })
    }

    pub(super) fn webidl_dom_string(value: &Value) -> Option<String> {
        match value {
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Interpreter, Value};

    #[test]
    fn converts_supported_webidl_primitive_values() {
        let interpreter = Interpreter::new();

        for (input, expected) in [
            (Value::Number(1.9), 1),
            (Value::Number(-1.0), u32::MAX),
            (Value::Number(4_294_967_296.0), 0),
            (Value::Undefined, 0),
            (Value::Number(f64::INFINITY), 0),
            (Value::String("42".to_owned()), 42),
        ] {
            assert_eq!(interpreter.webidl_unsigned_long(&input), Some(expected));
        }

        assert_eq!(
            Interpreter::webidl_dom_string(&Value::Number(-0.0)).as_deref(),
            Some("0")
        );

        assert_eq!(
            Interpreter::webidl_dom_string(&Value::Undefined).as_deref(),
            Some("undefined")
        );

        assert_eq!(
            Interpreter::webidl_dom_string(&Value::Number(f64::INFINITY)).as_deref(),
            Some("Infinity")
        );

        assert!(
            interpreter
                .webidl_unsigned_long(&Value::object([]))
                .is_none()
        );

        assert!(Interpreter::webidl_dom_string(&Value::Number(1.5)).is_none());
    }
}
