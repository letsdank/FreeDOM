//! Temporary synchronous test and assertion helpers used by the interpreter.
//!
//! <https://web-platform-tests.org/writing-tests/testharness-api.html>
//!
//! These helpers are a partial compatibility shim, not testharness.js itself.

use std::collections::HashMap;

use super::{Cleanup, Completion, Interpreter, JsError, Value};
use crate::javascript::{JsErrorKind, NativeException};

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

        if callback.is_bound() {
            return Err(JsError::unsupported(
                "bound test callbacks are not implemented",
            ));
        }

        if callback.definition().parameters.len() > 1 {
            return Err(JsError::runtime(
                "TypeError: test callbacks may accept at most one parameter",
            ));
        }

        let callback_arguments = if callback.definition().parameters.is_empty() {
            Vec::new()
        } else {
            vec![Value::TestContext]
        };

        let cleanup_start = self.cleanups.len();

        let test_result =
            self.with_shadowed_bindings([("this".to_owned(), Value::TestContext)], |interpreter| {
                interpreter.call_function_value_with_receiver(callback, callback_arguments, None)
            });

        let pending = test_result.map(Completion::Normal);

        let (test_result, cleanup_result) = self
            .with_pending_completion(pending, |interpreter| {
                Ok(interpreter.run_cleanups_since(cleanup_start))
            })?;

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

        if callback.is_bound() {
            return Err(JsError::unsupported(
                "bound cleanup callbacks are not implemented",
            ));
        }

        if !callback.definition().parameters.is_empty() {
            return Err(JsError::runtime(
                "TypeError: cleanup callbacks cannot accept parameters",
            ));
        }

        let mut captured = HashMap::new();

        for frame in &self.execution_frames {
            let super::ExecutionFrame::ActiveCall(frame) = frame else {
                continue;
            };

            let Some(names) = &frame.local_names else {
                continue;
            };

            for name in names {
                if let Some(value) = self.variables.get(name) {
                    captured.insert(name.clone(), value.clone());
                }
            }
        }

        self.cleanups.push(Cleanup { callback, captured });

        Ok(Value::Undefined)
    }

    fn run_cleanups_since(&mut self, cleanup_start: usize) -> Result<(), JsError> {
        self.with_temporary_values_scope(|interpreter, frame_index| {
            let mut first_error = None;

            while interpreter.cleanups.len() > cleanup_start {
                let cleanup = interpreter.cleanups.pop().unwrap();

                let result = interpreter.with_shadowed_bindings(cleanup.captured, |interpreter| {
                    interpreter.call_function_value_with_receiver(
                        &cleanup.callback,
                        Vec::new(),
                        None,
                    )
                });

                if let Err(error) = result {
                    if first_error.is_none() {
                        match &error.kind {
                            JsErrorKind::Thrown(value) | JsErrorKind::NativeException(NativeException::Materialized(value)) => {
                                interpreter
                                    .frame_values_mut(frame_index)
                                    .push(value.clone());
                            }

                            JsErrorKind::NativeException(NativeException::Pending { .. })
                            | JsErrorKind::Syntax
                            | JsErrorKind::UnclassifiedRuntime
                            | JsErrorKind::UnsupportedFeature
                            | JsErrorKind::ResourceLimit => {}
                        }

                        first_error = Some(error);
                    }
                }
            }

            match first_error {
                Some(error) => Err(error),
                None => Ok(()),
            }
        })
    }

    fn assertion_description(&self, arguments: &[Value], index: usize) -> String {
        arguments
            .get(index)
            .map(|value| value.to_js_string(&self.host_roots))
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

        let description = self.assertion_description(&arguments, 2);

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

        let description = self.assertion_description(&arguments, 1);

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

        let description = self.assertion_description(&arguments, 1);

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

        let description = self.assertion_description(&arguments, 2);

        Err(JsError::runtime(format!(
            "AssertionError{description}; values must differ, got {:?}",
            arguments[0]
        )))
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::{DocumentState, Value, evaluate_with_document};

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

    #[test]
    fn thrown_cleanup_value_survives_later_cleanup() {
        let mut document = DocumentState::with_title("");

        let result = evaluate_with_document(
            r#"
                let result = 0;

                try {
                    test(function() {
                        this.add_cleanup(function() {
                            document.title = 'later';
                        });

                        this.add_cleanup(function() {
                            throw {code: 42};
                        });
                    });
                } catch (error) {
                    result = error.code;
                }

                result;
            "#,
            &mut document,
        )
        .unwrap();

        assert_eq!(result, Value::Number(42.0));
        assert_eq!(document.title, "later");
    }
}
