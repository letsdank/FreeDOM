//! Partial execution of try, catch, and finally.
//!
//! <https://tc39.es/ecma262/#sec-try-statement>
//!
//! Explicit throws and classified native exceptions are catchable.
//! Unclassified runtime failures and unsupported engine features propagate
//! without executing handlers.

use super::{Completion, Interpreter, JsError, Statement, Value};
use crate::javascript::{JsErrorKind, ast::CatchClause};

impl Interpreter {
    pub(super) fn execute_try(
        &mut self,
        body: &[Statement],
        catch: Option<&CatchClause>,
        finally: Option<&[Statement]>,
    ) -> Result<Completion, JsError> {
        let result = self.execute(body);

        let result = match catch {
            Some(clause) => match result {
                Err(JsError {
                    kind: JsErrorKind::Thrown(value),
                    ..
                })
                | Err(JsError {
                    kind: JsErrorKind::NativeException(value),
                    ..
                }) => self.execute_catch(clause, value),

                other => other,
            },

            None => result,
        };

        // Engine failures are not JavaScript throws and must not be swallowed.
        if let Err(error) = &result {
            if !matches!(
                &error.kind,
                JsErrorKind::Thrown(_) | JsErrorKind::NativeException(_)
            ) {
                return result;
            }
        }

        if let Some(finally) = finally {
            match self.execute(finally)? {
                Completion::Normal(_) => {}
                abrupt => return Ok(abrupt),
            }
        }

        result
    }

    fn execute_catch(&mut self, clause: &CatchClause, value: Value) -> Result<Completion, JsError> {
        if let Some(name) = &clause.binding {
            let previous = self.variables.insert(name.clone(), value);
            let result = self.execute(&clause.body);

            if let Some(previous) = previous {
                self.variables.insert(name.clone(), previous);
            } else {
                self.variables.remove(name);
            }

            result
        } else {
            self.execute(&clause.body)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::{DocumentState, JsErrorKind, Value, evaluate, evaluate_with_document};

    #[test]
    fn catches_throw_statement_from_wpt_harness() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
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
            "let result = 0; \
            function fail() { throw { code: 7 }; } \
            try { fail(); } \
            catch (error) { result = error.code; } \
            result;",
        )
        .unwrap();

        assert_eq!(result, Value::Number(7.0));
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
            JsErrorKind::Thrown(Value::String(value)) if value == "bad"
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
            JsErrorKind::Thrown(Value::Number(2.0))
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
            "let n = 1; try { n(); } \
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
            "/testdata/wpt/resources/testharness.js"
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
            JsErrorKind::NativeException(Value::NativeError(_))
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
}
