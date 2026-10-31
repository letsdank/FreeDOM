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

            Statement::Declarations(declarations) => {
                for (name, _) in declarations {
                    if !names.contains(name) {
                        names.push(name.clone());
                    }
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

            Statement::Try {
                body,
                catch,
                finally,
            } => {
                let blocks = std::iter::once(body)
                    .chain(catch.iter().map(|clause| &clause.body))
                    .chain(finally.iter());

                for block in blocks {
                    for name in function_local_names(block) {
                        if !names.contains(&name) {
                            names.push(name);
                        }
                    }
                }
            }

            Statement::Expression(_)
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
        let mut replaced_bindings = Vec::new();
        let mut local_names = Vec::new();

        for parameter in &function.parameters {
            if local_names.contains(&parameter.name) {
                continue;
            }

            let previous = self
                .variables
                .insert(parameter.name.clone(), Value::Undefined);

            replaced_bindings.push((parameter.name.clone(), previous));
            local_names.push(parameter.name.clone());
        }

        let result = (|| -> Result<Completion, JsError> {
            for (index, parameter) in function.parameters.iter().enumerate() {
                let argument = arguments.get(index).cloned().unwrap_or(Value::Undefined);

                let value = match (argument, &parameter.default) {
                    (Value::Undefined, Some(default)) => self.evaluate_expression(default)?,

                    (value, _) => value,
                };

                self.variables.insert(parameter.name.clone(), value);
            }

            for name in function_local_names(&function.body) {
                if local_names.contains(&name) {
                    continue;
                }

                let previous = self.variables.insert(name.clone(), Value::Undefined);

                replaced_bindings.push((name.clone(), previous));
                local_names.push(name);
            }

            self.function_locals.push(local_names);

            let result = self.execute(&function.body);

            self.function_locals.pop();

            result
        })();

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

    #[test]
    fn bare_return_returns_undefined() {
        for source in [
            "function f() { return; } f();",
            "function f() { return } f();",
            "function f() { return\n; } f();",
        ] {
            assert_eq!(evaluate(source).unwrap(), Value::Undefined, "{source:?}");
        }
    }

    #[test]
    fn return_line_terminator_prevents_returning_the_next_expression() {
        for separator in [
            "\n",
            "\r",
            "\r\n",
            "\u{2028}",
            "\u{2029}",
            " /*\n*/ ",
            " // comment\n",
            " // comment\r",
            " // comment\u{2028}",
            " // comment\u{2029}",
        ] {
            let source = format!("function f() {{ return{separator}42; }} f();");

            assert_eq!(evaluate(&source).unwrap(), Value::Undefined, "{source:?}");
        }
    }

    #[test]
    fn return_with_expression_and_comment_returns_the_value() {
        let result = evaluate("function f() { return /* comment */ 42; } f();").unwrap();

        assert_eq!(result, Value::Number(42.0));
    }

    #[test]
    fn return_supports_regexp_literal() {
        let result = evaluate("function f() { return /x/; } f().test('x');").unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn evaluates_default_parameters_in_order() {
        let result = evaluate(
            "function f(a = 4, b = a + 1) { return b; } \
            f() === 5 && f(10) === 11 && f(undefined, 8) === 8;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn defaults_apply_only_to_undefined_and_arguments_are_flexible() {
        let result = evaluate(
            "function f(x = 9) { return x; } \
            f(null) === null && \
            f(0) === 0 && \
            f(undefined) === 9 && \
            f(3, 4) === 3;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn evaluates_default_each_call_and_skips_when_argument_is_supplied() {
        let result = evaluate(
            "let count = 0; \
            function f(x = (count += 1)) { return x; } \
            f(10); \
            f(); \
            f(); \
            count;",
        )
        .unwrap();

        assert_eq!(result, Value::Number(2.0));
    }

    #[test]
    fn restores_parameter_binding_if_default_throws() {
        let result = evaluate(
            "let x = 7; \
            let caught = ''; \
            function f(x = missing) {} \
            try { f(); } catch (e) { caught = e.name; } \
            x === 7 && caught === 'ReferenceError';",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }
}
