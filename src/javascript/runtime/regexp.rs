//! RegExp operations exposed to the JavaScript interpreter.
//!
//! <https://tc39.es/ecma262/#sec-regexpbuiltinexec>

use super::{Interpreter, JsError, RegExpValue, Value};
use crate::javascript::regexp::{matcher::Match, object::ExecResult};

impl Interpreter {
    pub(super) fn run_regexp(
        regexp: &RegExpValue,
        input: &[u16],
    ) -> Result<Option<ExecResult>, JsError> {
        let mut state = regexp.inner.borrow_mut();

        let start = Self::regexp_last_index(&state.last_index)?;

        state.regexp.set_last_index(start);

        let result = state
            .regexp
            .exec_result(input)
            .map_err(|error| JsError::runtime(format!("RegExp execution failed: {error:?}")))?;

        if state.regexp.uses_last_index() {
            state.last_index = Value::Number(state.regexp.last_index() as f64);
        }

        Ok(result)
    }

    pub(super) fn regexp_exec_value(
        input: String,
        units: &[u16],
        result: ExecResult,
        has_indices: bool,
    ) -> Result<Value, JsError> {
        let mut captures = Vec::with_capacity(result.captures.groups.len());

        for span in &result.captures.groups {
            let value = match span {
                None => Value::Undefined,

                Some(span) => {
                    let text = units
                        .get(span.start..span.end)
                        .ok_or_else(|| JsError::runtime("invalid RegExp capture range"))?;

                    let text = String::from_utf16(text).map_err(|_| {
                        JsError::runtime(
                            "RegExp capture contains an unpaired UTF-16 surrogate; \
                            JavaScript UTF-16 strings are not supported yet",
                        )
                    })?;

                    Value::String(text)
                }
            };

            captures.push(value);
        }

        let groups = result
            .named_groups
            .as_ref()
            .map_or(Value::Undefined, |named| {
                Value::object(
                    named
                        .iter()
                        .map(|group| (group.name.clone(), captures[group.index].clone())),
                )
            });

        let array = Value::array(captures);

        if let Value::Array(storage) = &array {
            let mut array = storage.borrow_mut();

            array.properties.set(
                "index".into(),
                Value::Number(result.captures.span.start as f64),
            );

            array.properties.set("input".into(), Value::String(input));
            array.properties.set("groups".into(), groups);

            if has_indices {
                let indices: Vec<Value> = result
                    .captures
                    .groups
                    .iter()
                    .map(|span| match span {
                        None => Value::Undefined,

                        Some(Match { start, end }) => Value::array(vec![
                            Value::Number(*start as f64),
                            Value::Number(*end as f64),
                        ]),
                    })
                    .collect();

                let index_groups = result
                    .named_groups
                    .as_ref()
                    .map_or(Value::Undefined, |named| {
                        Value::object(
                            named
                                .iter()
                                .map(|group| (group.name.clone(), indices[group.index].clone())),
                        )
                    });

                let indices = Value::array(indices);

                if let Value::Array(storage) = &indices {
                    storage
                        .borrow_mut()
                        .properties
                        .set("groups".into(), index_groups);
                }

                array.properties.set("indices".into(), indices);
            }
        }

        Ok(array)
    }
}
