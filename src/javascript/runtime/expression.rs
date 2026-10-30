//! Evaluation of the interpreter's supported expression subset.
//!
//! ECMAScript expressions:
//! <https://tc39.es/ecma262/#sec-ecmascript-language-expressions>
//!
//! This module preserves existing behavior and is not a complete implementation.

use super::{BinaryOperator, Expression, Interpreter, JsError, Url, Value};

impl Interpreter {
    pub(super) fn evaluate_expression(
        &mut self,
        expression: &Expression,
    ) -> Result<Value, JsError> {
        match expression {
            Expression::Literal(value) => Ok(value.clone()),

            Expression::RegExpLiteral { pattern, flags } => Self::create_regexp(pattern, flags),

            Expression::Array(expressions) => {
                let mut values = Vec::with_capacity(expressions.len());

                for expression in expressions {
                    values.push(self.evaluate_expression(expression)?);
                }

                Ok(Value::array(values))
            }

            Expression::Object(properties) => {
                let mut values = Vec::with_capacity(properties.len());

                for (name, expression) in properties {
                    values.push((name.clone(), self.evaluate_expression(expression)?));
                }

                Ok(Value::object(values))
            }

            Expression::Identifier(name) => {
                self.variables.get(name).cloned().ok_or_else(|| {
                    JsError::runtime(format!("ReferenceError: {name} is not defined"))
                })
            }

            Expression::New {
                constructor,
                arguments,
            } => self.construct(constructor, arguments),

            Expression::Member { object, property } => {
                let object = self.evaluate_expression(object)?;
                self.member_value(object, property)
            }

            Expression::Index { object, index } => {
                let object = self.evaluate_expression(object)?;
                let index = self.evaluate_expression(index)?;

                self.index_value(object, index)
            }

            Expression::Function(function) => Ok(Value::Function(function.clone())),

            Expression::Not(expression) => {
                let value = self.evaluate_expression(expression)?;
                Ok(Value::Boolean(!self.is_truthy(&value)))
            }

            Expression::Assignment { target, value } => {
                let value = self.evaluate_expression(value)?;
                self.assign(target, value.clone())?;
                Ok(value)
            }

            Expression::Call { callee, arguments } => self.evaluate_call(callee, arguments),

            Expression::Binary {
                left,
                operator: BinaryOperator::And,
                right,
            } => {
                let left = self.evaluate_expression(left)?;

                if self.is_truthy(&left) {
                    self.evaluate_expression(right)
                } else {
                    Ok(left)
                }
            }

            Expression::Binary {
                left,
                operator: BinaryOperator::Or,
                right,
            } => {
                let left = self.evaluate_expression(left)?;

                if self.is_truthy(&left) {
                    Ok(left)
                } else {
                    self.evaluate_expression(right)
                }
            }

            Expression::Binary {
                left,
                operator,
                right,
            } => {
                let left = self.evaluate_expression(left)?;
                let right = self.evaluate_expression(right)?;

                self.evaluate_binary(left, *operator, right)
            }

            Expression::Update {
                target,
                delta,
                prefix,
            } => self.evaluate_update(target, *delta, *prefix),

            Expression::Conditional {
                condition,
                consequent,
                alternate,
            } => {
                let condition = self.evaluate_expression(condition)?;

                if self.is_truthy(&condition) {
                    self.evaluate_expression(consequent)
                } else {
                    self.evaluate_expression(alternate)
                }
            }

            Expression::TypeOf(expression) => {
                // An unresolvable identifier is handled specially by typeof.
                // https://tc39.es/ecma262/#sec-typeof-operator
                if let Expression::Identifier(name) = expression.as_ref() {
                    if !self.variables.contains_key(name) {
                        return Ok(Value::String("undefined".to_owned()));
                    }
                }

                let value = self.evaluate_expression(expression)?;
                Ok(Value::String(Self::type_of(&value).to_owned()))
            }
        }
    }

    fn construct(&mut self, constructor: &str, arguments: &[Expression]) -> Result<Value, JsError> {
        if constructor == "RegExp" {
            return self.construct_regexp(arguments);
        }

        if constructor != "URL" {
            return Err(JsError::runtime(format!(
                "ReferenceError: constructor {constructor} is not defined"
            )));
        }

        if !(1..=2).contains(&arguments.len()) {
            return Err(JsError::runtime(
                "TypeError: URL constructor expects one or two arguments",
            ));
        }

        let input = self.evaluate_expression(&arguments[0])?.to_js_string();

        let url = if arguments.len() == 1 {
            Url::parse(&input)
        } else {
            let base = self.evaluate_expression(&arguments[1])?.to_js_string();

            Url::parse(&base).and_then(|base_url| base_url.join(&input))
        }
        .map_err(|error| JsError::runtime(format!("TypeError: invalid URL: {error}")))?;

        Ok(Value::Url(url))
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::{Value, evaluate};

    #[test]
    fn logical_expressions_return_operand_values() {
        assert_eq!(evaluate("0 || 7;").unwrap(), Value::Number(7.0));
        assert_eq!(evaluate("3 && 8;").unwrap(), Value::Number(8.0));
        assert_eq!(evaluate("0 && 8;").unwrap(), Value::Number(0.0));
    }

    #[test]
    fn conditional_only_evaluates_selected_branch() {
        let result = evaluate(
            "let count = 0; \
            true ? (count = 1) : (count = 2); \
            count;",
        )
        .unwrap();

        assert_eq!(result, Value::Number(1.0));
    }

    #[test]
    fn call_arguments_are_evaluated_left_to_right() {
        let result = evaluate(
            "let x = 0; \
            const f = function(a, b) { return a * 10 + b; }; \
            f(x = x + 1, x = x + 1);",
        )
        .unwrap();

        assert_eq!(result, Value::Number(12.0));
    }

    #[test]
    fn constructs_url_relative_to_base() {
        let result = evaluate(
            "const url = new URL('child', 'https://example.org/dir/'); \
            url.href;",
        )
        .unwrap();

        assert_eq!(
            result,
            Value::String("https://example.org/dir/child".to_owned())
        );
    }

    #[test]
    fn typeof_undeclared_identifier_returns_undefined() {
        assert_eq!(
            evaluate("typeof missing;").unwrap(),
            Value::String("undefined".to_owned())
        );

        assert_eq!(
            evaluate("typeof (missing);").unwrap(),
            Value::String("undefined".to_owned())
        );

        assert_eq!(
            evaluate("typeof $missing;").unwrap(),
            Value::String("undefined".to_owned())
        );

        assert_eq!(
            evaluate("let present = 42; typeof present;").unwrap(),
            Value::String("number".to_owned())
        );
    }

    #[test]
    fn undeclared_identifiers_still_report_reference_errors() {
        assert_eq!(
            evaluate("missing;").unwrap_err().message,
            "ReferenceError: missing is not defined"
        );

        assert_eq!(
            evaluate("$missing;").unwrap_err().message,
            "ReferenceError: $missing is not defined"
        );

        assert_eq!(
            evaluate("typeof missing.value;").unwrap_err().message,
            "ReferenceError: missing is not defined"
        );
    }

    #[test]
    fn executes_concise_arrow_bodies() {
        assert_eq!(
            evaluate("const f = x => x * 2; f(4);").unwrap(),
            Value::Number(8.0)
        );

        assert_eq!(
            evaluate("const f = (x) => x + 1; f(4);").unwrap(),
            Value::Number(5.0)
        );

        assert_eq!(
            evaluate("const f = () => 42; f();").unwrap(),
            Value::Number(42.0)
        );
    }

    #[test]
    fn arrow_object_expression_and_block_body_are_distinct() {
        assert_eq!(
            evaluate("const f = () => ({ value: 5 }); f().value;").unwrap(),
            Value::Number(5.0)
        );

        assert_eq!(
            evaluate("const f = x => { return x + 1; }; f(2);").unwrap(),
            Value::Number(3.0)
        );
    }
}
