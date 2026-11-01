//! Partial host microtask queue.
//!
//! <https://html.spec.whatwg.org/multipage/timers-and-user-prompts.html#microtask-queuing>
//! <https://html.spec.whatwg.org/mutlipage/webappapis.html#perform-a-microtask-checkpoint>
//!
//! Checkpoints currently run after top-level scripts, not a complete event loop.
//! An engine failure aborts the checkpoint and discards pending microtasks.

use super::{Interpreter, JsError, Value};
use crate::javascript::JsErrorKind;

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
                self.microtasks.push_back(callback);

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
            while let Some(callback) = self.microtasks.pop_front() {
                if let Err(error) = self.dispatch_call(callback, None, Vec::new()) {
                    match error.kind {
                        JsErrorKind::Thrown(_) | JsErrorKind::NativeException(_) => {
                            eprintln!("FreeDOM: uncaught microtask exception: {error}");
                        }

                        _ => return Err(error),
                    }
                }
            }

            Ok(())
        })();

        self.performing_microtask_checkpoint = false;

        result
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::{DocumentScriptSession, DocumentState, JsErrorKind, Value, evaluate};

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
}
