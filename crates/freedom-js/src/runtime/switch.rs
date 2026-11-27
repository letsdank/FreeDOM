//! Partial evaluation of JavaScript switch statements.
//!
//! <https://tc39.es/ecma262/#sec-switch-statement>

use super::{Completion, Expression, Interpreter, JsError, Value};
use crate::ast::SwitchCase;

impl Interpreter {
    pub(super) fn execute_switch(
        &mut self,
        discriminant: &Expression,
        cases: &[SwitchCase],
    ) -> Result<Completion, JsError> {
        let value = self.evaluate_expression(discriminant)?;

        self.with_frame_value(value, |interpreter, frame_index| {
            let mut selected = None;
            let mut default = None;

            for (index, case) in cases.iter().enumerate() {
                match &case.test {
                    Some(test) => {
                        let case_value = interpreter.evaluate_expression(test)?;

                        if interpreter
                            .strictly_equals(interpreter.frame_value(frame_index), &case_value)
                        {
                            selected = Some(index);
                            break;
                        }
                    }

                    None => {
                        default = Some(index);
                    }
                }
            }

            let Some(start) = selected.or(default) else {
                return Ok(Completion::Normal(Value::Undefined));
            };

            interpreter.set_frame_value(frame_index, Value::Undefined);

            for case in &cases[start..] {
                if case.body.is_empty() {
                    continue;
                }

                match interpreter.execute(&case.body)? {
                    Completion::Normal(value) => {
                        interpreter.set_frame_value(frame_index, value);
                    }

                    Completion::Break => {
                        return Ok(Completion::Normal(
                            interpreter.frame_value(frame_index).clone(),
                        ));
                    }

                    abrupt => return Ok(abrupt),
                }
            }

            Ok(Completion::Normal(
                interpreter.frame_value(frame_index).clone(),
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::{Value, evaluate};

    #[test]
    fn evaluates_cases_in_order_and_stops_after_match() {
        let result = evaluate(
            "let log = ''; \
            function f(x) { log += x; return x; } \
            switch (f(2)) { \
                case f(1): log += 'x'; break; \
                default: log += 'd'; \
                case f(2): log += 'm'; break; \
                case f(3): log += 'z'; \
            } \
            log;",
        )
        .unwrap();

        assert_eq!(result, Value::String("212m".to_owned()));
    }

    #[test]
    fn uses_default_only_after_checking_later_cases_and_falls_through() {
        let result = evaluate(
            "let out = ''; \
            switch (3) { \
                case 1: out += 'a'; break; \
                default: out += 'd'; \
                case 2: out += 'b'; \
                case 3: out += 'c'; \
                case 4: out += 'e'; \
            } \
            out;",
        )
        .unwrap();

        assert_eq!(result, Value::String("ce".to_owned()));

        let result = evaluate(
            "let out = ''; \
            switch (5) { \
                case 1: out += 'a'; break; \
                default: out += 'd'; \
                case 2: out += 'b'; \
            } \
            out;",
        )
        .unwrap();

        assert_eq!(result, Value::String("db".to_owned()));
    }

    #[test]
    fn uses_strict_equality_and_handles_no_match() {
        let result = evaluate(
            "let out = 'none'; \
            switch (1) { case '1': out = 'wrong'; } \
            out;",
        )
        .unwrap();

        assert_eq!(result, Value::String("none".to_owned()));
    }

    #[test]
    fn break_stays_in_switch_continue_reaches_loop() {
        let result = evaluate(
            "let i = 0; \
            let n = 0; \
            while (i < 3) { \
                i++; \
                switch (i) { \
                    case 1: continue; \
                    case 2: break; \
                    default: n += 10; \
                } \
                n++; \
            } \
            n;",
        )
        .unwrap();

        assert_eq!(result, Value::Number(12.0));
    }

    #[test]
    fn return_propagates_and_locals_are_restored() {
        let result = evaluate(
            "let x = 7; \
            function f() { \
                switch (1) { \
                    case 1: let x = 3; return x; \
                } \
            } \
            let result = f(); \
            result === 3 && x === 7;",
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }
}
