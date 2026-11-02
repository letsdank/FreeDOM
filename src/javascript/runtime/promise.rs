//! Partial Promise implementation.
//!
//! <https://tc39.es/ecma262/#sec-promise.resolve>
//! <https://tc39.es/ecma262/#sec-performpromisethen>
//!
//! Construction, thenable assimilation, species, and unhandled rejection
//! reporting are not implemented. Resolution currently accepts primitives.

use std::{cell::RefCell, fmt, rc::Rc};

use super::{Interpreter, JsError, NativeFunction, Value, microtasks::Microtask};
use crate::javascript::JsErrorKind;

#[derive(Clone)]
pub(in crate::javascript) struct PromiseHandle(Rc<RefCell<PromiseState>>);

impl fmt::Debug for PromiseHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("PromiseHandle")
            .field(&Rc::as_ptr(&self.0))
            .finish()
    }
}

impl PartialEq for PromiseHandle {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone)]
enum Outcome {
    Fulfilled(Value),
    Rejected(Value),
}

#[derive(Default)]
struct PromiseState {
    outcome: Option<Outcome>,
    reactions: Vec<Reaction>,
}

struct Reaction {
    on_fulfilled: Option<Value>,
    on_rejected: Option<Value>,
    target: PromiseHandle,
}

pub(super) struct PromiseReactionJob {
    outcome: Outcome,
    reaction: Reaction,
}

impl Interpreter {
    fn new_pending_promise(&self) -> (Value, PromiseHandle) {
        let handle = PromiseHandle(Rc::new(RefCell::new(PromiseState::default())));
        let promise = Value::object_with_prototype(self.promise_prototype.clone());

        let Value::Object(object) = &promise else {
            unreachable!("promise must be an object");
        };

        object.borrow_mut().promise = Some(handle.clone());

        (promise, handle)
    }

    fn promise_handle(value: &Value) -> Option<PromiseHandle> {
        match value {
            Value::Object(object) => object.borrow().promise.clone(),
            _ => None,
        }
    }

    fn require_promise_constructor(receiver: Option<Value>) -> Result<(), JsError> {
        if matches!(
            receiver,
            Some(Value::NativeFunction(NativeFunction::PromiseConstructor))
        ) {
            Ok(())
        } else {
            Err(JsError::type_error(
                "Promise method receiver is not the Promise constructor",
            ))
        }
    }

    pub(super) fn promise_resolve(
        &mut self,
        receiver: Option<Value>,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        Self::require_promise_constructor(receiver)?;

        let value = arguments.into_iter().next().unwrap_or(Value::Undefined);

        if Self::promise_handle(&value).is_some() {
            return Ok(value);
        }

        Self::require_primitive(&value)?;

        let (promise, handle) = self.new_pending_promise();

        self.settle_promise(&handle, Outcome::Fulfilled(value));

        Ok(promise)
    }

    pub(super) fn promise_reject(
        &mut self,
        receiver: Option<Value>,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        Self::require_promise_constructor(receiver)?;

        let reason = arguments.into_iter().next().unwrap_or(Value::Undefined);

        let (promise, handle) = self.new_pending_promise();

        self.settle_promise(&handle, Outcome::Rejected(reason));

        Ok(promise)
    }

    fn require_primitive(value: &Value) -> Result<(), JsError> {
        match value {
            Value::Undefined
            | Value::Null
            | Value::Boolean(_)
            | Value::Number(_)
            | Value::String(_) => Ok(()),

            _ => Err(JsError::unsupported(
                "Promise resolution of objects and thenables is not implemented",
            )),
        }
    }

    fn handler(value: Value) -> Result<Option<Value>, JsError> {
        match value {
            Value::Function(_)
            | Value::NativeFunction(_)
            | Value::ArrayMethod { .. }
            | Value::DocumentMethod(_)
            | Value::ElementMethod(_)
            | Value::TestContextMethod(_)
            | Value::RegExpTest(_)
            | Value::RegExpExec(_) => Ok(Some(value)),

            Value::ObjectConstructor | Value::ElementConstructor(_) => Err(JsError::unsupported(
                "Promise handler of this callable type is not implemented",
            )),

            _ => Ok(None),
        }
    }

    pub(super) fn promise_then(
        &mut self,
        receiver: Option<Value>,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        let source = receiver
            .as_ref()
            .and_then(Self::promise_handle)
            .ok_or_else(|| {
                JsError::type_error("Promise.prototype.then receiver is not a Promise")
            })?;

        let mut arguments = arguments.into_iter();

        let on_fulfilled = Self::handler(arguments.next().unwrap_or(Value::Undefined))?;
        let on_rejected = Self::handler(arguments.next().unwrap_or(Value::Undefined))?;

        let (promise, target) = self.new_pending_promise();

        let reaction = Reaction {
            on_fulfilled,
            on_rejected,
            target,
        };

        let mut state = source.0.borrow_mut();

        if let Some(outcome) = state.outcome.clone() {
            self.microtasks
                .push_back(Microtask::PromiseReaction(PromiseReactionJob {
                    outcome,
                    reaction,
                }));
        } else {
            state.reactions.push(reaction);
        }

        Ok(promise)
    }

    fn settle_promise(&mut self, promise: &PromiseHandle, outcome: Outcome) {
        let reactions = {
            let mut state = promise.0.borrow_mut();

            if state.outcome.is_some() {
                return;
            }

            state.outcome = Some(outcome.clone());

            std::mem::take(&mut state.reactions)
        };

        for reaction in reactions {
            self.microtasks
                .push_back(Microtask::PromiseReaction(PromiseReactionJob {
                    outcome: outcome.clone(),
                    reaction,
                }));
        }
    }

    pub(super) fn run_promise_reaction(&mut self, job: PromiseReactionJob) -> Result<(), JsError> {
        let PromiseReactionJob { outcome, reaction } = job;

        let handler = match &outcome {
            Outcome::Fulfilled(_) => reaction.on_fulfilled,
            Outcome::Rejected(_) => reaction.on_rejected,
        };

        let Some(handler) = handler else {
            self.settle_promise(&reaction.target, outcome);
            return Ok(());
        };

        let argument = match outcome {
            Outcome::Fulfilled(value) | Outcome::Rejected(value) => value,
        };

        match self.dispatch_call(handler, None, vec![argument]) {
            Ok(value) => {
                if Self::promise_handle(&value).as_ref() == Some(&reaction.target) {
                    self.settle_promise(
                        &reaction.target,
                        Outcome::Rejected(Value::native_error(
                            "TypeError",
                            "promise cannot resolve to itself",
                        )),
                    );
                } else {
                    Self::require_primitive(&value)?;

                    self.settle_promise(&reaction.target, Outcome::Fulfilled(value));
                }
            }

            Err(error) => match error.kind {
                JsErrorKind::Thrown(value) | JsErrorKind::NativeException(value) => {
                    self.settle_promise(&reaction.target, Outcome::Rejected(value));
                }

                _ => return Err(error),
            },
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::{DocumentScriptSession, DocumentState, JsErrorKind, Value, evaluate};

    #[test]
    fn schedules_then_as_microtasks_and_chains_values() {
        let mut doc = DocumentState::with_title("");
        let mut session = DocumentScriptSession::new(&doc);

        let result = session
            .evaluate(
                r#"
                    let log = '';

                    let p = Promise.resolve(1);

                    let next = p.then(function(x) {
                        log += x;
                        return 2;
                    });

                    next.then(function(x) {
                        log += x;
                    });

                    queueMicrotask(function() {
                        log += 'q';
                    });

                    log += 's';
                    log;
                "#,
                &mut doc,
            )
            .unwrap();

        assert_eq!(result, Value::String("s".into()));

        assert_eq!(
            session.evaluate("log;", &mut doc).unwrap(),
            Value::String("s1q2".into())
        );
    }

    #[test]
    fn thrown_handler_rejects_and_rejection_handler_recovers() {
        let mut doc = DocumentState::with_title("");
        let mut session = DocumentScriptSession::new(&doc);

        session
            .evaluate(
                r#"
            let log = '';

            Promise.resolve(1)
                .then(function(x) {
                    throw x + 1;
                })
                .then(undefined, function(error) {
                    log += error;
                    return 3;
                })
                .then(function(value) {
                    log += value;
                });
        "#,
                &mut doc,
            )
            .unwrap();

        assert_eq!(
            session.evaluate("log;", &mut doc).unwrap(),
            Value::String("23".into())
        );
    }

    #[test]
    fn missing_handlers_forward_fulfillment_and_rejection() {
        let mut doc = DocumentState::with_title("");
        let mut session = DocumentScriptSession::new(&doc);

        session
            .evaluate(
                r#"
                let log = '';

                Promise.resolve('a')
                    .then(null)
                    .then(function(x) {
                        log += x;
                    });

                Promise.reject('b')
                    .then()
                    .then(undefined, function(e) {
                        log += e;
                    });
            "#,
                &mut doc,
            )
            .unwrap();

        assert_eq!(
            session.evaluate("log;", &mut doc).unwrap(),
            Value::String("ab".into())
        );
    }

    #[test]
    fn resolve_preserves_promise_identity_and_then_creates_a_new_promise() {
        let result = evaluate(
            r#"
                let p = Promise.resolve(1);

                Promise.resolve(p) === p &&
                    p.then() !== p &&
                    typeof Promise === 'function';
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn self_resolution_rejects_with_type_error() {
        let mut doc = DocumentState::with_title("");
        let mut session = DocumentScriptSession::new(&doc);

        session
            .evaluate(
                r#"
                let name = '';
                let p;

                p = Promise.resolve().then(function() {
                    return p;
                });

                p.then(undefined, function(error) {
                    name = error.name;
                });
            "#,
                &mut doc,
            )
            .unwrap();

        assert_eq!(
            session.evaluate("name;", &mut doc).unwrap(),
            Value::String("TypeError".into())
        );
    }

    #[test]
    fn reports_unsupported_resolution_and_construction() {
        for source in [
            "Promise.resolve({});",
            "new Promise(function() {});",
            "Promise.resolve(1).then(function() { return {}; });",
        ] {
            assert_eq!(
                evaluate(source).unwrap_err().kind,
                JsErrorKind::UnsupportedFeature,
                "{source}"
            );
        }
    }

    #[test]
    fn rejects_invalid_receivers() {
        for source in [
            "let resolve = Promise.resolve; resolve(1);",
            "Promise.prototype.then.call({}, function() {});",
        ] {
            assert!(
                matches!(
                    evaluate(source).unwrap_err().kind,
                    JsErrorKind::NativeException(_)
                ),
                "{source}"
            );
        }
    }
}
