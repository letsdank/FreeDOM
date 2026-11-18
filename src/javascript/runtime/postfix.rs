//! Evaluation of member access, indexing, and calls in postfix chains.
//!
//! <https://tc39.es/ecma262/#sec-left-hand-side-expressions>
//!
//! This module implements only the interpreter's supported expression subset.

use super::reference::Reference;
use super::{Expression, Interpreter, JsError, PendingCall, PostfixStep, Value};

enum PostfixResult {
    Value(Value),
    Reference(Reference),
}

impl PostfixResult {
    fn into_value(self, interpreter: &mut Interpreter) -> Result<Value, JsError> {
        match self {
            Self::Value(value) => Ok(value),
            Self::Reference(reference) => interpreter.read_reference(&reference),
        }
    }
}

impl Interpreter {
    pub(super) fn evaluate_postfix_value(
        &mut self,
        base: &Expression,
        steps: &[PostfixStep],
    ) -> Result<Value, JsError> {
        let result = self.evaluate_postfix_chain(base, steps)?;
        result.into_value(self)
    }

    pub(super) fn evaluate_postfix_reference(
        &mut self,
        base: &Expression,
        steps: &[PostfixStep],
    ) -> Result<Reference, JsError> {
        match self.evaluate_postfix_chain(base, steps)? {
            PostfixResult::Reference(reference) => Ok(reference),

            PostfixResult::Value(_) => {
                unreachable!("a property step produces a reference")
            }
        }
    }

    fn evaluate_postfix_chain(
        &mut self,
        base: &Expression,
        steps: &[PostfixStep],
    ) -> Result<PostfixResult, JsError> {
        let mut result = PostfixResult::Value(self.evaluate_expression(base)?);

        for step in steps {
            self.consume_step()?;

            result = match step {
                PostfixStep::Member(property) => {
                    let object = result.into_value(self)?;

                    PostfixResult::Reference(Reference::Member {
                        object,
                        property: property.clone(),
                    })
                }

                PostfixStep::Index(index) => {
                    let object = result.into_value(self)?;

                    self.with_temporary_value(object, |interpreter, object_index| {
                        let index = interpreter.evaluate_expression(index)?;
                        let object = interpreter.temporary_values[object_index].clone();

                        Ok(PostfixResult::Reference(Reference::Index { object, index }))
                    })?
                }

                PostfixStep::Call(arguments) => {
                    let receiver = match &result {
                        PostfixResult::Reference(reference) => reference.receiver(),
                        PostfixResult::Value(_) => None,
                    };

                    let callee = result.into_value(self)?;

                    PostfixResult::Value(self.evaluate_postfix_call(callee, receiver, arguments)?)
                }
            };
        }

        Ok(result)
    }

    fn evaluate_postfix_call(
        &mut self,
        callee: Value,
        receiver: Option<Value>,
        expressions: &[Expression],
    ) -> Result<Value, JsError> {
        let index = self.pending_calls.len();

        self.pending_calls.push(PendingCall {
            callee,
            receiver,
            arguments: Vec::with_capacity(expressions.len()),
        });

        let result = (|| {
            for expression in expressions {
                let argument = self.evaluate_expression(expression)?;

                self.pending_calls[index].arguments.push(argument);
            }

            let call = &self.pending_calls[index];
            let callee = call.callee.clone();
            let receiver = call.receiver.clone();
            let arguments = call.arguments.clone();

            self.dispatch_call(callee, receiver, arguments)
        })();

        self.pending_calls.truncate(index);

        result
    }
}
