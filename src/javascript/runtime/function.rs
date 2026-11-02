//! User-function invocation for the interpreter's current subset.
//!
//! ECMAScript function calls:
//! <https://tc39.es/ecma262/#sec-ecmascript-function-objects-call-thisargument-argumentslist>
//!
//! Binding and argument handling remain incomplete.

use super::{Completion, FunctionValue, Interpreter, JsError, Statement, UserFunction, Value};
use crate::javascript::FunctionKind;

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

            Statement::ForOf { binding, body, .. } | Statement::ForIn { binding, body, .. } => {
                if !names.contains(binding) {
                    names.push(binding.clone());
                }

                for name in function_local_names(body) {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
            }

            Statement::ForOfArray { bindings, body, .. } => {
                for name in bindings.iter().cloned().chain(function_local_names(body)) {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
            }

            Statement::ForInExisting { body, .. } => {
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

            Statement::Switch { cases, .. } => {
                for case in cases {
                    for name in function_local_names(&case.body) {
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
    pub(super) fn call_function_with_receiver(
        &mut self,
        function: &UserFunction,
        arguments: Vec<Value>,
        receiver: Option<Value>,
    ) -> Result<Value, JsError> {
        let Some(receiver) = receiver.filter(|_| function.kind == FunctionKind::Ordinary) else {
            return self.call_function(function, arguments);
        };

        let previous_this = self.variables.shadow("this".to_owned(), receiver);
        let result = self.call_function(function, arguments);

        self.variables.restore(previous_this);

        result
    }

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
                .shadow(parameter.name.clone(), Value::Undefined);

            replaced_bindings.push(previous);
            local_names.push(parameter.name.clone());
        }

        self.function_locals.push(local_names.clone());

        let result = (|| -> Result<Completion, JsError> {
            for (index, parameter) in function.parameters.iter().enumerate() {
                let value = if parameter.is_rest {
                    Value::array(arguments.iter().skip(index).cloned().collect())
                } else {
                    let argument = arguments.get(index).cloned().unwrap_or(Value::Undefined);

                    match (argument, &parameter.default) {
                        (Value::Undefined, Some(default)) => self.evaluate_expression(default)?,

                        (value, _) => value,
                    }
                };

                self.variables
                    .assign_existing(&parameter.name, value)
                    .map_err(|_| {
                        JsError::runtime("internal error: parameter binding is missing")
                    })?;
            }

            for name in function_local_names(&function.body) {
                if local_names.contains(&name) {
                    continue;
                }

                let previous = self.variables.shadow(name.clone(), Value::Undefined);

                replaced_bindings.push(previous);
                local_names.push(name);
            }

            *self
                .function_locals
                .last_mut()
                .expect("function frame must exist") = local_names;

            self.execute(&function.body)
        })();

        self.function_locals.pop();

        for previous in replaced_bindings.into_iter().rev() {
            self.variables.restore(previous);
        }

        result.map(Completion::into_value)
    }

    pub(super) fn create_function(&self, function: &UserFunction) -> Value {
        let referenced = super::references::referenced_names(function);

        let mut captures = self.capture_frames.last().cloned().unwrap_or_default();

        captures.retain(|name, _| referenced.contains(name));

        if let Some(names) = self.function_locals.last() {
            captures.extend(self.variables.capture_names(names, &referenced));
        }

        let catch_names: Vec<_> = self
            .catch_bindings
            .iter()
            .filter(|(depth, _)| *depth == self.function_locals.len())
            .map(|(_, name)| name.clone())
            .collect();

        captures.extend(self.variables.capture_names(&catch_names, &referenced));

        Value::function(function.clone(), self.object_prototype.clone(), captures)
    }

    pub(super) fn call_function_value_with_receiver(
        &mut self,
        function: &FunctionValue,
        arguments: Vec<Value>,
        receiver: Option<Value>,
    ) -> Result<Value, JsError> {
        if function.definition().kind == FunctionKind::Async {
            return Err(JsError::unsupported(
                "async function invocation is not implemented",
            ));
        }

        let previous = self.variables.install_captures(function.captures());

        self.capture_frames.push(function.captures().clone());

        let result = self.call_function_with_receiver(function.definition(), arguments, receiver);

        self.capture_frames.pop();

        for binding in previous.into_iter().rev() {
            self.variables.restore(binding);
        }

        result
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

    #[test]
    fn collects_remaining_arguments_in_a_fresh_array() {
        let result = evaluate(
            "function f(a, ...rest) { return rest; } \
            let x = f(1, 2, 3); \
            let y = f(1, 2, 3); \
            x[0] = 9; \
            x.length === 2 && \
            x[0] === 9 && \
            x[1] === 3 && \
            y[0] === 2 && \
            x !== y;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn rest_parameter_handles_missing_arguments_and_defaults() {
        let result = evaluate(
            "function f(a = 2, ...rest) { \
                return a + rest.length; \
            } \
            f() === 2 && \
            f(undefined, 4, 5) === 4 && \
            f(7) === 7;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn rest_elements_keep_object_identity() {
        let result = evaluate(
            "let object = {}; \
            function f(...args) { return args[0]; } \
            f(object) === object;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn restores_outer_rest_binding_on_default_failure() {
        let result = evaluate(
            "let rest = 42; \
            let error = ''; \
            function f(a = missing, ...rest) {} \
            try { f(); } catch (e) { error = e.name; } \
            rest === 42 && error === 'ReferenceError';",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn function_identity_is_shared_by_aliases_but_not_identical_definitions() {
        let result = evaluate(
            "let f = function() { return 1; }; \
            let alias = f; \
            let other = function() { return 1; }; \
            let obj = { f }; \
            f === alias && obj.f === f && f !== other;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn evaluating_the_same_function_expression_twice_creates_distinct_functions() {
        let result = evaluate(
            "function make() { \
                return function() { return 1; }; \
            } \
            let first = make(); \
            let second = make(); \
            first !== second && \
            first() === 1 && \
            second() === 1;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn same_value_assertions_use_function_identity() {
        let result = evaluate(
            "let f = function() {}; \
            let alias = f; \
            let other = function() {}; \
            assert_equals(f, alias); \
            assert_not_equals(f, other); \
            true;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn ordinary_function_prototype_is_shared_and_replaceable() {
        let result = evaluate(
            "function F() {} \
            let alias = F; \
            let original = F.prototype; \
            original.x = 3; \
            let before = alias['prototype'].x; \
            let replacement = Object.create(null); \
            F['prototype'] = replacement; \
            before === 3 && \
            alias.prototype === replacement && \
            original !== replacement;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn ordinary_function_get_distinct_prototypes_and_arrows_start_without_one() {
        let result = evaluate(
            "let f = function() {}; \
            let g = function() {}; \
            let a = () => 1; \
            let b = x => x; \
            let empty = \
                a.prototype === undefined && \
                b.prototype === undefined; \
            a.prototype = 7; \
            empty && \
            a.prototype === 7 && \
            f.prototype !== g.prototype;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn assigning_function_properties_through_aliases_is_observable() {
        let result = evaluate(
            "function f() {} \
            let alias = f; \
            alias['count'] = 1; \
            f.count += 2; \
            alias.count === 3;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn executes_prototype_assignment_statement_from_testharness() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        let fragment = "DedicatedWorkerTestEnvironment.prototype = Object.create(WorkerTestEnvironment.prototype);";

        assert!(source.contains(fragment));

        let script = format!(
            "function WorkerTestEnvironment() {{}} \
            function DedicatedWorkerTestEnvironment() {{}} \
            WorkerTestEnvironment.prototype.marker = 42; \
            {fragment} \
            DedicatedWorkerTestEnvironment.prototype.marker === 42;"
        );

        assert_eq!(evaluate(&script).unwrap(), Value::Boolean(true));
    }

    #[test]
    fn debug_format_of_self_referencing_function_is_finite() {
        let error = evaluate(
            "function f() {} \
            f.self = f; \
            assert_not_equals(f, f);",
        )
        .unwrap_err();

        assert!(error.message.contains("FunctionValue"));
    }

    #[test]
    fn restores_parameter_binding_when_default_initializer_throws() {
        let result = evaluate(
            r#"
                let value = 9;

                function f(value = missing) {
                    return value;
                }

                try {
                    f();
                } catch (error) {}

                value;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Number(9.0));
    }

    #[test]
    fn returned_closures_share_cells_but_separate_calls_do_not() {
        let result = evaluate(
            r#"
                function make() {
                    let count = 0;

                    return {
                        inc: function() {
                            count += 1;
                            return count;
                        },

                        read: function() {
                            return count;
                        }
                    };
                }

                let first = make();
                let second = make();

                first.inc() === 1 &&
                    first.inc() === 2 &&
                    first.read() === 2 &&
                    second.read() === 0 &&
                    second.inc() === 1 &&
                    first.read() === 2;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn nested_closure_keeps_captures_of_its_creator() {
        let result = evaluate(
            r#"
                function outer() {
                    let count = 10;

                    return function() {
                        let step = 2;

                        return function() {
                            count += step;
                            return count;
                        };
                    };
                }

                let middle = outer();
                let inner = middle();

                inner() === 12 && inner() === 14;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn restores_caller_bindings_when_closure_throws() {
        let result = evaluate(
            r#"
                function make() {
                    let x = 3;

                    return function() {
                        x += 1;
                        throw x;
                    };
                }

                let x = 99;
                let f = make();
                let caught = 0;

                try {
                    f();
                } catch (e) {
                    caught = e;
                }

                x === 99 && caught === 4;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn closures_work_with_call_bind_and_construction() {
        let result = evaluate(
            r#"
                function make(base) {
                    return function(n) {
                        this.value = base + n;
                        return this.value;
                    };
                }

                let f = make(10);
                let a = {};
                let b = {};

                let bound = f.bind(b, 2);
                let called = f.call(a, 1);
                let bound_result = bound();
                let instance = new f(3);

                called === 11 &&
                    a.value === 11 &&
                    bound_result === 12 &&
                    b.value === 12 &&
                    instance.value === 13;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn closure_created_in_default_parameter_keeps_parameter_binding() {
        let result = evaluate(
            r#"
                function make(value, reader = function() {
                    return value;
                }) {
                    return reader;
                }

                let first = make(3);
                let second = make(5);

                first() === 3 && second() === 5;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn creates_async_functions_but_does_not_execute_them() {
        use crate::javascript::{DocumentScriptSession, DocumentState, JsErrorKind};

        let mut document = DocumentState::with_title("");
        let mut session = DocumentScriptSession::new(&document);

        let result = session
            .evaluate(
                "let count = 0; \
            async function f(x = (count = 1)) { \
                count = 2; \
                return await x; \
            } \
            typeof f === 'function' && f.prototype === undefined;",
                &mut document,
            )
            .unwrap();

        assert_eq!(result, Value::Boolean(true));

        let error = session.evaluate("f();", &mut document).unwrap_err();

        assert_eq!(error.kind, JsErrorKind::UnsupportedFeature);

        assert_eq!(
            session.evaluate("count;", &mut document).unwrap(),
            Value::Number(0.0)
        );

        assert!(matches!(
            evaluate("async function f() {} new f();").unwrap_err().kind,
            JsErrorKind::NativeException(_)
        ));
    }
}
