//! User-function invocation for the interpreter's current subset.
//!
//! ECMAScript function calls:
//! <https://tc39.es/ecma262/#sec-ecmascript-function-objects-call-thisargument-argumentslist>
//!
//! Binding and argument handling remain incomplete.

use super::{Completion, Interpreter, JsError, Statement, UserFunction, Value};

fn function_local_names(statements: &[Statement]) -> Vec<String> {
    let mut names = Vec::new();

    for statement in statements {
        match statement {
            Statement::Declaration { name, .. } | Statement::FunctionDeclaration { name, .. } => {
                if !names.contains(name) {
                    names.push(name.clone());
                }
            }

            Statement::If {
                then_body,
                else_body,
                ..
            } => {
                for name in function_local_names(then_body)
                    .into_iter()
                    .chain(function_local_names(else_body))
                {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
            }

            Statement::While { body, .. } => {
                for name in function_local_names(body) {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
            }

            Statement::For {
                initializer, body, ..
            } => {
                if let Some(initializer) = initializer {
                    for name in function_local_names(std::slice::from_ref(initializer.as_ref())) {
                        if !names.contains(&name) {
                            names.push(name);
                        }
                    }
                }

                for name in function_local_names(body) {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
            }

            Statement::ForOf { binding, body, .. } => {
                if !names.contains(binding) {
                    names.push(binding.clone());
                }

                for name in function_local_names(body) {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
            }

            Statement::Expression(_)
            | Statement::Try { .. }
            | Statement::Return(_)
            | Statement::Throw(_)
            | Statement::Break
            | Statement::Continue => {}
        }
    }

    names
}

impl Interpreter {
    pub(super) fn call_function(
        &mut self,
        function: &UserFunction,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        if function.parameters.len() != arguments.len() {
            return Err(JsError::runtime(format!(
                "TypeError: function expects {} arguments, got {}",
                function.parameters.len(),
                arguments.len(),
            )));
        }

        let mut replaced_bindings = Vec::new();
        let mut local_names = Vec::new();

        for (parameter, argument) in function.parameters.iter().zip(arguments) {
            let previous = self.variables.insert(parameter.clone(), argument);
            replaced_bindings.push((parameter.clone(), previous));
            local_names.push(parameter.clone());
        }

        for name in function_local_names(&function.body) {
            if local_names.iter().any(|local_name| local_name == &name) {
                continue;
            }

            let previous = self.variables.insert(name.clone(), Value::Undefined);
            replaced_bindings.push((name.clone(), previous));
            local_names.push(name.clone());
        }

        self.function_locals.push(local_names);

        let result = self.execute(&function.body);

        self.function_locals.pop();

        for (name, previous) in replaced_bindings.into_iter().rev() {
            if let Some(value) = previous {
                self.variables.insert(name, value);
            } else {
                self.variables.remove(&name);
            }
        }

        result.map(Completion::into_value)
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::{Value, evaluate};

    #[test]
    fn evaluates_user_function_with_arguments() {
        let result = evaluate("const f = function(a, b) { return a + b; }; f(2, 3);").unwrap();

        assert_eq!(result, Value::Number(5.0));
    }

    #[test]
    fn restores_shadowed_binding_after_user_function() {
        let result = evaluate(
            "let x = 9; \
            const f = function(x) { let y = 3; return x + y; }; \
            f(2); x;",
        )
        .unwrap();

        assert_eq!(result, Value::Number(9.0));
    }

    #[test]
    fn restores_outer_parameter_after_nested_function_call() {
        let result = evaluate(
            "const outer = function(x) { \
                const inner = function(x) { return x + 1; }; \
                inner(2); \
                return x; \
            }; \
            outer(7);",
        )
        .unwrap();

        assert_eq!(result, Value::Number(7.0));
    }
}
