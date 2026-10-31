//! Partial array destructuring in for-of loops.
//!
//! <https://tc39.es/ecma262/#sec-for-in-and-for-of-statements>
//!
//! Only arrays and simple lexical bindings are supported.

use super::{Completion, Expression, Interpreter, JsError, Statement, Value};

impl Interpreter {
    pub(super) fn execute_for_of_array(
        &mut self,
        bindings: &[String],
        iterable: &Expression,
        body: &[Statement],
    ) -> Result<Completion, JsError> {
        let target = self.evaluate_expression(iterable)?;

        let items = match target {
            Value::Array(items) => items,

            Value::Null | Value::Undefined | Value::Boolean(_) | Value::Number(_) => {
                return Err(JsError::type_error("for-of value is not iterable"));
            }

            _ => {
                return Err(JsError::unsupported(
                    "for-of array binding currently requires an array source",
                ));
            }
        };

        let items = items.borrow().values.clone();
        let mut previous = Vec::new();

        for binding in bindings {
            previous.push((
                binding.clone(),
                self.variables.insert(binding.clone(), Value::Undefined),
            ));
        }

        let result = (|| -> Result<Completion, JsError> {
            for item in items {
                let values = match item {
                    Value::Array(values) => values.borrow().values.clone(),

                    Value::Null | Value::Undefined | Value::Boolean(_) | Value::Number(_) => {
                        return Err(JsError::type_error("array binding value is not iterable"));
                    }

                    _ => {
                        return Err(JsError::unsupported(
                            "array binding only supports array values",
                        ));
                    }
                };

                for (index, binding) in bindings.iter().enumerate() {
                    let value = values.get(index).cloned().unwrap_or(Value::Undefined);

                    self.variables.insert(binding.clone(), value);
                }

                match self.execute(body)? {
                    Completion::Normal(_) | Completion::Continue => {}

                    Completion::Break => {
                        return Ok(Completion::Normal(Value::Undefined));
                    }

                    Completion::Return(value) => {
                        return Ok(Completion::Return(value));
                    }
                }
            }

            Ok(Completion::Normal(Value::Undefined))
        })();

        for (binding, old) in previous.into_iter().rev() {
            if let Some(value) = old {
                self.variables.insert(binding, value);
            } else {
                self.variables.remove(&binding);
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::{JsErrorKind, Value, evaluate};

    #[test]
    fn iterates_object_entries_with_array_bindings() {
        let result = evaluate(
            "const required_props = { \
                code: 22, \
                name: 'QuotaExceededError' \
            }; \
            let out = ''; \
            for (const [prop, expected] of Object.entries(required_props)) { \
                out += prop + '=' + expected + ';'; \
            } \
            out;",
        )
        .unwrap();

        assert_eq!(
            result,
            Value::String("code=22;name=QuotaExceededError;".to_owned())
        );
    }

    #[test]
    fn shorter_rows_produce_undefined_and_trailing_comma_is_supported() {
        let result = evaluate(
            "let out = ''; \
            for (let [x,y,] of [[1], [2, 3]]) { \
                if (y === undefined) { \
                    out += 'missing'; \
                } else { \
                    out += x + y; \
                } \
            } \
            out;",
        )
        .unwrap();

        assert_eq!(result, Value::String("missing5".to_owned()));
    }

    #[test]
    fn propagates_return_continue_and_restores_outer_bindings() {
        let result = evaluate(
            "let a = 'outer'; \
            let b = 'old'; \
            function f() { \
                for (const [a, b] of [[1, 2], [3, 4]]) { \
                    if (a === 1) continue; \
                    return b;\
                } \
            } \
            let result = f(); \
            result === 4 && a === 'outer' && b === 'old';",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn restores_bindings_when_item_is_not_iterable() {
        let result = evaluate(
            "let a = 7; \
            let name = ''; \
            try { \
                for (let [a] of [null]) {} \
            } catch (e) { \
                name = e.name; \
            } \
            a === 7 && name === 'TypeError';",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn unsupported_iterables_are_not_misreported_as_type_errors() {
        let error = evaluate("for (let [x] of ['abc']) {}").unwrap_err();

        assert_eq!(error.kind, JsErrorKind::UnsupportedFeature);

        let result = evaluate(
            "let name = ''; \
            try { \
                for (let [x] of null) {} \
            } catch (e) { \
                name = e.name; \
            } \
            name;",
        )
        .unwrap();

        assert_eq!(result, Value::String("TypeError".to_owned()));
    }
}
