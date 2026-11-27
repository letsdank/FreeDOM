//! Partial host microtask queue.
//!
//! <https://html.spec.whatwg.org/multipage/timers-and-user-prompts.html#microtask-queuing>
//! <https://html.spec.whatwg.org/multipage/webappapis.html#perform-a-microtask-checkpoint>
//!
//! Checkpoints currently run after top-level scripts, not a complete event loop.
//! An engine failure aborts the checkpoint and discards pending microtasks.

use super::promise::PromiseReactionJob;
use super::{ExecutionFrame, Interpreter, JsError, RuntimeRoot, Value};
use crate::JsErrorKind;

#[derive(Clone)]
pub(super) enum Microtask {
    Callback(Value),
    PromiseReaction(PromiseReactionJob),
}

impl Microtask {
    pub(super) fn visit_roots(&self, visit: &mut impl FnMut(RuntimeRoot<'_>)) {
        match self {
            Self::Callback(value) => visit(RuntimeRoot::Value(value)),
            Self::PromiseReaction(job) => job.visit_roots(visit),
        }
    }
}

impl Interpreter {
    pub(super) fn queue_microtask(&mut self, arguments: Vec<Value>) -> Result<Value, JsError> {
        let callback = arguments.into_iter().next().unwrap_or(Value::Undefined);

        match callback {
            Value::Function(_)
            | Value::NativeFunction(_)
            | Value::ArrayMethod { .. }
            | Value::DocumentMethod(_)
            | Value::ElementMethod(_)
            | Value::TestContextMethod(_)
            | Value::RegExpTest(_)
            | Value::RegExpExec(_) => {
                self.microtasks.push_back(Microtask::Callback(callback));

                Ok(Value::Undefined)
            }

            Value::ObjectConstructor | Value::ElementConstructor(_) => Err(JsError::unsupported(
                "queueMicrotask with this callable is not implemented",
            )),

            _ => Err(JsError::type_error(
                "queueMicrotask callback is not callable",
            )),
        }
    }

    pub(super) fn perform_microtask_checkpoint(&mut self) -> Result<(), JsError> {
        if self.performing_microtask_checkpoint {
            return Ok(());
        }

        self.performing_microtask_checkpoint = true;

        let result = (|| {
            while let Some(job) = self.microtasks.pop_front() {
                self.consume_step()?;
                self.with_exception_scope(|interpreter| {
                    let job_result =
                        interpreter.with_active_microtask(job, |interpreter, index| {
                            match interpreter.active_microtask(index).clone() {
                                Microtask::Callback(callback) => interpreter
                                    .dispatch_call(callback, None, Vec::new())
                                    .map(|_| ()),

                                Microtask::PromiseReaction(reaction) => {
                                    interpreter.run_promise_reaction(reaction)
                                }
                            }
                        });

                    match job_result {
                        Err(error)
                            if matches!(
                                error.kind,
                                JsErrorKind::Thrown(_) | JsErrorKind::NativeException(_)
                            ) =>
                        {
                            eprintln!("FreeDOM: uncaught microtask exception: {error}");
                            interpreter.clear_exception();
                            Ok(())
                        }

                        result => result,
                    }
                })?;
            }

            Ok(())
        })();

        self.performing_microtask_checkpoint = false;

        result
    }

    fn with_active_microtask<T>(
        &mut self,
        job: Microtask,
        operation: impl FnOnce(&mut Self, usize) -> Result<T, JsError>,
    ) -> Result<T, JsError> {
        let index = self.execution_frames.len();

        self.execution_frames.push(ExecutionFrame::Microtask(job));

        let result = operation(self, index);

        match self.execution_frames.pop() {
            Some(ExecutionFrame::Microtask(_)) => {}
            _ => unreachable!("microtask frame must exist"),
        }

        result
    }

    fn active_microtask(&self, index: usize) -> &Microtask {
        match self.execution_frames.get(index) {
            Some(ExecutionFrame::Microtask(job)) => job,
            _ => unreachable!("microtask frame must exist"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Interpreter, JsError, Microtask};
    use crate::{
        DocumentScriptSession, DocumentState, JsErrorKind, Value, evaluate, parse_program,
    };
    use std::rc::Rc;

    #[test]
    fn handled_job_errors_do_not_replace_an_outer_exception_root() {
        use super::super::ExecutionFrame;

        let mut interpreter = Interpreter::new();
        let Value::Object(outer) = Value::object([]) else {
            unreachable!();
        };
        let program = parse_program(
            r#"
                let marker = {};
                let caught = false;
                let nativeName = '';
                Promise.resolve(1)
                    .then(function() { throw marker; })
                    .then(undefined, function(error) { caught = error === marker; });
                Promise.resolve(1)
                    .then(Promise.resolve)
                    .then(undefined, function(error) { nativeName = error.name; });
                queueMicrotask(function() { throw {}; });
            "#,
        )
        .unwrap();

        interpreter
            .with_exception_scope(|interpreter| {
                interpreter.record_exception(&JsError::thrown(Value::Object(outer.clone())));
                interpreter.execute(&program)?;
                interpreter.perform_microtask_checkpoint()?;

                assert!(interpreter.execution_frames.iter().any(|frame| matches!(
                    frame,
                    ExecutionFrame::ExceptionScope(Some(JsError {
                        kind: JsErrorKind::Thrown(Value::Object(value)),
                        ..
                    })) if Rc::ptr_eq(value, &outer)
                )));
                assert_eq!(
                    *interpreter.variables.get("caught").unwrap(),
                    Value::Boolean(true)
                );
                assert_eq!(
                    *interpreter.variables.get("nativeName").unwrap(),
                    Value::String("TypeError".to_owned())
                );
                Ok(())
            })
            .unwrap();
        assert!(interpreter.execution_frames.is_empty());
    }

    #[test]
    fn runs_microtasks_after_script_and_in_fifo_order() {
        let mut document = DocumentState::with_title("");
        let mut session = DocumentScriptSession::new(&document);

        let result = session
            .evaluate(
                r#"
                    let order = '';

                    queueMicrotask(function() {
                        order += 'a';

                        queueMicrotask(function() {
                            order += 'c';
                        });
                    });

                    queueMicrotask(function() {
                        order += 'b';
                    });

                    order += 's';
                    order;
        "#,
                &mut document,
            )
            .unwrap();

        assert_eq!(result, Value::String("s".to_owned()));

        assert_eq!(
            session.evaluate("order;", &mut document).unwrap(),
            Value::String("sabc".to_owned())
        );
    }

    #[test]
    fn microtask_can_update_document_before_snapshot() {
        let mut document = DocumentState::with_title("before");
        let mut session = DocumentScriptSession::new(&document);

        session
            .evaluate(
                "queueMicrotask(function() { document.title = 'after'; });",
                &mut document,
            )
            .unwrap();

        assert_eq!(document.title, "after");
    }

    #[test]
    fn throwing_microtask_does_not_stop_next_microtask() {
        let mut document = DocumentState::with_title("");
        let mut session = DocumentScriptSession::new(&document);

        session
            .evaluate(
                r#"
            let result = '';

            queueMicrotask(function() {
                throw 'error';
            });

            queueMicrotask(function() {
                result = 'done';
            });
        "#,
                &mut document,
            )
            .unwrap();

        assert_eq!(
            session.evaluate("result;", &mut document).unwrap(),
            Value::String("done".to_owned())
        );
    }

    #[test]
    fn checkpoint_runs_when_script_throws() {
        let mut document = DocumentState::with_title("before");
        let mut session = DocumentScriptSession::new(&document);

        let error = session
            .evaluate(
                r#"
                queueMicrotask(function() {
                    document.title = 'after';
                });

                throw 1;
            "#,
                &mut document,
            )
            .unwrap_err();

        assert!(matches!(error.kind, JsErrorKind::Thrown(_)));
        assert_eq!(document.title, "after");
    }

    #[test]
    fn validates_callback_and_returns_undefined() {
        assert_eq!(
            evaluate("queueMicrotask(function() {});").unwrap(),
            Value::Undefined
        );

        for source in [
            "queueMicrotask();",
            "queueMicrotask(null);",
            "queueMicrotask(5);",
        ] {
            let error = evaluate(source).unwrap_err();

            assert!(
                matches!(error.kind, JsErrorKind::NativeException(_)),
                "{source}"
            );
        }
    }

    #[test]
    fn microtask_keeps_closure_cells_alive() {
        let mut document = DocumentState::with_title("");
        let mut session = DocumentScriptSession::new(&document);

        session
            .evaluate(
                r#"
                function enqueue() {
                    let label = 'captured';

                    queueMicrotask(function() {
                        document.title = label;
                    });

                    label = 'updated';
                }

                enqueue();
            "#,
                &mut document,
            )
            .unwrap();

        assert_eq!(document.title, "updated");
    }

    #[test]
    fn engine_failures_are_not_reported_as_javascript_exceptions() {
        let error = evaluate("queueMicrotask(function() { new Object(); });").unwrap_err();

        assert_eq!(error.kind, JsErrorKind::UnsupportedFeature);
    }

    #[test]
    fn engine_failure_does_not_run_or_leak_pending_tasks() {
        let mut document = DocumentState::with_title("before");
        let mut session = DocumentScriptSession::new(&document);

        let error = session
            .evaluate(
                r#"
                    queueMicrotask(function() {
                        document.tiel = 'unexpected';
                    });

                    new Object();
                "#,
                &mut document,
            )
            .unwrap_err();

        assert_eq!(error.kind, JsErrorKind::UnsupportedFeature);

        session.evaluate("0;", &mut document).unwrap();

        assert_eq!(document.title, "before");
    }

    #[test]
    fn engine_failure_in_a_job_discards_later_jobs() {
        let mut document = DocumentState::with_title("before");
        let mut session = DocumentScriptSession::new(&document);

        let error = session
            .evaluate(
                r#"
                    queueMicrotask(function() {
                        new Object();
                    });

                    queueMicrotask(function() {
                        document.title = 'unexpected';
                    });
                "#,
                &mut document,
            )
            .unwrap_err();

        assert_eq!(error.kind, JsErrorKind::UnsupportedFeature);

        session.evaluate("0;", &mut document).unwrap();

        assert_eq!(document.title, "before");
    }

    #[test]
    fn active_microtask_survives_nested_frames_and_unwinds_on_error() {
        let mut interpreter = Interpreter::new();
        let job = Microtask::Callback(Value::Number(7.0));

        let result = interpreter.with_active_microtask(job, |runtime, index| {
            runtime.with_frame_value(Value::Number(9.0), |runtime, value_index| {
                assert_eq!(runtime.frame_value(value_index), &Value::Number(9.0));

                assert!(matches!(
                    runtime.active_microtask(index),
                    Microtask::Callback(Value::Number(7.0))
                ));

                Err::<(), _>(JsError::resource_limit("test limit"))
            })
        });

        assert!(result.is_err());
        assert!(interpreter.execution_frames.is_empty());
    }
}
