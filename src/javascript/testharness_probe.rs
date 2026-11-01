//! Diagnostic bootstrap of the local WPT harness in a minimal synthetic host.
//!
//! <https://web-platform-tests.org/writing-tests/testharness-api.html>
//!
//! This is not a WPT runner or a browser environment: `self` is an ordinary
//! object, and the existing native assertion shim remains installed.

use super::{Completion, Interpreter, JsError, Value, parse_program};

pub(crate) fn probe_testharness(source: &str) -> Result<(), JsError> {
    let mut interpreter = Interpreter::new();

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

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::probe_testharness;
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
}
