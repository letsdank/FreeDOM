//! Diagnostic bootstrap of the local WPT harness in a minimal synthetic host.
//!
//! <https://web-platform-tests.org/writing-tests/testharness-api.html>
//!
//! This is not a WPT runner or a browser environment: `self` is an ordinary
//! object, and the legacy native WPT assertion shim is not installed.

use super::{HostError, HostValue, Interpreter, JsError, parse_program};

fn execute_host_source(
    interpreter: &mut Interpreter,
    source: &str,
) -> Result<HostValue, HostError> {
    let program = parse_program(source).map_err(|error| interpreter.export_error(error))?;
    interpreter.execute_host_script(&program)
}

fn host_array(
    interpreter: &Interpreter,
    value: &HostValue,
    invalid_message: &str,
) -> Result<Vec<HostValue>, HostError> {
    value
        .array_items()
        .map_err(|_| {
            interpreter.export_error(JsError::runtime("harness report array is borrowed"))
        })?
        .ok_or_else(|| interpreter.export_error(JsError::runtime(invalid_message)))
}

fn host_property(
    interpreter: &Interpreter,
    value: &HostValue,
    name: &str,
) -> Result<HostValue, HostError> {
    value
        .object_own_property(name)
        .map_err(|_| {
            interpreter.export_error(JsError::runtime("harness report object is borrowed"))
        })?
        .ok_or_else(|| {
            interpreter.export_error(JsError::runtime("invalid testharness result fields"))
        })
}

fn bootstrap_testharness(source: &str) -> Result<Interpreter, HostError> {
    let mut interpreter = Interpreter::new_without_wpt_shim();

    execute_host_source(&mut interpreter, "let self = {};")?;
    execute_host_source(&mut interpreter, source)?;

    for name in ["test", "assert_equals", "add_completion_callback"] {
        let result = execute_host_source(&mut interpreter, &format!("self.{name};"))?;

        if !result.is_user_function() {
            return Err(interpreter.export_error(JsError::runtime(format!(
                "testharness did not expose function self.{name}"
            ))));
        }
    }

    Ok(interpreter)
}

pub(crate) fn probe_testharness(source: &str) -> Result<(), HostError> {
    bootstrap_testharness(source).map(|_| ())
}

/// Runs one synchronous harness test in the synthetic shell.
pub(crate) fn probe_testharness_test(source: &str) -> Result<(), HostError> {
    let report = run_harness_scripts(
        source,
        &[r#"
            let freedom_probe_body_ran = false;

            test(function() {
                freedom_probe_body_ran = true;
                assert_equals(1, 1);
            }, 'FreeDOM synchronous harness probe');

            if (!freedom_probe_body_ran) {
                throw 'test body did not run';
            }
        "#],
    )?;

    if report.harness_status == 0
        && report.results.len() == 1
        && report.results[0].name == "FreeDOM synchronous harness probe"
        && report.results[0].status == 0
    {
        Ok(())
    } else {
        Err(HostError {
            offset: None,
            message: "testharness did not report exactly one passing test result".to_owned(),
            kind: super::JsErrorKind::UnclassifiedRuntime,
        })
    }
}

/// A synthetic shell report; this is not an HTML WPT run.
#[derive(Debug, PartialEq)]
pub(crate) struct HarnessReport {
    pub(crate) harness_status: u8,
    pub(crate) results: Vec<HarnessResult>,
}

#[derive(Debug, PartialEq)]
pub(crate) struct HarnessResult {
    pub(crate) name: String,
    pub(crate) status: u8,
}

/// Runs synchronous scripts against the upstream harness in a synthetic shell.
///
/// This forces explicit_done; async tests, HTML, resource loading and real
/// Window globals are not supported.
pub(crate) fn run_harness_scripts(
    source: &str,
    scripts: &[&str],
) -> Result<HarnessReport, HostError> {
    let mut interpreter = bootstrap_testharness(source)?;

    execute_host_source(
        &mut interpreter,
        r#"
            let __freedom_results = [];
            let __freedom_completions = 0;
            let __freedom_harness_status = -1;

            self.add_result_callback(function(result) {
                __freedom_results.push({ name: result.name, status: result.status });
            });

            self.add_completion_callback(function(results, status) {
                __freedom_completions += 1;
                __freedom_harness_status = status.status;
            });

            self.setup(null, { explicit_done: true });

            let test = self.test;
            let assert_equals = self.assert_equals;
        "#,
    )?;

    for source in scripts {
        execute_host_source(&mut interpreter, source)?;
    }

    execute_host_source(&mut interpreter, "self.done()")?;

    let snapshot = execute_host_source(
        &mut interpreter,
        "[__freedom_results, __freedom_completions, __freedom_harness_status];",
    )?;

    let fields = host_array(&interpreter, &snapshot, "invalid harness report")?;

    if fields.len() != 3 || fields[1].as_number() != Some(1.0) {
        return Err(interpreter.export_error(JsError::runtime(
            "testharness did not complete exactly once",
        )));
    }

    let harness_status = match fields[2].as_number() {
        Some(number) if number.fract() == 0.0 && (0.0..=3.0).contains(&number) => number as u8,

        _ => {
            return Err(
                interpreter.export_error(JsError::runtime("invalid testharness completion status"))
            );
        }
    };

    let records = host_array(&interpreter, &fields[0], "invalid testharness result list")?;

    let mut results = Vec::with_capacity(records.len());

    for record in records {
        let name = host_property(&interpreter, &record, "name")?;
        let status = host_property(&interpreter, &record, "status")?;

        let (Some(name), Some(status)) = (name.as_string(), status.as_number()) else {
            return Err(
                interpreter.export_error(JsError::runtime("invalid testharness result fields"))
            );
        };

        if status.fract() != 0.0 || !(0.0..=4.0).contains(&status) {
            return Err(
                interpreter.export_error(JsError::runtime("invalid testharness subtest status"))
            );
        }

        results.push(HarnessResult {
            name: name.to_owned(),
            status: status as u8,
        });
    }

    drop(snapshot);
    Ok(HarnessReport {
        harness_status,
        results,
    })
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

        let thrown = probe_testharness("throw {code: 42};").unwrap_err();
        let JsErrorKind::Thrown(value) = thrown.kind else {
            panic!("expected thrown value")
        };
        let crate::javascript::Value::Object(object) = value.as_test_value() else {
            panic!("expected object")
        };
        assert_eq!(
            object.borrow().get("code"),
            Some(&crate::javascript::Value::Number(42.0))
        );
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
            let completion_callback = null;

            self.add_result_callback = function(callback) {
                result_callback = callback;
            };

            self.assert_equals = function(a, b) {
                if (a !== b) {
                    throw 'not equal';
                }
            };

            self.add_completion_callback = function(callback) {
                completion_callback = callback;
            };

            self.setup = function() {};

            self.done = function() {
                completion_callback([], { status: 0 });
            };

            self.test = function(callback, name) {
                callback();
                result_callback({ name: name, status: 0 });
            };
        "#;

        probe_testharness_test(passing).unwrap();

        let failing = passing.replace("status: 0", "status: 1");
        assert!(probe_testharness_test(&failing).is_err());

        let missing = passing.replace("result_callback({ name: name, status: 0 });", "");
        assert!(probe_testharness_test(&missing).is_err());

        let body_not_run = passing.replace("callback();", "");
        assert!(probe_testharness_test(&body_not_run).is_err());

        let duplicate = passing.replace(
            "result_callback({ name: name, status: 0 });",
            "result_callback({ name: name, status: 0 }); \
            result_callback({ name: name, status: 0 });",
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

    #[test]
    fn runs_multiple_scripts_with_upstream_harness() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        let report = super::run_harness_scripts(
            source,
            &[
                "test(function() { assert_equals(1, 1); }, 'first');",
                "test(function() { assert_equals(2, 2); }, 'second');",
            ],
        )
        .unwrap();

        assert_eq!(report.harness_status, 0);

        assert_eq!(
            report.results,
            vec![
                super::HarnessResult {
                    name: "first".into(),
                    status: 0
                },
                super::HarnessResult {
                    name: "second".into(),
                    status: 0
                },
            ]
        );

        let failed = super::run_harness_scripts(
            source,
            &[
                "test(function() { assert_equals(1, 1); }, 'first');",
                "test(function() { assert_equals(2, 3); }, 'second');",
            ],
        );

        match failed {
            Ok(report) => {
                assert_eq!(report.results.len(), 2);
                assert_eq!(report.results[0].status, 0);
                assert_eq!(report.results[1].status, 1);
            }

            Err(error) => {
                assert!(matches!(
                    error.kind,
                    JsErrorKind::UnsupportedFeature | JsErrorKind::UnclassifiedRuntime
                ));
            }
        }

        let empty = super::run_harness_scripts(source, &[]).unwrap();

        assert_eq!(empty.harness_status, 1);
        assert!(empty.results.is_empty());

        let syntax = super::run_harness_scripts(source, &["let = ;"]).unwrap_err();

        assert_eq!(syntax.kind, JsErrorKind::Syntax);
    }
}
