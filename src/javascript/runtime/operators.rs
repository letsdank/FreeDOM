//! Partial evaluation of JavaScript operators and assignment targets.
//!
//! ECMAScript expressions:
//! <https://tc39.es/ecma262/#sec-ecmascript-language-expressions>
//!
//! This module preserves the interpreter's existing behavior.

use super::value_ops::PrimitiveHint;
use super::{BinaryOperator, Expression, Interpreter, JsError, Value};

impl Interpreter {
    pub(super) fn evaluate_binary(
        &mut self,
        left: Value,
        operator: BinaryOperator,
        right: Value,
    ) -> Result<Value, JsError> {
        match operator {
            BinaryOperator::StrictEqual => Ok(Value::Boolean(Self::strictly_equals(&left, &right))),
            BinaryOperator::StrictNotEqual => {
                Ok(Value::Boolean(!Self::strictly_equals(&left, &right)))
            }

            BinaryOperator::Equal => Ok(Value::Boolean(self.loosely_equals(&left, &right))),
            BinaryOperator::NotEqual => Ok(Value::Boolean(!self.loosely_equals(&left, &right))),

            BinaryOperator::And | BinaryOperator::Or | BinaryOperator::Nullish => {
                unreachable!("logical operators are evaluated with short-circuiting")
            }

            BinaryOperator::Add => {
                let left = self.ordinary_to_primitive(&left, PrimitiveHint::Number)?;
                let right = self.ordinary_to_primitive(&right, PrimitiveHint::Number)?;
                if matches!(&left, Value::String(_)) || matches!(&right, Value::String(_)) {
                    return Ok(Value::String(format!(
                        "{}{}",
                        left.to_js_string(&self.host_roots),
                        right.to_js_string(&self.host_roots)
                    )));
                }

                let (left, right) = self.numeric_operands(&left, &right)?;
                Ok(Value::Number(left + right))
            }

            BinaryOperator::Subtract => {
                let (left, right) = self.numeric_operands(&left, &right)?;
                Ok(Value::Number(left - right))
            }

            BinaryOperator::Multiply => {
                let (left, right) = self.numeric_operands(&left, &right)?;
                Ok(Value::Number(left * right))
            }

            BinaryOperator::Divide => {
                let (left, right) = self.numeric_operands(&left, &right)?;
                Ok(Value::Number(left / right))
            }

            BinaryOperator::Remainder => {
                let (left, right) = self.numeric_operands(&left, &right)?;

                Ok(Value::Number(left % right))
            }

            BinaryOperator::Less => Ok(Value::Boolean(self.compare_values(
                &left,
                &right,
                |a, b| a < b,
            )?)),

            BinaryOperator::LessEqual => Ok(Value::Boolean(self.compare_values(
                &left,
                &right,
                |a, b| a <= b,
            )?)),

            BinaryOperator::Greater => Ok(Value::Boolean(self.compare_values(
                &left,
                &right,
                |a, b| a > b,
            )?)),

            BinaryOperator::GreaterEqual => Ok(Value::Boolean(self.compare_values(
                &left,
                &right,
                |a, b| a >= b,
            )?)),

            BinaryOperator::Instanceof => self.evaluate_instanceof(left, right),

            BinaryOperator::In => self.evaluate_in(left, right),
        }
    }

    pub(super) fn evaluate_update(
        &mut self,
        target: &Expression,
        delta: f64,
        prefix: bool,
    ) -> Result<Value, JsError> {
        let reference = self.prepare_reference(target)?;

        self.with_active_reference(reference, |interpreter, reference_index| {
            let reference = interpreter.active_reference(reference_index).clone();
            let previous = interpreter.read_reference(&reference)?;

            interpreter.with_frame_value(previous, |interpreter, previous_index| {
                let previous = interpreter.frame_value(previous_index).clone();
                let number = interpreter.to_number(&previous)?;
                let updated = Value::Number(number + delta);
                let reference = interpreter.active_reference(reference_index).clone();

                interpreter.write_reference(reference, updated.clone())?;

                if prefix {
                    Ok(updated)
                } else {
                    Ok(Value::Number(number))
                }
            })
        })
    }

    pub(super) fn evaluate_unary_number(
        &mut self,
        operand: &Expression,
        negate: bool,
    ) -> Result<Value, JsError> {
        let value = self.evaluate_expression(operand)?;
        let number = self.to_number(&value)?;

        Ok(Value::Number(if negate { -number } else { number }))
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::{Value, evaluate};

    #[test]
    fn numeric_operators_use_primitive_conversion_and_propagate_errors() {
        let result = evaluate(
            r#"
            let trace = '';
            let left = { valueOf: function() { trace = trace + 'l'; return 2; } };
            let right = { valueOf: function() { trace = trace + 'r'; return '3'; } };
            let sum = left + right;
            let difference = left - right;
            let update = { valueOf: function() { return 8; } };
            
            let before = update++;
            let failure = false;
            try { +{ valueOf: function() { throw 'boom'; } }; }
            catch (e) { failure = e === 'boom'; }
            sum === '23' && difference === -1 && trace === 'lrlr'
                && before === 8 && update === 9 && failure
                && '10' < '2'
                && String.fromCharCode({ valueOf: function() { return 65; } }) === 'A';
            "#,
        )
        .unwrap();
        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn evaluates_binary_precedence_and_string_concatenation() {
        assert_eq!(evaluate("1 + 2 * 3;").unwrap(), Value::Number(7.0));

        assert_eq!(
            evaluate("\"answer: \" + 42;").unwrap(),
            Value::String("answer: 42".to_owned())
        );
    }

    #[test]
    fn preserves_prefix_and_postfix_update_results() {
        let result = evaluate("let x = 1; let old = x++; let next = ++x; old + next + x;").unwrap();

        assert_eq!(result, Value::Number(7.0));
    }

    #[test]
    fn assigns_to_array_index() {
        let result = evaluate("let values = [1]; values[0] = 4; values[0];").unwrap();

        assert_eq!(result, Value::Number(4.0));
    }

    #[test]
    fn logical_operators_short_circuit_side_effects() {
        let result = evaluate("let x = 0; false && (x = 1); true || (x = 2); x;").unwrap();

        assert_eq!(result, Value::Number(0.0));
    }

    #[test]
    fn evaluates_unary_numeric_operators() {
        for (source, expected) in [
            ("-1;", -1.0),
            ("+'12';", 12.0),
            ("-true;", -1.0),
            ("- -2 * 3;", 6.0),
            ("1 + -2;", -1.0),
        ] {
            assert_eq!(
                evaluate(source).unwrap(),
                Value::Number(expected),
                "{source}"
            );
        }

        let nan = evaluate("+undefined;")
            .unwrap()
            .as_number()
            .expect("expected number");

        assert!(nan.is_nan());
    }

    #[test]
    fn unary_minus_preserves_signed_zero() {
        let negative_zero = evaluate("-0;")
            .unwrap()
            .as_number()
            .expect("expected number");

        assert_eq!(negative_zero, 0.0);
        assert!(negative_zero.is_sign_negative());

        let positive_zero = evaluate("- -0;")
            .unwrap()
            .as_number()
            .expect("expected number");

        assert!(positive_zero.is_sign_positive());
    }

    #[test]
    fn unary_operand_is_evaluated_once() {
        let result = evaluate(
            "let count = 0; \
            function get() { count++; return 2; } \
            let value = -get(); \
            count === 1 && value === -2;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn unary_object_conversion_is_explicitly_unsupported() {
        let error = evaluate("+new URL('https://example.org');").unwrap_err();

        assert_eq!(
            error.kind,
            crate::javascript::JsErrorKind::UnsupportedFeature
        );
    }

    #[test]
    fn remainder_obeys_multiplicative_precedence_and_dividend_sign() {
        for (source, expected) in [
            ("8 + 5 % 3 * 2;", 12.0),
            ("-5 % 2;", -1.0),
            ("5 % -2;", 1.0),
            ("'7' % 4;", 3.0),
            ("5 % (1 / 0);", 5.0),
        ] {
            assert_eq!(
                evaluate(source).unwrap(),
                Value::Number(expected),
                "{source}"
            );
        }
    }

    #[test]
    fn remainder_handles_nan_infinity_and_negative_zero() {
        for source in ["0 % 0;", "1 % 0;", "(1 / 0) % 2;"] {
            let number = evaluate(source)
                .unwrap()
                .as_number()
                .expect("expected number");

            assert!(number.is_nan(), "{source}");
        }

        let number = evaluate("-4 % 2;")
            .unwrap()
            .as_number()
            .expect("expected number");

        assert_eq!(number, 0.0);
        assert!(number.is_sign_negative());
    }

    #[test]
    fn arrays_use_value_of_then_to_string_for_primitive_conversion() {
        let result = evaluate(
            r#"
                let trace = '';
                let array = [1, 2];

                let first = 'x' + array === 'x1,2'
                    && +[] === 0
                    && +[7] === 7;

                let other = [3];

                other.valueOf = function() {
                    trace = trace + 'v';
                    return {};
                };

                other.toString = function() {
                    trace = trace + 's';
                    return '8';
                };

                let second = +other === 8 && trace === 'vs';

                let element = {
                    toString: function() { return 'object'; }
                };

                let joined = '' + [element, [1, 2], null] === 'object,1,2,';

                let error = false;

                try {
                    +[{
                        toString: function() { throw 'boom'; }
                    }];
                } catch (e) {
                    error = e === 'boom';
                }

                first && second && joined && error;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn cyclic_array_coercion_is_bounded() {
        assert_eq!(
            evaluate("let a = []; a.push(a); '' + a;").unwrap(),
            Value::String(String::new())
        );
    }
}
