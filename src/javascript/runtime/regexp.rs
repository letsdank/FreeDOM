//! RegExp operations exposed to the JavaScript interpreter.
//!
//! <https://tc39.es/ecma262/#sec-regexpbuiltinexec>

use super::{Expression, Interpreter, JsError, Value};
use crate::javascript::regexp::{matcher::Match, object::ExecResult};
use crate::javascript::{RegExpState, RegExpValue};

impl Interpreter {
    pub(super) fn construct_regexp(&mut self, arguments: &[Expression]) -> Result<Value, JsError> {
        let mut values = Vec::with_capacity(arguments.len());

        for argument in arguments {
            values.push(self.evaluate_expression(argument)?);
        }

        let pattern = match values.first() {
            None | Some(Value::Undefined) => String::new(),

            Some(Value::RegExp(_)) => {
                return Err(JsError::runtime(
                    "RegExp construction from another RegExp is not implemented yet",
                ));
            }

            Some(value) => value.to_js_string(),
        };

        let flags = match values.get(1) {
            None | Some(Value::Undefined) => String::new(),
            Some(value) => value.to_js_string(),
        };

        Self::create_regexp(&pattern, &flags)
    }

    pub(super) fn create_regexp(pattern: &str, flags: &str) -> Result<Value, JsError> {
        let regexp = crate::javascript::regexp::object::RegExp::compile(pattern, flags)
            .map_err(|error| JsError::runtime(format!("RegExp compilation failed: {error:?}")))?;

        Ok(Value::RegExp(RegExpValue {
            inner: std::rc::Rc::new(std::cell::RefCell::new(RegExpState {
                regexp,
                last_index: Value::Number(0.0),
            })),
        }))
    }

    // Object ToPrimitive is not implemented here yet.
    // https://tc39.es/ecma262/#sec-tolength
    fn regexp_last_index(value: &Value) -> Result<usize, JsError> {
        let number = match value {
            Value::Number(number) => *number,

            Value::Boolean(value) => {
                if *value {
                    1.0
                } else {
                    0.0
                }
            }

            Value::Null => 0.0,

            Value::Undefined => f64::NAN,

            Value::String(value) => {
                return Ok(crate::javascript::regexp::last_index::from_string(value));
            }

            _ => {
                return Err(JsError::runtime(
                    "RegExp lastIndex conversion is not implemented for this value",
                ));
            }
        };

        Ok(crate::javascript::regexp::last_index::from_number(number))
    }

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

#[cfg(test)]
mod tests {
    use super::{Interpreter, Value};
    use crate::javascript::evaluate;

    #[test]
    fn converts_supported_last_index_values() {
        assert_eq!(Interpreter::regexp_last_index(&Value::Number(1.9),), Ok(1));
        assert_eq!(Interpreter::regexp_last_index(&Value::Undefined,), Ok(0));

        assert_eq!(
            Interpreter::regexp_last_index(&Value::String("0x2".into())),
            Ok(2)
        );

        assert!(Interpreter::regexp_last_index(&Value::array(Vec::new())).is_err());
    }

    #[test]
    fn constructor_and_exec_share_the_regexp_binding() {
        evaluate(
            r#"
                const re = new RegExp("(?<x>a)", "dg");
                re.lastIndex = "1";

                const result = re.exec("ba");

                assert_equals(result[0], "a");
                assert_equals(result.groups.x, "a");
                assert_equals(result.indices[0][0], 1);
                assert_equals(re.lastIndex, 2);
            "#,
        )
        .unwrap();
    }
}
