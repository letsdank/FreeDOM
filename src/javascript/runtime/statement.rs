//! Execution of the interpreter's supported statement subset.
//!
//! ECMAScript statement evaluation:
//! <https://tc39.es/ecma262/#sec-ecmascript-language-statements-and-declarations>

use super::{Completion, Interpreter, JsError, Statement, Value};

impl Interpreter {
    pub(super) fn execute_statements(
        &mut self,
        program: &[Statement],
    ) -> Result<Completion, JsError> {
        // This is limited hoisting for the current flat environment model.
        for statement in program {
            if let Statement::FunctionDeclaration { name, function } = statement {
                self.variables
                    .insert(name.clone(), Value::Function(function.clone()));
            }
        }

        let mut result = Value::Undefined;

        for statement in program {
            result = match statement {
                Statement::FunctionDeclaration { .. } => Value::Undefined,

                Statement::Declaration { name, value } => {
                    let value = self.evaluate_expression(value)?;
                    self.variables.insert(name.clone(), value);
                    Value::Undefined
                }

                Statement::Expression(expression) => self.evaluate_expression(expression)?,

                Statement::Return(expression) => {
                    return Ok(Completion::Return(self.evaluate_expression(expression)?));
                }

                Statement::If {
                    condition,
                    then_body,
                    else_body,
                } => {
                    let condition = self.evaluate_expression(condition)?;

                    let branch = if self.is_truthy(&condition) {
                        then_body
                    } else {
                        else_body
                    };

                    match self.execute(branch)? {
                        Completion::Normal(value) => value,
                        Completion::Return(value) => {
                            return Ok(Completion::Return(value));
                        }
                        Completion::Break => {
                            return Ok(Completion::Break);
                        }
                        Completion::Continue => return Ok(Completion::Continue),
                    }
                }

                Statement::While { condition, body } => {
                    let mut last_value = Value::Undefined;

                    loop {
                        let condition = self.evaluate_expression(condition)?;

                        if !self.is_truthy(&condition) {
                            break;
                        }

                        match self.execute(body)? {
                            Completion::Normal(value) => last_value = value,
                            Completion::Return(value) => {
                                return Ok(Completion::Return(value));
                            }
                            Completion::Break => break,
                            Completion::Continue => continue,
                        }
                    }

                    last_value
                }

                Statement::Break => {
                    return Ok(Completion::Break);
                }

                Statement::Continue => {
                    return Ok(Completion::Continue);
                }

                Statement::For {
                    initializer,
                    condition,
                    update,
                    body,
                } => {
                    if let Some(initializer) = initializer {
                        match self.execute(std::slice::from_ref(initializer.as_ref()))? {
                            Completion::Normal(_) => {}

                            Completion::Return(value) => {
                                return Ok(Completion::Return(value));
                            }

                            Completion::Break => {
                                return Ok(Completion::Break);
                            }

                            Completion::Continue => {
                                return Ok(Completion::Continue);
                            }
                        }
                    }

                    loop {
                        if let Some(condition) = condition {
                            let value = self.evaluate_expression(condition)?;

                            if !self.is_truthy(&value) {
                                break;
                            }
                        }

                        match self.execute(body)? {
                            Completion::Normal(_) => {}
                            Completion::Return(value) => return Ok(Completion::Return(value)),
                            Completion::Break => break,
                            Completion::Continue => {}
                        }

                        if let Some(update) = update {
                            self.evaluate_expression(update)?;
                        }
                    }

                    Value::Undefined
                }

                Statement::ForOf {
                    binding,
                    iterable,
                    body,
                } => {
                    let iterable = self.evaluate_expression(iterable)?;

                    let Value::Array(values) = iterable else {
                        return Err(JsError::runtime(
                            "TypeError: for-of currently requires an Array",
                        ));
                    };
                    let values = values.borrow().values.clone();

                    let previous_binding = self.variables.insert(binding.clone(), Value::Undefined);

                    let completion = (|| -> Result<Completion, JsError> {
                        for value in values {
                            self.variables.insert(binding.clone(), value);

                            match self.execute(body)? {
                                Completion::Normal(_) | Completion::Continue => {}
                                Completion::Break => {
                                    return Ok(Completion::Normal(Value::Undefined));
                                }
                                Completion::Return(value) => return Ok(Completion::Return(value)),
                            }
                        }

                        Ok(Completion::Normal(Value::Undefined))
                    })();

                    if let Some(value) = previous_binding {
                        self.variables.insert(binding.clone(), value);
                    } else {
                        self.variables.remove(binding);
                    }

                    match completion? {
                        Completion::Normal(value) => value,
                        Completion::Return(value) => return Ok(Completion::Return(value)),
                        Completion::Break => return Ok(Completion::Break),
                        Completion::Continue => return Ok(Completion::Continue),
                    }
                }

                Statement::Try { .. } => {
                    return Err(JsError::runtime(
                        "try/catch/finally execution is not implemented yet",
                    ));
                }
            };
        }

        Ok(Completion::Normal(result))
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::{Value, evaluate};

    #[test]
    fn returns_from_nested_loop_and_conditional() {
        let result =
            evaluate("const f = function() { while (true) { if (true) { return 42; } } }; f();")
                .unwrap();

        assert_eq!(result, Value::Number(42.0));
    }

    #[test]
    fn continue_in_for_loop_still_runs_update() {
        let result = evaluate(
            "let sum = 0; for (let i = 0; i < 4; i++) { \
            if (i === 1) continue; sum = sum + i; } sum;",
        )
        .unwrap();

        assert_eq!(result, Value::Number(5.0));
    }

    #[test]
    fn restores_previous_for_of_binding_after_break() {
        let result = evaluate("let x = 7; for (let x of [1, 2]) { break; } x;").unwrap();

        assert_eq!(result, Value::Number(7.0));
    }

    #[test]
    fn function_declaration_is_available_before_its_position() {
        let result = evaluate(
            "let result = add(2, 3); \
            function add(a, b) { return a + b; } \
            result;",
        )
        .unwrap();

        assert_eq!(result, Value::Number(5.0));
    }

    #[test]
    fn function_declaration_does_not_overwrite_outer_binding_after_call() {
        let result = evaluate(
            "let x = 7; \
            const f = function() { \
                function x() { return 3; } \
                return x(); \
            }; \
            f(); \
            x;",
        )
        .unwrap();

        assert_eq!(result, Value::Number(7.0));
    }

    #[test]
    fn function_body_hoists_nested_function_declarations() {
        let result = evaluate(
            "function outer() { \
                return inner(); \
                function inner() { return 4; } \
            } \
            outer();",
        )
        .unwrap();

        assert_eq!(result, Value::Number(4.0));
    }

    #[test]
    fn rejects_try_execution_without_running_its_body() {
        let mut document = crate::javascript::DocumentState::default();
        document.title = "before".to_owned();

        let error = crate::javascript::evaluate_with_document(
            "try { document.title = 'changed'; } finally {}",
            &mut document,
        )
        .unwrap_err();

        assert_eq!(
            error.message,
            "try/catch/finally execution is not implemented yet"
        );
        assert_eq!(document.title, "before");
    }

    #[test]
    fn keeps_contextual_keyword_property_names_working() {
        let value = evaluate("let object = { catch: 3 }; object.catch;").unwrap();

        assert_eq!(value, Value::Number(3.0));
    }
}
