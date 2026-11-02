//! Partial implementations of string methods.
//!
//! <https://tc39.es/ecma262/#sec-string.prototype.split>
//! <https://tc39.es/ecma262/#sec-touint32>
//!
//! Strings are currently stored as UTF-8 Rust strings rather than arbitrary
//! ECMAScript UTF-16 code units. RegExp and object separators are unsupported.

use super::{Interpreter, JsError, Value};

impl Interpreter {
    pub(super) fn string_split(
        &self,
        receiver: Option<Value>,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        let input = match receiver.unwrap_or(Value::Undefined) {
            Value::String(input) => input,

            Value::Undefined | Value::Null => {
                return Err(JsError::type_error(
                    "String.prototype.split receiver is null or undefined",
                ));
            }

            _ => {
                return Err(JsError::unsupported(
                    "String.prototype.split receiver coercion is not implemented",
                ));
            }
        };

        let separator = match arguments.first().unwrap_or(&Value::Undefined) {
            Value::Undefined => None,
            Value::String(separator) => Some(separator.as_str()),

            _ => {
                return Err(JsError::unsupported(
                    "String.prototype.split only supports string separators",
                ));
            }
        };

        let limit = match arguments.get(1).unwrap_or(&Value::Undefined) {
            Value::Undefined => u32::MAX,

            value => {
                let number = self.to_number(value).ok_or_else(|| {
                    JsError::unsupported(
                        "String.prototype.split limit object coercion is not implemented",
                    )
                })?;

                if !number.is_finite() || number == 0.0 {
                    0
                } else {
                    number.trunc().rem_euclid(4_294_967_296.0) as u32
                }
            }
        };

        if limit == 0 {
            return Ok(Value::array(Vec::new()));
        }

        let Some(separator) = separator else {
            return Ok(Value::array(vec![Value::String(input)]));
        };

        let parts: Vec<Value> = if separator.is_empty() {
            if input.chars().any(|character| (character as u32) > 0xFFFF) {
                return Err(JsError::unsupported(
                    "String.prototype.split requires UTF-16 surrogate support",
                ));
            }

            input
                .chars()
                .take(limit as usize)
                .map(|character| Value::String(character.to_string()))
                .collect()
        } else {
            input
                .split(separator)
                .take(limit as usize)
                .map(|part| Value::String(part.to_owned()))
                .collect()
        };

        Ok(Value::array(parts))
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::{JsErrorKind, Value, evaluate};

    #[test]
    fn splits_strings_and_preserves_empty_fields() {
        let result = evaluate(
            r#"
                let parts = '.one..two.'.split('.');

                parts.length === 5 &&
                    parts[0] === '' &&
                    parts[1] === 'one' &&
                    parts[2] === '' &&
                    parts[3] === 'two' &&
                    parts[4] === '';
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn handles_undefined_empty_and_multichar_separators() {
        let result = evaluate(
            r#"
                let a = 'abc'.split();
                let b = ''.split('');
                let c = 'a--b--'.split('--');
                let d = 'éλ'.split('');

                a.length === 1 &&
                    a[0] === 'abc' &&
                    b.length === 0 &&
                    c.length === 3 &&
                    c[0] === 'a' &&
                    c[1] === 'b' &&
                    c[2] === '' &&
                    d.length === 2 &&
                    d[0] === 'é' &&
                    d[1] === 'λ';
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn converts_the_limit_to_uint32() {
        let result = evaluate(
            r#"
                let a = 'a,b,c'.split(',', 2);
                let b = 'a,b'.split(',', 0);
                let c = 'a,b'.split(',', 4294967297);
                let d = 'a,b'.split(',', -1);
                let e = 'a,b'.split(',', 1);
                let f = 'a,b'.split(',', 0 / 0);
                let g = 'abc'.split(undefined, null);

                a.length === 2 &&
                    a[1] === 'b' &&
                    b.length === 0 &&
                    c.length === 1 &&
                    c[0] === 'a' &&
                    d.length === 2 &&
                    e.length === 1 &&
                    f.length === 0 &&
                    g.length === 0;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn supports_indexed_method_lookup_and_explicit_this() {
        let result = evaluate(
            r#"
                let method = 'x'["split"];
                let parts = method.call('a.b', '.');

                parts.length === 2 &&
                    parts[0] === 'a' &&
                    parts[1] === 'b';
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn signals_unimplemented_separator_limit_and_utf16_cases() {
        for source in [
            "'a,b'.split(/,/);",
            "'abc'.split({});",
            "'abc'.split(',', {});",
            "'😀'.split('');",
        ] {
            assert_eq!(
                evaluate(source).unwrap_err().kind,
                JsErrorKind::UnsupportedFeature,
                "{source}",
            );
        }

        assert_eq!(
            evaluate("'😀'.split('', 0).length;").unwrap(),
            Value::Number(0.0)
        );
    }

    #[test]
    fn executes_the_expose_split_statement_from_testharness() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        let fragment = source
            .lines()
            .map(str::trim)
            .find(|line| *line == "var components = name.split(\".\");")
            .expect("WPT expose statement must exist");

        let script = format!(
            "let name = 'assert.example'; \
            {fragment} \
            components.length === 2 && \
                components[0] === 'assert' && \
                components[1] === 'example';"
        );

        assert_eq!(evaluate(&script).unwrap(), Value::Boolean(true));
    }
}
