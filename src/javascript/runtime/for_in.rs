//! Partial `for...in` evaluation for ordinary objects.
//!
//! <https://tc39.es/ecma262/#sec-for-in-and-for-of-statements>
//!
//! Property descriptors, dynamic deletion, and full lexical environments
//! are not implemented.

use super::{Completion, Expression, Interpreter, JsError, Statement, Value};
use crate::javascript::value::ObjectState;

#[derive(Clone, Copy)]
pub(super) enum ForInBindingMode {
    Lexical,
    Var,
    Existing,
}

impl Interpreter {
    pub(super) fn execute_for_in(
        &mut self,
        binding: &str,
        object: &Expression,
        body: &[Statement],
        mode: ForInBindingMode,
    ) -> Result<Completion, JsError> {
        if matches!(mode, ForInBindingMode::Var) {
            self.variables.insert_if_absent(binding, Value::Undefined);
        }

        let target = self.evaluate_expression(object)?;

        let keys = match target {
            Value::Null | Value::Undefined => Vec::new(),

            Value::Object(object) => ObjectState::for_in_keys(&object).ok_or_else(|| {
                JsError::unsupported("for-in encountered an unimplemented prototype")
            })?,

            _ => {
                return Err(JsError::unsupported(
                    "for-in is not implemented for this value",
                ));
            }
        };

        let previous = if matches!(mode, ForInBindingMode::Lexical) {
            Some(self.variables.insert(binding.to_owned(), Value::Undefined))
        } else {
            None
        };

        let result = (|| -> Result<Completion, JsError> {
            for key in keys {
                self.variables
                    .insert(binding.to_owned(), Value::String(key));

                match self.execute(body)? {
                    Completion::Normal(_) | Completion::Continue => {}

                    Completion::Break => {
                        return Ok(Completion::Normal(Value::Undefined));
                    }

                    Completion::Return(value) => {
                        return Ok(Completion::Return(value));
                    }
                }
            }

            Ok(Completion::Normal(Value::Undefined))
        })();

        if let Some(previous) = previous {
            match previous {
                Some(value) => {
                    self.variables.insert(binding.to_owned(), value);
                }

                None => {
                    self.variables.remove(binding);
                }
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::{JsErrorKind, Value, evaluate};

    #[test]
    fn enumerates_own_and_inherited_keys_in_order_without_duplicates() {
        let result = evaluate(
            "let p = Object.create(null); \
            p['9'] = 1; \
            p['3'] = 1; \
            p.a = 1; \
            p.b = 1; \
            let o = Object.create(p); \
            o['2'] = 1; \
            o['1'] = 1; \
            o.a = 2; \
            let out = ''; \
            for (let k in o) { out += k + ','; } \
            out;",
        )
        .unwrap();

        assert_eq!(result, Value::String("1,2,a,3,9,b,".to_owned()));
    }

    #[test]
    fn lexical_and_var_bindings_have_distinct_lifetimes() {
        let result = evaluate(
            "let o = Object.create(null); \
            o.a = 1; \
            o.b = 2; \
            let key = 'before'; \
            for (let key in o) {} \
            let saved = key; \
            var last = 'first'; \
            for (var last in o) {} \
            saved === 'before' && key === 'before' && last === 'b';",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn handles_continue_break_return_and_single_target_evaluation() {
        let result = evaluate(
            "let o = Object.create(null); \
            o.a = 1; \
            o.b = 2; \
            let calls = 0; \
            function get() { calls++; return o; } \
            function f() { \
                for (let k in get()) { \
                    if (k === 'a') continue; \
                    return k; \
                } \
                return 'missing'; \
            } \
            f() === 'b' && calls === 1;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));

        let result = evaluate(
            "let o = Object.create(null); \
            o.a = 1; \
            o.b = 1; \
            let result = ''; \
            for (let k in o) { result += k; break; } \
            result;",
        )
        .unwrap();

        assert_eq!(result, Value::String("a".to_owned()));
    }

    #[test]
    fn restores_lexical_binding_after_error() {
        let result = evaluate(
            "let o = Object.create(null); \
            o.a = 1; \
            let k = 'outer'; \
            try { \
                for (let k in o) { missing; } \
            } catch (e) {} \
            k;",
        )
        .unwrap();

        assert_eq!(result, Value::String("outer".to_owned()));
    }

    #[test]
    fn unknown_prototypes_and_other_targets_are_explicitly_unsupported() {
        for source in ["for (let k in {}) {}", "for (let k in []) {}"] {
            assert_eq!(
                evaluate(source).unwrap_err().kind,
                JsErrorKind::UnsupportedFeature
            );
        }

        assert_eq!(
            evaluate("for (var k in null) {} k;").unwrap(),
            Value::Undefined
        );
    }

    #[test]
    fn existing_binding_retains_last_key_and_is_not_localized() {
        let result = evaluate(
            "let o = Object.create(null); \
            o.a = 1; \
            o.b = 2; \
            let key = 'initial'; \
            function f() { \
                for (key in o) { let inside = 1; } \
            } \
            f(); \
            key;",
        )
        .unwrap();

        assert_eq!(result, Value::String("b".to_owned()));
    }

    #[test]
    fn zero_iterations_do_not_create_or_change_existing_binding() {
        let result = evaluate(
            "let key = 'unchanged'; \
            for (key in Object.create(null)) {} \
            key;",
        )
        .unwrap();

        assert_eq!(result, Value::String("unchanged".to_owned()));

        let result = evaluate(
            "for (not_declared in null) {} \
            typeof not_declared;",
        )
        .unwrap();

        assert_eq!(result, Value::String("undefined".to_owned()));
    }

    #[test]
    fn rhs_error_does_not_write_the_binding() {
        let result = evaluate(
            "let key = 'unchanged'; \
            try { for (key in missing) {} } catch (e) {} \
            key;",
        )
        .unwrap();

        assert_eq!(result, Value::String("unchanged".to_owned()));
    }

    #[test]
    fn evaluates_rhs_once_and_preserves_key_after_break() {
        let result = evaluate(
            "let o = Object.create(null); \
            o.a = 1; \
            o.b = 2; \
            let count = 0; \
            function get() { count++; return o; } \
            let key = ''; \
            for (key in get()) { break; } \
            count === 1 && key === 'a';",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }
}
