//! User-function invocation for the interpreter's current subset.
//!
//! ECMAScript function calls:
//! <https://tc39.es/ecma262/#sec-ecmascript-function-objects-call-thisargument-argumentslist>
//!
//! Binding and argument handling remain incomplete.

use super::{Completion, FunctionValue, Interpreter, JsError, Statement, UserFunction, Value};
use crate::FunctionKind;

fn function_local_names(statements: &[Statement]) -> Vec<String> {
    let mut names = Vec::new();

    for statement in statements {
        match statement {
            Statement::Declaration { name, .. } | Statement::FunctionDeclaration { name, .. } => {
                if !names.contains(name) {
                    names.push(name.clone());
                }
            }

            Statement::Declarations(declarations) => {
                for (name, _) in declarations {
                    if !names.contains(name) {
                        names.push(name.clone());
                    }
                }
            }

            Statement::If {
                then_body,
                else_body,
                ..
            } => {
                for name in function_local_names(then_body)
                    .into_iter()
                    .chain(function_local_names(else_body))
                {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
            }

            Statement::While { body, .. } => {
                for name in function_local_names(body) {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
            }

            Statement::For {
                initializer, body, ..
            } => {
                if let Some(initializer) = initializer {
                    for name in function_local_names(std::slice::from_ref(initializer.as_ref())) {
                        if !names.contains(&name) {
                            names.push(name);
                        }
                    }
                }

                for name in function_local_names(body) {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
            }

            Statement::ForOf { binding, body, .. } | Statement::ForIn { binding, body, .. } => {
                if !names.contains(binding) {
                    names.push(binding.clone());
                }

                for name in function_local_names(body) {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
            }

            Statement::ForOfArray { bindings, body, .. } => {
                for name in bindings.iter().cloned().chain(function_local_names(body)) {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
            }

            Statement::ForInExisting { body, .. } => {
                for name in function_local_names(body) {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
            }

            Statement::Try {
                body,
                catch,
                finally,
            } => {
                let blocks = std::iter::once(body)
                    .chain(catch.iter().map(|clause| &clause.body))
                    .chain(finally.iter());

                for block in blocks {
                    for name in function_local_names(block) {
                        if !names.contains(&name) {
                            names.push(name);
                        }
                    }
                }
            }

            Statement::Switch { cases, .. } => {
                for case in cases {
                    for name in function_local_names(&case.body) {
                        if !names.contains(&name) {
                            names.push(name);
                        }
                    }
                }
            }

            Statement::Expression(_)
            | Statement::Return(_)
            | Statement::Throw(_)
            | Statement::Break
            | Statement::Continue => {}
        }
    }

    names
}

impl Interpreter {
    fn call_function(
        &mut self,
        function: &UserFunction,
        frame_index: usize,
    ) -> Result<Value, JsError> {
        let body_locals = function_local_names(&function.body);

        if function.kind == FunctionKind::Ordinary
            && !function
                .parameters
                .iter()
                .any(|parameter| parameter.name == "arguments")
            && body_locals.iter().any(|name| name == "arguments")
        {
            return Err(JsError::unsupported(
                "function body declarations named arguments are not implemented",
            ));
        }

        let mut local_names = Vec::new();

        for parameter in &function.parameters {
            if local_names.contains(&parameter.name) {
                continue;
            }

            let previous = self
                .variables
                .shadow(parameter.name.clone(), Value::Undefined);

            self.active_call_frame_mut(frame_index)
                .replaced_bindings
                .push(previous);
            local_names.push(parameter.name.clone());
        }

        if function.kind == FunctionKind::Ordinary
            && !local_names.iter().any(|name| name == "arguments")
        {
            let potentially_mapped = if function
                .parameters
                .iter()
                .all(|parameter| !parameter.is_rest && parameter.default.is_none())
            {
                function.parameters.len()
            } else {
                0
            };

            let arguments = self.active_call_frame(frame_index).arguments.clone();

            let previous = self.variables.shadow(
                "arguments".to_owned(),
                Value::arguments(arguments.clone(), potentially_mapped),
            );

            self.active_call_frame_mut(frame_index)
                .replaced_bindings
                .push(previous);
            local_names.push("arguments".to_owned());
        }

        self.active_call_frame_mut(frame_index).local_names = Some(local_names.clone());

        let result = (|| -> Result<Completion, JsError> {
            for (index, parameter) in function.parameters.iter().enumerate() {
                let value = if parameter.is_rest {
                    let arguments = &self.active_call_frame(frame_index).arguments;

                    Value::array(arguments.iter().skip(index).cloned().collect())
                } else {
                    let argument = self
                        .active_call_frame(frame_index)
                        .arguments
                        .get(index)
                        .cloned()
                        .unwrap_or(Value::Undefined);

                    match (argument, &parameter.default) {
                        (Value::Undefined, Some(default)) => {
                            self.evaluate_expression_with_handoff(
                                default,
                                |interpreter, result| {
                                    interpreter.assign_function_parameter(&parameter.name, result?)
                                },
                            )?;
                            continue;
                        }

                        (value, _) => value,
                    }
                };

                self.assign_function_parameter(&parameter.name, value)?;
            }

            for name in body_locals {
                if local_names.contains(&name) {
                    continue;
                }

                let previous = self.variables.shadow(name.clone(), Value::Undefined);

                self.active_call_frame_mut(frame_index)
                    .replaced_bindings
                    .push(previous);
                local_names.push(name);
            }

            self.active_call_frame_mut(frame_index).local_names = Some(local_names);
            self.execute(&function.body)
        })();

        result.map(|completion| match completion {
            Completion::Normal(_) => Value::Undefined,
            completion => completion.into_value(),
        })
    }

    fn assign_function_parameter(&mut self, name: &str, value: Value) -> Result<(), JsError> {
        self.variables
            .assign_existing(name, value)
            .map_err(|_| JsError::runtime("internal error: parameter binding is missing"))
    }

    pub(super) fn create_function(&self, function: &UserFunction) -> Value {
        let referenced = super::identifier_analysis::referenced_names(function);

        let mut captures = self
            .current_active_function()
            .map(|frame| frame.callee.captures().clone())
            .unwrap_or_default();

        captures.retain(|name, _| referenced.contains(name));

        if let Some(names) = self
            .current_active_function()
            .and_then(|frame| frame.local_names.as_ref())
        {
            captures.extend(self.variables.capture_names(names, &referenced));
        }

        let function_depth = self.function_depth();

        let catch_names: Vec<_> = self
            .catch_bindings
            .iter()
            .filter(|(depth, _)| *depth == function_depth)
            .map(|(_, name)| name.clone())
            .collect();

        captures.extend(self.variables.capture_names(&catch_names, &referenced));

        let prototype = (function.kind == FunctionKind::Ordinary).then(|| {
            self.host_roots.create_ordinary_object(
                [],
                super::ObjectPrototype::Object(self.object_prototype.clone()),
            )
        });
        Value::function(&self.host_roots, function.clone(), prototype, captures)
    }

    pub(super) fn call_function_value_with_receiver(
        &mut self,
        function: &FunctionValue,
        arguments: Vec<Value>,
        receiver: Option<Value>,
    ) -> Result<Value, JsError> {
        if function.definition().kind == FunctionKind::Async {
            return Err(JsError::unsupported(
                "async function invocation is not implemented",
            ));
        }

        let previous = self.variables.install_captures(function.captures());
        let frame_index = self.execution_frames.len();

        self.execution_frames
            .push(super::ExecutionFrame::ActiveCall(super::ActiveCallFrame {
                callee: function.clone(),
                arguments,
                replaced_bindings: previous,
                local_names: None,
                return_value: None,
            }));

        if let Some(receiver) =
            receiver.filter(|_| function.definition().kind == FunctionKind::Ordinary)
        {
            let previous = self.variables.shadow("this".to_owned(), receiver);

            self.active_call_frame_mut(frame_index)
                .replaced_bindings
                .push(previous);
        }

        let result = self
            .call_function(function.definition(), frame_index)
            .map(Completion::Return);

        // Keep both the completion and unrestored bindings visible to root traversal.
        let (result, ()) = self
            .with_pending_completion(result, |interpreter| {
                // The completion frame now owns the result during restoration.
                interpreter.active_call_frame_mut(frame_index).return_value = None;
                interpreter.restore_frame_bindings(frame_index);
                let value = match &interpreter.execution_frames[frame_index + 1] {
                    super::ExecutionFrame::PendingCompletion(Ok(Completion::Return(value))) => {
                        Some(value.clone())
                    }
                    super::ExecutionFrame::PendingCompletion(Err(_)) => None,
                    _ => unreachable!("function results use return completion"),
                };
                if let Some(value) = value {
                    interpreter.retain_call_result(&value);
                }
                Ok(())
            })
            .expect("restoring function bindings cannot fail");

        match self.execution_frames.pop() {
            Some(super::ExecutionFrame::ActiveCall(_)) => {}
            _ => unreachable!("active call frame must exist"),
        }

        result.map(|completion| match completion {
            Completion::Return(value) => value,
            _ => unreachable!("function results use return completion"),
        })
    }
}

#[cfg(test)]
mod tests;
