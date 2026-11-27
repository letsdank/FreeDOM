//! Partial execution of try, catch, and finally.
//!
//! <https://tc39.es/ecma262/#sec-try-statement>
//!
//! Explicit throws and classified native exceptions are catchable.
//! Unclassified runtime failures and unsupported engine features propagate
//! without executing handlers.

use super::{Completion, ExecutionFrame, Interpreter, JsError, Statement, Value};
use crate::{JsErrorKind, NativeException, ast::CatchClause};

impl Interpreter {
    pub(super) fn execute_try(
        &mut self,
        body: &[Statement],
        catch: Option<&CatchClause>,
        finally: Option<&[Statement]>,
    ) -> Result<Completion, JsError> {
        let result = self.execute(body);
        let index = self.execution_frames.len();

        // Retain the body completion until catch has installed its binding.
        let (result, ()) = self.with_pending_completion(result, |interpreter| {
            let caught = match (&interpreter.execution_frames[index], catch) {
                (
                    ExecutionFrame::PendingCompletion(Err(JsError {
                        kind: JsErrorKind::Thrown(value),
                        ..
                    })),
                    Some(clause),
                )
                | (
                    ExecutionFrame::PendingCompletion(Err(JsError {
                        kind: JsErrorKind::NativeException(NativeException::Materialized(value)),
                        ..
                    })),
                    Some(clause),
                ) => Some((clause, value.clone())),
                _ => None,
            };

            if let Some((clause, value)) = caught {
                // The pending completion now owns the exception during catch.
                interpreter.clear_exception();
                let result = interpreter.execute_catch(clause, value);
                interpreter.replace_pending_completion(index, result);
            }

            // Engine failures are not JavaScript throws and must not be swallowed.
            let run_finally = match &interpreter.execution_frames[index] {
                ExecutionFrame::PendingCompletion(Err(error)) => matches!(
                    &error.kind,
                    JsErrorKind::Thrown(_) | JsErrorKind::NativeException(_)
                ),
                ExecutionFrame::PendingCompletion(Ok(_)) => true,
                _ => unreachable!("pending try completion must exist"),
            };

            if let (true, Some(finally)) = (run_finally, finally) {
                match interpreter.execute(finally) {
                    Ok(Completion::Normal(_)) => {}
                    abrupt => interpreter.replace_pending_completion(index, abrupt),
                }
            }

            Ok(())
        })?;

        result
    }

    fn execute_catch(&mut self, clause: &CatchClause, value: Value) -> Result<Completion, JsError> {
        if let Some(name) = &clause.binding {
            self.with_shadowed_bindings([(name.clone(), value)], |interpreter| {
                let depth = interpreter.function_depth();

                interpreter.catch_bindings.push((depth, name.clone()));

                let result = interpreter.execute(&clause.body);

                interpreter.catch_bindings.pop();

                result
            })
        } else {
            self.execute(&clause.body)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{DocumentState, JsErrorKind, Value, evaluate, evaluate_with_document};

    #[test]
    fn catches_throw_statement_from_wpt_harness() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../testdata/wpt/resources/testharness.js"
        ));

        let fragment = source
            .lines()
            .map(str::trim)
            .find(|line| *line == "throw value;")
            .expect("WPT throw statement must exist");

        let script = format!(
            "let value = 7; \
            let result = 0; \
            try {{ {fragment} }} \
            catch (error) {{ result = error; }} \
            result;"
        );

        assert_eq!(evaluate(&script).unwrap(), Value::Number(7.0));
    }

    #[test]
    fn catches_object_thrown_through_function_call() {
        let result = evaluate(
            "let result = 0; let marker = { code: 7 }; \
            function fail() { throw marker; } \
            try { fail(); } \
            catch (error) { result = error === marker ? error.code : 0; } \
            finally { result = result + 1; } \
            result;",
        )
        .unwrap();

        assert_eq!(result, Value::Number(8.0));
    }

    #[test]
    fn restores_catch_binding_after_return() {
        let result = evaluate(
            "let error = 9; \
            function f() { \
                try { throw 3; } \
                catch (error) { return error; } \
            } \
            f(); \
            error;",
        )
        .unwrap();

        assert_eq!(result, Value::Number(9.0));
    }

    #[test]
    fn finally_runs_after_uncaught_throw() {
        let mut document = DocumentState::default();

        let error = evaluate_with_document(
            "try { throw 'bad'; } \
            finally { document.title = 'clean'; }",
            &mut document,
        )
        .unwrap_err();

        assert!(matches!(
            error.kind,
            JsErrorKind::Thrown(value) if value.as_string() == Some("bad")
        ));
        assert_eq!(document.title, "clean");
    }

    #[test]
    fn abrupt_finally_overrides_return_and_throw() {
        assert_eq!(
            evaluate(
                "function f() { \
                    try { return 1; } \
                    finally { return 2; } \
                } \
                f();"
            )
            .unwrap(),
            Value::Number(2.0)
        );

        let error = evaluate("try { throw 1; } finally { throw 2; }").unwrap_err();

        assert!(matches!(
            error.kind,
            JsErrorKind::Thrown(value) if value.as_number() == Some(2.0)
        ));
    }

    #[test]
    fn finally_runs_before_continue_and_break() {
        let value = evaluate(
            "let n = 0; \
            for (let i = 0; i < 3; i++) { \
                try { \
                    if (i === 1) continue; \
                    if (i === 2) break; \
                } finally { \
                    n = n + 1; \
                } \
            } \
            n;",
        )
        .unwrap();

        assert_eq!(value, Value::Number(3.0));
    }

    #[test]
    fn unsupported_engine_failure_is_not_caught() {
        let error = evaluate("try { new RegExp(/a/); } catch (e) { 3; }").unwrap_err();

        assert_eq!(error.kind, JsErrorKind::UnsupportedFeature);
    }

    #[test]
    fn unclassified_failure_does_not_run_javascript_handlers() {
        let mut document = DocumentState::default();
        document.title = "before".to_owned();

        let error = evaluate_with_document(
            "let a = []; try { a.pop(1); } \
            catch (e) { document.title = 'catch'; } \
            finally { document.title = 'finally'; }",
            &mut document,
        )
        .unwrap_err();

        assert_eq!(error.kind, JsErrorKind::UnclassifiedRuntime);
        assert_eq!(document.title, "before");
    }

    #[test]
    fn restores_function_local_declared_in_try_block() {
        let value = evaluate(
            "let x = 7; \
            function f() { \
                try { let x = 2; } finally {} \
            } \
            f(); \
            x;",
        )
        .unwrap();

        assert_eq!(value, Value::Number(7.0));
    }

    #[test]
    fn catches_reference_error_and_reads_name_and_message() {
        let result = evaluate(
            "let description = ''; \
            try { missing; } \
            catch (error) { \
                description = error.name + ': ' + error['message']; \
            } \
            description;",
        )
        .unwrap();

        assert_eq!(
            result,
            Value::String("ReferenceError: missing is not defined".to_owned())
        );
    }

    #[test]
    fn stringifies_native_error_using_name_and_message() {
        let result = evaluate(
            "let text = ''; \
            try { missing; } \
            catch (e) { text = '' + e; } \
            text;",
        )
        .unwrap();

        assert_eq!(
            result,
            Value::String("ReferenceError: missing is not defined".to_owned())
        );
    }

    #[test]
    fn reads_error_message_with_wpt_harness_statement() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../testdata/wpt/resources/testharness.js"
        ));

        let fragment = "var message = e.message;";

        assert!(source.contains(fragment));

        let script = format!(
            "let message = ''; \
            try {{ missing; }} \
            catch (e) {{ {fragment} }} \
            message;"
        );

        assert_eq!(
            evaluate(&script).unwrap(),
            Value::String("missing is not defined".to_owned())
        );
    }

    #[test]
    fn native_error_retains_identity_and_mutable_properties() {
        let result = evaluate(
            "let saved = null; \
            let same = false; \
            try { \
                try { missing; } \
                catch (e) { \
                    saved = e; \
                    e.message = 'changed'; \
                    throw e; \
                } \
            } catch (e) { \
                same = e === saved && e.message === 'changed'; \
            } \
            same;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn finally_runs_on_uncaught_reference_error() {
        let mut document = DocumentState::default();

        let error = evaluate_with_document(
            "try { missing; } \
            finally { document.title = 'clean'; }",
            &mut document,
        )
        .unwrap_err();

        assert!(matches!(
            error.kind,
            JsErrorKind::NativeException(value)
                if matches!(value.as_test_value(), Value::NativeError(_))
        ));
        assert_eq!(error.message, "ReferenceError: missing is not defined");
        assert_eq!(document.title, "clean");
    }

    #[test]
    fn native_error_has_object_type_and_accepts_indexed_properties() {
        let result = evaluate(
            "let result = false; \
            try { missing; } \
            catch (e) { \
                e['extra'] = 3; \
                result = typeof e === 'object' && e.extra === 3; \
            } \
            result;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn catches_non_callable_type_error() {
        let result = evaluate(
            "let description = ''; \
            try { let n = 1; n(); } \
            catch (e) { description = e.name + ': ' + e.message; } \
            description;",
        )
        .unwrap();

        assert_eq!(
            result,
            Value::String("TypeError: this value is not callable".to_owned())
        );
    }

    #[test]
    fn evaluates_arguments_before_non_callable_error() {
        let result = evaluate(
            "let count = 0; \
            try { let n = 1; n(count = count + 1); } \
            catch (e) {} \
            count;",
        )
        .unwrap();

        assert_eq!(result, Value::Number(1.0));
    }

    #[test]
    fn argument_exception_precedes_non_callable_error() {
        let result = evaluate(
            "let name = ''; \
            try { let n = 1; n(missing); } \
            catch (e) { name = e.name; } \
            name;",
        )
        .unwrap();

        assert_eq!(result, Value::String("ReferenceError".to_owned()));
    }

    #[test]
    fn finally_runs_on_uncaught_non_callable_type_error() {
        let mut document = DocumentState::default();

        let error = evaluate_with_document(
            "try { let n = 1; n(); } \
            finally { document.title = 'clean'; }",
            &mut document,
        )
        .unwrap_err();

        assert!(matches!(
            error.kind,
            JsErrorKind::NativeException(value)
                if matches!(value.as_test_value(), Value::NativeError(_))
        ));
        assert_eq!(error.message, "TypeError: this value is not callable");
        assert_eq!(document.title, "clean");
    }

    #[test]
    fn rethrows_type_error_using_wpt_harness_statement() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../testdata/wpt/resources/testharness.js"
        ));

        let fragment = "throw e;";

        assert!(source.lines().any(|line| line.trim() == fragment));

        let script = format!(
            "let name = ''; \
            try {{ \
                try {{ let n = 1; n(); }} \
                catch (e) {{ {fragment} }} \
            }} \
            catch (e) {{ name = e.name; }} \
            name;"
        );

        assert_eq!(
            evaluate(&script).unwrap(),
            Value::String("TypeError".to_owned())
        );
    }

    #[test]
    fn restores_shadowed_catch_binding_when_handler_throws() {
        let result = evaluate(
            r#"
                let error = 'outer';

                try {
                    try {
                        throw 'first';
                    } catch (error) {
                        throw 'second';
                    }
                } catch (other) {}

                error;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::String("outer".to_owned()));
    }

    #[test]
    fn closure_keeps_top_level_catch_binding_after_exit() {
        let result = evaluate(
            r#"
                let error = 42;
                let reader;

                try {
                    throw 7;
                } catch (error) {
                    reader = function() {
                        error += 1;
                        return error;
                    };
                }

                reader() === 8 &&
                    reader() === 9 &&
                    error === 42;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn catch_binding_shadows_function_local_and_remains_shared() {
        let result = evaluate(
            r#"
                function create() {
                    let error = 10;
                    let reader;

                    try {
                        throw 3;
                    } catch (error) {
                        reader = function() {
                            error += 2;
                            return error;
                        };

                        error += 1;
                    }

                    return { reader: reader, outer: error };
                }

                let result = create();

                result.outer === 10 &&
                    result.reader() === 6 &&
                    result.reader() === 8;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn nested_catches_capture_the_nearest_binding() {
        let result = evaluate(
            r#"
                let inner;
                let outer;

                try {
                    throw 1;
                } catch (error) {
                    try {
                        throw 2;
                    } catch (error) {
                        inner = function() {
                            return error;
                        };
                    }

                    outer = function() {
                        return error;
                    };
                }

                inner() === 2 && outer() === 1;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn returned_closure_passes_catch_binding_to_nested_closure() {
        let result = evaluate(
            r#"
                let make;

                try {
                    throw 5;
                } catch (error) {
                    make = function() {
                        return function() {
                            return error;
                        };
                    };
                }

                let reader = make();

                reader() === 5;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn catch_scope_does_not_become_the_scope_of_a_called_function() {
        let result = evaluate(
            r#"
                let error = 99;

                function create() {
                    return function() {
                        return error;
                    };
                }

                let reader;

                try {
                    throw 7;
                } catch (error) {
                    reader = create();
                }

                reader() === 99;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn catch_scope_does_not_leak_into_default_parameter_closures() {
        let result = evaluate(
            r#"
                let error = 99;

                function create(reader = function() {
                    return error;
                }) {
                    return reader;
                }

                let reader;

                try {
                    throw 7;
                } catch (error) {
                    reader = create();
                }

                reader() === 99;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }
}
