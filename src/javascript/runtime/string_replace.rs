//! Partial String.prototype.replace and built-in RegExp replacement.
//!
//! <https://tc39.es/ecma262/#sec-string.prototype.replace>
//! <https://tc39.es/ecma262/#sec-regexp.prototype-@@replace>
//!
//! Object coercion, replacement substitutions and unpaired UTF-16 are incomplete.

use super::{Interpreter, JsError, Value};
use crate::javascript::regexp::matcher::Match;

struct ReplacementMatch {
    span: Match,
    groups: Vec<Option<Match>>,
    named: bool,
}

enum Replacement {
    Text(String),
    Callable(Value),
}

fn utf16_string(units: &[u16]) -> Result<String, JsError> {
    String::from_utf16(units).map_err(|_| {
        JsError::unsupported("String.replace requires unpaired UTF-16 surrogate support")
    })
}

impl Interpreter {
    pub(super) fn string_replace(
        &mut self,
        receiver: Option<Value>,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        let input = match receiver.unwrap_or(Value::Undefined) {
            Value::String(input) => input,

            Value::Null | Value::Undefined => {
                return Err(JsError::type_error(
                    "String.prototype.replace receiver is null or undefined",
                ));
            }

            _ => {
                return Err(JsError::unsupported(
                    "String.prototype.replace receiver coercion is not implemented",
                ));
            }
        };

        let search = arguments.first().cloned().unwrap_or(Value::Undefined);
        let replace = arguments.get(1).cloned().unwrap_or(Value::Undefined);

        let replacement = match replace {
            Value::Function(_) | Value::NativeFunction(_) => Replacement::Callable(replace),

            Value::ArrayMethod { .. }
            | Value::DocumentMethod(_)
            | Value::ElementMethod(_)
            | Value::TestContextMethod(_)
            | Value::RegExpTest(_)
            | Value::RegExpExec(_)
            | Value::ObjectConstructor
            | Value::ElementConstructor(_) => {
                return Err(JsError::unsupported(
                    "String.replace callable replacement type is not implemented",
                ));
            }

            _ => match Self::string_constructor(vec![replace])? {
                Value::String(text) => Replacement::Text(text),
                _ => unreachable!(),
            },
        };

        let input_units: Vec<u16> = input.encode_utf16().collect();
        let mut matches = Vec::new();

        match search {
            Value::RegExp(regexp) => {
                let global = regexp.inner.borrow().regexp.is_global();

                if global {
                    regexp.inner.borrow_mut().last_index = Value::Number(0.0);
                }

                loop {
                    let Some(found) = Self::run_regexp(&regexp, &input_units)? else {
                        break;
                    };

                    let span = found.captures.span;

                    matches.push(ReplacementMatch {
                        span,
                        groups: found.captures.groups,
                        named: found.named_groups.is_some(),
                    });

                    if !global {
                        break;
                    }

                    if matches.len() > 65_536 {
                        return Err(JsError::unsupported("String.replace match limit exceeded"));
                    }

                    if span.start == span.end {
                        let next = span
                            .end
                            .checked_add(1)
                            .ok_or_else(|| JsError::unsupported("String.replace index overflow"))?;

                        regexp.inner.borrow_mut().last_index = Value::Number(next as f64);
                    }
                }
            }

            value => {
                let Value::String(search) = Self::string_constructor(vec![value])? else {
                    unreachable!();
                };

                let needle: Vec<u16> = search.encode_utf16().collect();

                let start = if needle.is_empty() {
                    Some(0)
                } else if needle.len() <= input_units.len() {
                    input_units
                        .windows(needle.len())
                        .position(|window| window == needle)
                } else {
                    None
                };

                if let Some(start) = start {
                    let span = Match {
                        start,
                        end: start + needle.len(),
                    };

                    matches.push(ReplacementMatch {
                        span,
                        groups: vec![Some(span)],
                        named: false,
                    });
                }
            }
        }

        let mut output = Vec::new();
        let mut consumed = 0;

        for matched in matches {
            output.extend_from_slice(&input_units[consumed..matched.span.start]);

            let text = match &replacement {
                Replacement::Text(text) => {
                    if text.contains('$') {
                        return Err(JsError::unsupported(
                            "String.replace substitution patterns are not implemented",
                        ));
                    }

                    text.clone()
                }

                Replacement::Callable(function) => {
                    if matched.named {
                        return Err(JsError::unsupported(
                            "String.replace named capture callback is not implemented",
                        ));
                    }

                    if let Value::Function(function) = function {
                        if !function.is_bound()
                            && super::references::referenced_names(function.definition())
                                .contains("this")
                        {
                            return Err(JsError::unsupported(
                                "String.replace callback this binding is not implemented",
                            ));
                        }
                    }

                    let mut values = Vec::with_capacity(matched.groups.len() + 2);

                    for group in &matched.groups {
                        values.push(match group {
                            Some(span) => {
                                Value::String(utf16_string(&input_units[span.start..span.end])?)
                            }

                            None => Value::Undefined,
                        });
                    }

                    values.push(Value::Number(matched.span.start as f64));
                    values.push(Value::String(input.clone()));

                    let result = self.dispatch_call(function.clone(), None, values)?;

                    let Value::String(text) = Self::string_constructor(vec![result])? else {
                        unreachable!();
                    };

                    text
                }
            };

            output.extend(text.encode_utf16());
            consumed = matched.span.end;
        }

        output.extend_from_slice(&input_units[consumed..]);

        Ok(Value::String(utf16_string(&output)?))
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::{JsErrorKind, Value, evaluate};

    #[test]
    fn replaces_first_string_and_calls_callback() {
        let result = evaluate(
            r#"
                let args = '';

                let result = 'aba'.replace(
                    'b',
                    function(match, index, input) {
                        args = match + index + input;
                        return 'X';
                    }
                );

                result === 'aXa' &&
                    args === 'b1aba' &&
                    'aaa'.replace('a', 'z') === 'zaa' &&
                    'abc'.replace('q', 'z') === 'abc';
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn replaces_regexp_with_captures_and_resets_global_last_index() {
        let result = evaluate(
            r#"
                let re = /(a)(b)?/g;
                re.lastIndex = 4;

                let args = '';

                let result = 'ab a'.replace(
                    re,
                    function(match, a, b, index) {
                        args += a + (b === undefined ? '-' : b) + index;
                        return 'X';
                    }
                );

                result === 'X X' &&
                    args === 'ab0a-3' &&
                    re.lastIndex === 0;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn handles_empty_matches_and_no_match_without_calling_callback() {
        let result = evaluate(
            r#"
                let count = 0;

                let a = 'ab'.replace(/(?:)/g, function() {
                    count += 1;
                    return '-';
                });

                let b = 'hello'.replace(/z/g, function() {
                    throw 'unexpected';
                });

                a === '-a-b-' &&
                    count === 3 &&
                    b === 'hello';
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn executes_harness_surrogate_sanitizer_pattern_without_matching_valid_utf16() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        let fragment =
            r#"/([\ud800-\udbff]+)(?![\udc00-\udfff])|(^|[^\ud800-\udbff])([\udc00-\udfff]+)/g"#;

        assert!(source.contains(fragment));

        let script = format!(
            r#"
                let s = 'hello 😀';

                s.replace({fragment}, function() {{
                    throw 'unexpected';
                }}) === s;
            "#
        );

        assert_eq!(evaluate(&script).unwrap(), Value::Boolean(true));
    }

    #[test]
    fn reports_utf16_callback_index_and_propagates_callback_errors() {
        let result = evaluate(
            r#"
                let position = -1;

                let result = '😀a'.replace(
                    /a/,
                    function(match, index) {
                        position = index;
                        return 'z';
                    }
                );

                let caught = '';

                try {
                    'a'.replace(/a/, function() {
                        throw 'boom';
                    });
                } catch (error) {
                    caught = error;
                }

                result === '😀z' &&
                    position === 2 &&
                    caught === 'boom';
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn reports_unsupported_substitutions_and_unpaired_utf16() {
        for source in ["'a'.replace('a', '$&');", "'😀'.replace(/./, 'x');"] {
            assert_eq!(
                evaluate(source).unwrap_err().kind,
                JsErrorKind::UnsupportedFeature,
                "{source}"
            );
        }
    }
}
