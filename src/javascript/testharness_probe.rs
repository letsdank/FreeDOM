//! Diagnostic bootstrap of the local WPT harness in a minimal synthetic host.
//!
//! <https://web-platform-tests.org/writing-tests/testharness-api.html>
//!
//! This is not a WPT runner or a browser environment: `self` is an ordinary
//! object, and the legacy native WPT assertion shim is not installed.

use super::{Completion, Interpreter, JsError, Value, parse_program};

fn bootstrap_testharness(source: &str) -> Result<Interpreter, JsError> {
    let mut interpreter = Interpreter::new_without_wpt_shim();

    interpreter.execute(&parse_program("let self = {};")?)?;

    let program = parse_program(source)?;
    interpreter.execute_script(&program)?;

    for name in ["test", "assert_equals", "add_completion_callback"] {
        let check = parse_program(&format!("self.{name};"))?;
        let result = interpreter.execute(&check)?;

        if !matches!(result, Completion::Normal(Value::Function(_))) {
            return Err(JsError::runtime(format!(
                "testharness did not expose function self.{name}"
            )));
        }
    }

    Ok(interpreter)
}

pub(crate) fn probe_testharness(source: &str) -> Result<(), JsError> {
    bootstrap_testharness(source).map(|_| ())
}

/// Runs one synchronous harness test and checks its result callback.
///
/// This does not verify harness completion or execute a WPT document.
pub(crate) fn probe_testharness_test(source: &str) -> Result<(), JsError> {
    let mut interpreter = bootstrap_testharness(source)?;

    let program = parse_program(
        r#"
            let freedom_probe_count = 0;
            let freedom_probe_status = -1;
            let freedom_probe_body_ran = false;

            self.add_result_callback(function(result) {
                freedom_probe_count = freedom_probe_count + 1;
                freedom_probe_status = result.status;
            });

            self.test(function() {
                freedom_probe_body_ran = true;
                self.assert_equals(1, 1);
            }, 'FreeDOM synchronous harness probe');

            freedom_probe_body_ran &&
                freedom_probe_count === 1 &&
                freedom_probe_status === 0;
        "#,
    )?;

    match interpreter.execute_script(&program)? {
        Completion::Normal(Value::Boolean(true)) => Ok(()),

        _ => Err(JsError::runtime(
            "testharness did not report exactly one passing test result",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{probe_testharness, probe_testharness_test};
    use crate::javascript::JsErrorKind;

    #[test]
    fn rejects_native_shim_as_harness_bootstrap() {
        let error = probe_testharness("").unwrap_err();

        assert_eq!(
            error.message,
            "testharness did not expose function self.test"
        );
    }

    #[test]
    fn accepts_functions_installed_on_synthetic_self() {
        probe_testharness(
            r#"
                self.test = function() {};
                self.assert_equals = function() {};
                self.add_completion_callback = function() {};
            "#,
        )
        .unwrap();
    }

    #[test]
    fn preserves_script_failures() {
        let syntax_error = probe_testharness("let = ;").unwrap_err();

        assert_eq!(syntax_error.kind, JsErrorKind::Syntax);

        let thrown = probe_testharness("throw 7;").unwrap_err();

        assert!(matches!(thrown.kind, JsErrorKind::Thrown(_)));
    }

    #[test]
    fn bootstraps_local_wpt_testharness_in_synthetic_host() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        probe_testharness(source).unwrap();
    }

    #[test]
    fn does_not_install_legacy_wpt_globals() {
        probe_testharness(
            r#"
                if (typeof test !== "undefined" ||
                    typeof assert_equals !== "undefined" ||
                    typeof assert_true !== "undefined" ||
                    typeof assert_false !== "undefined" ||
                    typeof assert_not_equals !== "undefined") {
                    throw "legacy WPT globals are present";
                }

                self.test = function() {};
                self.assert_equals = function() {};
                self.add_completion_callback = function() {};
            "#,
        )
        .unwrap();
    }

    #[test]
    fn synchronous_probe_checks_the_result_callback() {
        let passing = r#"
            let result_callback = null;

            self.add_result_callback = function(callback) {
                result_callback = callback;
            };

            self.assert_equals = function(a, b) {
                if (a !== b) {
                    throw 'not equal';
                }
            };

            self.add_completion_callback = function() {};

            self.test = function(callback) {
                callback();
                result_callback({ status: 0 });
            };
        "#;

        probe_testharness_test(passing).unwrap();

        let failing = passing.replace("status: 0", "status: 1");
        assert!(probe_testharness_test(&failing).is_err());

        let missing = passing.replace("result_callback({ status: 0 });", "");
        assert!(probe_testharness_test(&missing).is_err());

        let body_not_run = passing.replace("callback();", "");
        assert!(probe_testharness_test(&body_not_run).is_err());

        let duplicate = passing.replace(
            "result_callback({ status: 0 });",
            "result_callback({ status: 0 }); result_callback({ status: 0 });",
        );
        assert!(probe_testharness_test(&duplicate).is_err());
    }

    #[test]
    fn runs_synchronous_probe_with_upstream_testharness() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        probe_testharness_test(source).unwrap();
    }
}
