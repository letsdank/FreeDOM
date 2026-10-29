//! Temporary synchronous test and assertion helpers used by the interpreter.
//!
//! <https://web-platform-tests.org/writing-tests/testharness-api.html>
//!
//! These helpers are a partial compatibility shim, not testharness.js itself.

use std::collections::HashMap;

use super::{Cleanup, Interpreter, JsError, Value};

impl Interpreter {
    pub(super) fn run_test(&mut self, arguments: Vec<Value>) -> Result<Value, JsError> {
        if !(1..=2).contains(&arguments.len()) {
            return Err(JsError::runtime(
                "TypeError: test expects a callback and an optional description",
            ));
        }

        let Value::Function(callback) = &arguments[0] else {
            return Err(JsError::runtime(
                "TypeError: test expects a function callback",
            ));
        };

        if callback.parameters.len() > 1 {
            return Err(JsError::runtime(
                "TypeError: test callbacks may accept at most one parameter",
            ));
        }

        let callback_arguments = if callback.parameters.is_empty() {
            Vec::new()
        } else {
            vec![Value::TestContext]
        };

        let cleanup_start = self.cleanups.len();

        let previous_this = self.variables.insert("this".to_owned(), Value::TestContext);

        let test_result = self.call_function(callback, callback_arguments);

        if let Some(previous_this) = previous_this {
            self.variables.insert("this".to_owned(), previous_this);
        } else {
            self.variables.remove("this");
        }

        let cleanup_result = self.run_cleanups_since(cleanup_start);

        test_result?;
        cleanup_result?;

        Ok(Value::Undefined)
    }

    pub(super) fn add_cleanup(&mut self, arguments: Vec<Value>) -> Result<Value, JsError> {
        if arguments.len() != 1 {
            return Err(JsError::runtime(
                "TypeError: add_cleanup expects one callback",
            ));
        }

        let Value::Function(callback) = arguments.into_iter().next().unwrap() else {
            return Err(JsError::runtime(
                "TypeError: add_cleanup expects a function callback",
            ));
        };

        if !callback.parameters.is_empty() {
            return Err(JsError::runtime(
                "TypeError: cleanup callbacks cannot accept parameters",
            ));
        }

        let mut captured = HashMap::new();

        for scope in &self.function_locals {
            for name in scope {
                if let Some(value) = self.variables.get(name) {
                    captured.insert(name.clone(), value.clone());
                }
            }
        }

        self.cleanups.push(Cleanup { callback, captured });

        Ok(Value::Undefined)
    }

    fn run_cleanups_since(&mut self, cleanup_start: usize) -> Result<(), JsError> {
        let mut first_error = None;

        while self.cleanups.len() > cleanup_start {
            let cleanup = self.cleanups.pop().unwrap();
            let mut replaced_bindings = Vec::new();

            for (name, value) in cleanup.captured {
                let previous = self.variables.insert(name.clone(), value);
                replaced_bindings.push((name, previous));
            }

            let result = self.call_function(&cleanup.callback, Vec::new());

            for (name, previous) in replaced_bindings.into_iter().rev() {
                if let Some(value) = previous {
                    self.variables.insert(name, value);
                } else {
                    self.variables.remove(&name);
                }
            }

            if let Err(error) = result {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }

        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn assertion_description(arguments: &[Value], index: usize) -> String {
        arguments
            .get(index)
            .map(Value::to_js_string)
            .filter(|message| !message.is_empty())
            .map(|message| format!(": {message}"))
            .unwrap_or_default()
    }

    pub(super) fn assert_equals(&mut self, arguments: Vec<Value>) -> Result<Value, JsError> {
        if !(2..=3).contains(&arguments.len()) {
            return Err(JsError::runtime(
                "TypeError: assert_equals expects two or three arguments",
            ));
        }

        let actual = &arguments[0];
        let expected = &arguments[1];

        if Self::same_value(actual, expected) {
            return Ok(Value::Undefined);
        }

        let description = Self::assertion_description(&arguments, 2);

        Err(JsError::runtime(format!(
            "AssertionError{description}; expected {expected:?}, got {actual:?}"
        )))
    }

    pub(super) fn assert_true(&mut self, arguments: Vec<Value>) -> Result<Value, JsError> {
        if !(1..=2).contains(&arguments.len()) {
            return Err(JsError::runtime(
                "TypeError: assert_true expects one or two arguments",
            ));
        }

        if arguments[0] == Value::Boolean(true) {
            return Ok(Value::Undefined);
        }

        let description = Self::assertion_description(&arguments, 1);

        Err(JsError::runtime(format!(
            "AssertionError{description}; expected true, got {:?}",
            arguments[0]
        )))
    }

    pub(super) fn assert_false(&mut self, arguments: Vec<Value>) -> Result<Value, JsError> {
        if !(1..=2).contains(&arguments.len()) {
            return Err(JsError::runtime(
                "TypeError: assert_false expects one or two arguments",
            ));
        }

        if arguments[0] == Value::Boolean(false) {
            return Ok(Value::Undefined);
        }

        let description = Self::assertion_description(&arguments, 1);

        Err(JsError::runtime(format!(
            "AssertionError{description}; expected false, got {:?}",
            arguments[0]
        )))
    }

    pub(super) fn assert_not_equals(&mut self, arguments: Vec<Value>) -> Result<Value, JsError> {
        if !(2..=3).contains(&arguments.len()) {
            return Err(JsError::runtime(
                "TypeError: assert_not_equals expects two or three arguments",
            ));
        }

        if !Self::same_value(&arguments[0], &arguments[1]) {
            return Ok(Value::Undefined);
        }

        let description = Self::assertion_description(&arguments, 2);

        Err(JsError::runtime(format!(
            "AssertionError{description}; values must differ, got {:?}",
            arguments[0]
        )))
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::{DocumentState, evaluate_with_document};

    #[test]
    fn cleanup_callbacks_run_in_reverse_order() {
        let mut document = DocumentState::with_title("");

        evaluate_with_document(
            r#"
                test(function() {
                    this.add_cleanup(function() {
                        document.title = document.title + "1";
                    });
                    
                    this.add_cleanup(function() {
                        document.title = document.title + "2";
                    });
                });
            "#,
            &mut document,
        )
        .unwrap();

        assert_eq!(document.title, "21");
    }

    #[test]
    fn all_cleanups_run_and_body_error_takes_priority() {
        let mut document = DocumentState::with_title("");

        let error = evaluate_with_document(
            r#"
                test(function() {
                    this.add_cleanup(function() {
                        document.title = "second";
                    });
                    
                    this.add_cleanup(function() {
                        document.title = "first";
                        assert_equals(1, 2, "cleanup failure");
                    });
                    
                    assert_equals(1, 2, "body failure");
                });
            "#,
            &mut document,
        )
        .unwrap_err();

        assert!(error.message.contains("body failure"));
        assert_eq!(document.title, "second");
    }
}
