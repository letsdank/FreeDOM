//! Partial evaluation of JavaScript operators and assignment targets.
//!
//! ECMAScript expressions:
//! <https://tc39.es/ecma262/#sec-ecmascript-language-expressions>
//!
//! This module preserves the interpreter's existing behavior.

use super::{BinaryOperator, Expression, Interpreter, JsError, Value};

impl Interpreter {
    pub(super) fn evaluate_binary(
        &self,
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

            BinaryOperator::And | BinaryOperator::Or => {
                unreachable!("logical operators are evaluated with short-circuiting")
            }

            BinaryOperator::Add => {
                if matches!(&left, Value::String(_)) || matches!(&right, Value::String(_)) {
                    return Ok(Value::String(format!(
                        "{}{}",
                        left.to_js_string(),
                        right.to_js_string()
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

            BinaryOperator::Instanceof => {
                let matches = match (&left, &right) {
                    (Value::Element(node_id), Value::ElementConstructor(tag)) => self
                        .current_document_element(*node_id)
                        .map(|element| element.tag_name == *tag)
                        .unwrap_or(false),

                    _ => false,
                };

                Ok(Value::Boolean(matches))
            }
        }
    }

    pub(super) fn evaluate_update(
        &mut self,
        target: &Expression,
        delta: f64,
        prefix: bool,
    ) -> Result<Value, JsError> {
        let reference = self.prepare_assignment(target)?;
        let previous = self.read_assignment(&reference)?;

        let Some(number) = self.to_number(&previous) else {
            return Err(JsError::runtime(
                "TypeError: update operator requires a numeric value",
            ));
        };

        let updated = Value::Number(number + delta);

        self.write_assignment(reference, updated.clone())?;

        if prefix {
            Ok(updated)
        } else {
            Ok(Value::Number(number))
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::{Value, evaluate};

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
}
