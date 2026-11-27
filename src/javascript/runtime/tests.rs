use super::{Completion, ExecutionFrame, Interpreter, RootVisitError, RuntimeRoot};
use crate::javascript::{JsError, JsErrorKind, Value, parse_program};
use std::collections::HashSet;
use std::rc::Rc;

#[test]
fn execution_guard_bounds_loops_and_microtasks() {
    let mut interpreter = Interpreter::new();

    // Keep the test fast; normal execution uses the default limit.
    interpreter.execution_limit = 256;

    for source in [
        "for (;;) {}",
        "function again() { queueMicrotask(again); } queueMicrotask(again);",
    ] {
        let program = parse_program(source).unwrap();

        let error = interpreter.execute_script(&program).err().unwrap();

        assert!(matches!(error.kind, JsErrorKind::ResourceLimit));
        assert!(error.message.contains("execution budget"));
        assert!(interpreter.execution_frames.is_empty());

        let next_program = parse_program("42;").unwrap();

        let result = interpreter
            .execute_script(&next_program)
            .unwrap()
            .into_value();

        assert_eq!(result, Value::Number(42.0));
    }
}

#[test]
fn execution_depth_limit_unwinds_and_resets() {
    let mut interpreter = Interpreter::new();

    for source in [
        "function direct() { direct(); } direct();",
        "function nested() { if (true) { nested(); } } nested();",
    ] {
        let program = parse_program(source).unwrap();

        let error = interpreter.execute_script(&program).err().unwrap();

        assert_eq!(error.kind, JsErrorKind::ResourceLimit);
        assert!(error.message.contains("execution depth"));

        assert_eq!(interpreter.execution_depth, 0);

        let next_program = parse_program("42;").unwrap();

        let result = interpreter
            .execute_script(&next_program)
            .unwrap()
            .into_value();

        assert_eq!(result, Value::Number(42.0));
    }
}

#[test]
fn pending_completions_restore_after_nested_errors() {
    let mut interpreter = Interpreter::new();
    let original = Err(JsError::thrown(Value::Number(7.0)));

    let (result, ()) = interpreter
        .with_pending_completion(original, |runtime| {
            assert_eq!(runtime.execution_frames.len(), 1);

            assert!(matches!(
                &runtime.execution_frames[0],
                ExecutionFrame::PendingCompletion(Err(JsError {
                    kind: JsErrorKind::Thrown(Value::Number(7.0)),
                    ..
                }))
            ));

            let nested =
                runtime.with_pending_completion(Ok(Completion::Normal(Value::Null)), |runtime| {
                    assert_eq!(runtime.execution_frames.len(), 2);
                    Err::<(), _>(JsError::resource_limit("test limit"))
                });

            assert!(nested.is_err());
            assert_eq!(runtime.execution_frames.len(), 1);

            Ok(())
        })
        .unwrap();

    assert!(matches!(
        result,
        Err(JsError {
            kind: JsErrorKind::Thrown(Value::Number(7.0)),
            ..
        })
    ));

    assert!(interpreter.execution_frames.is_empty());
}

#[test]
fn replacing_pending_completion_updates_exception_roots() {
    let mut interpreter = Interpreter::new();
    let Value::Object(original) = Value::object([]) else {
        unreachable!();
    };
    let Value::Object(replacement) = Value::object([]) else {
        unreachable!();
    };

    let (result, ()) = interpreter
        .with_pending_completion(
            Err(JsError::thrown(Value::Object(original.clone()))),
            |interpreter| {
                let check_roots = |interpreter: &Interpreter, old: bool, new: bool| {
                    let mut saw_old = false;
                    let mut saw_new = false;
                    interpreter
                        .visit_runtime_roots(&mut |root| {
                            if let RuntimeRoot::Value(Value::Object(object)) = root {
                                saw_old |= Rc::ptr_eq(object, &original);
                                saw_new |= Rc::ptr_eq(object, &replacement);
                            }
                        })
                        .unwrap();
                    assert_eq!((saw_old, saw_new), (old, new));
                };

                check_roots(interpreter, true, false);
                interpreter.replace_pending_completion(
                    0,
                    Err(JsError::thrown(Value::Object(replacement.clone()))),
                );
                check_roots(interpreter, false, true);
                Ok(())
            },
        )
        .unwrap();

    assert!(matches!(
        result,
        Err(JsError {
            kind: JsErrorKind::Thrown(Value::Object(object)),
            ..
        }) if Rc::ptr_eq(&object, &replacement)
    ));
    assert!(interpreter.execution_frames.is_empty());
}

#[test]
fn script_exception_keeps_identity_across_microtask_checkpoint() {
    let mut interpreter = Interpreter::new();

    let program = parse_program(
        "let value = { x: 1 }; \
            queueMicrotask(function() { value.x = 2; }); \
            throw value;",
    )
    .unwrap();

    let error = interpreter.execute_script(&program).err().unwrap();

    let JsErrorKind::Thrown(value) = error.kind else {
        panic!("expected thrown value");
    };

    assert_eq!(
        interpreter.member_value(value, "x").unwrap(),
        Value::Number(2.0)
    );

    assert!(interpreter.execution_frames.is_empty());
}

#[test]
fn active_call_frames_are_restored_after_nested_calls_and_errors() {
    let mut interpreter = Interpreter::new();

    let program = parse_program(
        r#"
            let x = 42;
            function inner(x) { return x + 1; }
            function outer(x = inner(4), ...rest) { return x + rest[0]; }
            outer(undefined, 3) === 8 && x === 42;
            "#,
    )
    .unwrap();

    let result = interpreter.execute_script(&program).unwrap().into_value();

    assert_eq!(result, Value::Boolean(true));
    assert!(interpreter.execution_frames.is_empty());

    let program = parse_program("function tail(x = missing) {} fail();").unwrap();

    assert!(interpreter.execute_script(&program).is_err());
    assert!(interpreter.execution_frames.is_empty());

    let program = parse_program("x;").unwrap();

    assert_eq!(
        interpreter.execute_script(&program).unwrap().into_value(),
        Value::Number(42.0)
    );
}

#[test]
fn binary_temporary_values_are_scoped_across_nested_calls_and_errors() {
    let mut interpreter = Interpreter::new();

    interpreter
        .execution_frames
        .push(ExecutionFrame::TemporaryValues(vec![Value::Number(77.0)]));

    let program = parse_program(
        r#"
            let trace = '';
            let left = {
                valueOf: function() {
                    trace = trace + 'l';
                    return 2;
                }
            };
            let right = {
                valueOf: function() {
                    trace = trace + 'r';
                    return 3;
                }
            };

            function nested() {
                trace = trace + 'n';
                return right;
            }

            left + nested() === 5 && trace === 'nlr';
            "#,
    )
    .unwrap();

    let result = interpreter.execute_script(&program).unwrap().into_value();

    assert_eq!(result, Value::Boolean(true));
    assert_eq!(interpreter.frame_values(0), &[Value::Number(77.0)]);
    assert_eq!(interpreter.execution_frames.len(), 1);

    for source in [
        "1 + missing;",
        "({valueOf: function() { throw 'error'; }}) + 1;",
    ] {
        let program = parse_program(source).unwrap();

        assert!(interpreter.execute_script(&program).is_err());
        assert_eq!(interpreter.frame_values(0), &[Value::Number(77.0)]);
        assert_eq!(interpreter.execution_frames.len(), 1);
    }
}

#[test]
fn assignment_references_survive_nested_calls_and_errors() {
    let mut interpreter = Interpreter::new();

    let program = parse_program(
        r#"
            let target = {a: 1};
            let original = target;
            let key = 'a';

            function mutate() {
                target = {a: 100};
                key = 'b';
                return 2;
            }

            original[key] += mutate();

            original.a === 3 && target.a === 100 && key === 'b';
            "#,
    )
    .unwrap();

    let result = interpreter.execute_script(&program).unwrap().into_value();

    assert_eq!(result, Value::Boolean(true));
    assert!(interpreter.execution_frames.is_empty());

    for source in [
        "original.a = missing;",
        "original.a += missing;",
        "original.a += ({ valueOf: function() { throw 'error'; } });",
    ] {
        let program = parse_program(source).unwrap();

        assert!(interpreter.execute_script(&program).is_err());
        assert!(interpreter.execution_frames.is_empty());
    }
}

#[test]
fn indexed_updates_restore_temporary_roots_after_calls_and_errors() {
    let mut interpreter = Interpreter::new();

    let program = parse_program(
        r#"
            let calls = 0;
            let target = {
                x: {
                    valueOf: function() {
                        calls = calls + 1;
                        return 4;
                    }
                }
            };
            let original = target;

            function choose() {
                calls = calls + 1;
                target = {x: 99};
                return 'x';
            }

            let before = target[choose()]++;

            before === 4 &&
                original.x === 5 &&
                target.x === 99 &&
                calls === 2;
            "#,
    )
    .unwrap();

    let result = interpreter.execute_script(&program).unwrap().into_value();

    assert_eq!(result, Value::Boolean(true));
    assert!(interpreter.execution_frames.is_empty());

    for source in [
        "original[missing]++;",
        "({x: {valueOf: function() { throw 'error'; }}}).x++;",
    ] {
        let program = parse_program(source).unwrap();

        assert!(interpreter.execute_script(&program).is_err());
        assert!(interpreter.execution_frames.is_empty());
    }
}

#[test]
fn pending_postfix_calls_are_scoped_nested_calls_and_errors() {
    let mut interpreter = Interpreter::new();

    let program = parse_program(
        r#"
            let trace = '';
            let target = {
                factor: 10,
                method: function(a, b) {
                    return this.factor + a.value + b;
                }
            };
            let original = target;

            function first() {
                trace = trace + 'a';
                return {value: 3};
            }

            function second() {
                trace = trace + 'b';
                target = {};
                return 2;
            }

            function fail() {
                throw 'error';
            }

            let result = target.method(first(), second());

            result === 15 && trace === 'ab' && target !== original;
            "#,
    )
    .unwrap();

    let result = interpreter.execute_script(&program).unwrap().into_value();

    assert_eq!(result, Value::Boolean(true));
    assert!(interpreter.execution_frames.is_empty());

    for source in [
        "original.method({}, missing);",
        "original.method({}, fail());",
        "original.factor();",
    ] {
        let program = parse_program(source).unwrap();

        assert!(interpreter.execute_script(&program).is_err());
        assert!(interpreter.execution_frames.is_empty());
    }
}

#[test]
fn literal_temporaries_are_scoped_and_preserve_holes() {
    let mut interpreter = Interpreter::new();

    interpreter
        .execution_frames
        .push(ExecutionFrame::TemporaryValues(vec![Value::Number(77.0)]));

    let program = parse_program(
        r#"
            let trace = '';

            function next() {
                trace = trace + 'n';
                return {value:2};
            }

            let array = [{value: 1},, next()];
            let object = {
                first: array[0],
                second: next(),
                first: array[2]
            };

            array.length === 3 &&
                !array.hasOwnProperty(1) &&
                array[0].value === 1 &&
                object.first.value === 2 &&
                object.second.value === 2 &&
                trace === 'nn';
            "#,
    )
    .unwrap();

    let result = interpreter.execute_script(&program).unwrap().into_value();

    assert_eq!(result, Value::Boolean(true));
    assert_eq!(interpreter.frame_values(0), &[Value::Number(77.0)]);
    assert_eq!(interpreter.execution_frames.len(), 1);

    for source in ["[{x: 1}, missing];", "({x: {a: 1}, y: missing});"] {
        let program = parse_program(source).unwrap();

        assert!(interpreter.execute_script(&program).is_err());
        assert_eq!(interpreter.frame_values(0), &[Value::Number(77.0)]);
        assert_eq!(interpreter.execution_frames.len(), 1);
    }
}

#[test]
fn constructor_arguments_are_scoped_across_nested_calls_and_errors() {
    let mut interpreter = Interpreter::new();

    let program = parse_program(
        r#"
            let trace = '';

            function A(a, b) {
                this.result = a.value + b.value;
            }

            let Constructor = A;

            function first() {
                trace = trace + 'a';
                return {value: 1};
            }

            function second() {
                trace = trace + 'b';
                Constructor = 4;
                return {value: 2};
            }

            let instance = new Constructor(first(), second());

            instance.result === 3 && trace === 'ab' && Constructor === 4;
            "#,
    )
    .unwrap();

    let result = interpreter.execute_script(&program).unwrap().into_value();

    assert_eq!(result, Value::Boolean(true));
    assert!(interpreter.execution_frames.is_empty());

    for source in ["new A(first(), missing);", "new Constructor(first());"] {
        let program = parse_program(source).unwrap();

        assert!(interpreter.execute_script(&program).is_err());
        assert!(interpreter.execution_frames.is_empty());
    }
}

#[test]
fn regexp_arguments_and_apply_length_conversion_restore_temporaries() {
    let mut interpreter = Interpreter::new();

    interpreter
        .execution_frames
        .push(ExecutionFrame::TemporaryValues(vec![Value::Number(77.0)]));

    let program = parse_program(
        r#"
            let trace = '';

            function pattern() {
                trace = trace + 'p';
                return 'a';
            }

            function flags() {
                trace = trace + 'f';
                return 'i';
            }

            function extra() {
                trace = trace + 'e';
                return 0;
            }

            let regexp = new RegExp(pattern(), flags(), extra());

            let list = {
                0: 7,
                length: {
                    valueOf: function() {
                        list = null;
                        return 1;
                    }
                }
            };

            function first(a) {
                return a;
            }

            let result = first.apply(null, list);

            regexp.test('A') &&
                trace === 'pfe' &&
                result === 7 &&
                list === null;
            "#,
    )
    .unwrap();

    let result = interpreter.execute_script(&program).unwrap().into_value();

    assert_eq!(result, Value::Boolean(true));
    assert_eq!(interpreter.frame_values(0), &[Value::Number(77.0)]);
    assert_eq!(interpreter.execution_frames.len(), 1);

    for source in [
        "new RegExp('a', missing);",
        "new RegExp('a', 'z');",
        "first.apply(null, {length: {valueOf: function() { throw 'error'; }}});",
    ] {
        let program = parse_program(source).unwrap();

        assert!(interpreter.execute_script(&program).is_err());
        assert_eq!(interpreter.frame_values(0), &[Value::Number(77.0)]);
        assert_eq!(interpreter.execution_frames.len(), 1);
    }
}

#[test]
fn finally_preserves_pending_values_and_unwinds_frames() {
    let mut interpreter = Interpreter::new();

    let program = parse_program(
        r#"
            let object = { value: 1 };

            function run() {
                try {
                    return object;
                } finally {
                    try {
                        object.value = 2;
                    } finally {
                        object.value = object.value + 1;
                    }
                }
            }

            run() === object && object.value === 3;
            "#,
    )
    .unwrap();

    let result = interpreter.execute_script(&program).unwrap().into_value();

    assert_eq!(result, Value::Boolean(true));
    assert!(interpreter.execution_frames.is_empty());

    let program = parse_program("try { throw 1; } finally { throw 2; }").unwrap();
    let error = interpreter.execute_script(&program).err().unwrap();

    assert!(matches!(
        error.kind,
        JsErrorKind::Thrown(Value::Number(2.0))
    ));

    assert!(interpreter.execution_frames.is_empty());
}

#[test]
fn shadowed_binding_frames_unwind_after_catch_and_loops() {
    let mut interpreter = Interpreter::new();

    let program = parse_program("let x = {value: 7};").unwrap();
    interpreter.execute_script(&program).unwrap();

    for source in [
        "try { throw 1; } catch (x) { throw 2; }",
        "for (let x in {a: 1}) { throw 2; }",
        "for (let x of [1]) { throw 2; }",
        "for (const [x, y] of [[1, 2]]) { throw 2; }",
    ] {
        let program = parse_program(source).unwrap();

        assert!(interpreter.execute_script(&program).is_err(), "{source}");
        assert!(interpreter.execution_frames.is_empty(), "{source}");

        let program = parse_program("x.value;").unwrap();

        assert_eq!(
            interpreter.execute_script(&program).unwrap().into_value(),
            Value::Number(7.0)
        );
    }
}

#[test]
fn pending_completion_and_shadowed_binding_remain_roots_during_restoration() {
    for throwing in [false, true] {
        let mut interpreter = Interpreter::new();
        let Value::Object(original) = Value::object([]) else {
            unreachable!();
        };
        let Value::Object(returned) = Value::object([]) else {
            unreachable!();
        };
        interpreter
            .variables
            .insert("x".into(), Value::Object(original.clone()));
        let snapshot = interpreter.variables.shadow("x".into(), Value::Undefined);
        let index = interpreter.execution_frames.len();
        interpreter
            .execution_frames
            .push(ExecutionFrame::ShadowedBindings(vec![snapshot]));
        let pending = if throwing {
            Err(JsError::thrown(Value::Object(returned.clone())))
        } else {
            Ok(Completion::Return(Value::Object(returned.clone())))
        };

        let (result, ()) = interpreter
            .with_pending_completion(pending, |interpreter| {
                for step in 0..2 {
                    let mut saw_original = false;
                    let mut saw_returned = false;
                    interpreter
                        .visit_runtime_roots(&mut |root| {
                            if let RuntimeRoot::Value(Value::Object(object)) = root {
                                saw_original |= Rc::ptr_eq(object, &original);
                                saw_returned |= Rc::ptr_eq(object, &returned);
                            }
                        })
                        .unwrap();
                    assert!(saw_original && saw_returned);
                    if step == 0 {
                        interpreter.restore_frame_bindings(index);
                    }
                }
                Ok(())
            })
            .unwrap();

        assert_eq!(result.is_err(), throwing);
        assert!(matches!(
            interpreter.execution_frames.pop(),
            Some(ExecutionFrame::ShadowedBindings(_))
        ));
        assert!(interpreter.execution_frames.is_empty());
    }
}

#[test]
fn primitive_conversion_frames_unwind_after_success_and_throw() {
    let mut interpreter = Interpreter::new();
    for (source, succeeds) in [
        ("+{ valueOf: function() { return 7; } };", true),
        ("+{ valueOf: function() { throw 'failure'; } };", false),
    ] {
        let program = parse_program(source).unwrap();
        assert_eq!(interpreter.execute_script(&program).is_ok(), succeeds);
        assert!(interpreter.execution_frames.is_empty(), "{source}");
    }
}

#[test]
fn for_of_items_remain_in_frames_and_unwind_on_errors() {
    let mut interpreter = Interpreter::new();

    let result = interpreter.with_for_of_items(
        vec![Value::Number(1.0), Value::Number(2.0)],
        |runtime, index| {
            assert_eq!(runtime.next_for_of_item(index), Some(Value::Number(1.0)));

            runtime.with_shadowed_bindings([(String::from("x"), Value::Undefined)], |runtime| {
                match &runtime.execution_frames[index] {
                    ExecutionFrame::ForOfItems(items) => {
                        assert_eq!(items.as_slice(), &[Value::Number(2.0)]);
                    }

                    _ => panic!("expected for-of frame"),
                }

                Err::<Completion, _>(JsError::resource_limit("test limit"))
            })
        },
    );

    assert!(result.is_err());
    assert!(interpreter.execution_frames.is_empty());

    let program = parse_program(
        r#"
            let source = [{value: 1}, {value: 2}];
            let sum = 0;

            for (let item of source) {
                source = [];

                for (let n of [3, 4]) {
                    sum = sum + n;
                }

                sum = sum + item.value;
            }

            let pairs = [[1, 2], [3, 4]];
            let pair_sum = 0;

            for (const [a, b] of pairs) {
                pairs = [];
                pair_sum = pair_sum + a + b;
            }

            sum === 17 && pair_sum === 10;
            "#,
    )
    .unwrap();

    let result = interpreter.execute_script(&program).unwrap().into_value();

    assert_eq!(result, Value::Boolean(true));
    assert!(interpreter.execution_frames.is_empty());
}

#[test]
fn switch_and_while_keep_values_across_nested_calls() {
    let mut interpreter = Interpreter::new();

    let program = parse_program(
        r#"
            let target = {value: 7};
            let source = [target];
            let matched = false;

            function choose() {
                let result = source[0];
                target = null;
                source = [];
                return result;
            }

            switch (target) {
                case choose():
                    matched = true;
                    break;
            }

            matched && target === null;
            "#,
    )
    .unwrap();

    let result = interpreter.execute_script(&program).unwrap().into_value();

    assert_eq!(result, Value::Boolean(true));
    assert!(interpreter.execution_frames.is_empty());

    let program = parse_program(
        r#"
            let counter = 0;
            let remembered = {value: 7};

            function next() {
                counter = counter + 1;

                if (counter === 2) {
                    remembered = null;
                    return false;
                }

                return true;
            }

            while (next()) {
                remembered;
            }
            "#,
    )
    .unwrap();

    let result = interpreter.execute_script(&program).unwrap().into_value();

    assert_eq!(
        interpreter.member_value(result, "value").unwrap(),
        Value::Number(7.0)
    );
    assert!(interpreter.execution_frames.is_empty());

    for source in ["switch (1) { case missing: break; }", "while (missing) {}"] {
        let program = parse_program(source).unwrap();

        assert!(interpreter.execute_script(&program).is_err());
        assert!(interpreter.execution_frames.is_empty());
    }
}

#[test]
fn reference_frames_survive_nested_frames_and_unwind_on_error() {
    let mut interpreter = Interpreter::new();
    let object = Value::array(vec![Value::Number(5.0)]);

    let reference = super::reference::Reference::Index {
        object: object.clone(),
        index: Value::Number(0.0),
    };

    let result = interpreter.with_active_reference(reference, |runtime, reference_index| {
        runtime.with_frame_value(Value::Number(9.0), |runtime, value_index| {
            assert_eq!(runtime.frame_value(value_index), &Value::Number(9.0));

            match runtime.active_reference(reference_index) {
                super::reference::Reference::Index {
                    object: actual,
                    index,
                } => {
                    assert_eq!(actual, &object);
                    assert_eq!(index, &Value::Number(0.0));
                }

                _ => panic!("expected index reference"),
            }

            Err::<(), _>(JsError::resource_limit("test limit"))
        })
    });

    assert!(result.is_err());
    assert!(interpreter.execution_frames.is_empty());
}

#[test]
fn pending_call_frames_remain_accessible_during_nested_calls() {
    let mut interpreter = Interpreter::new();

    let arguments = [super::Expression::Literal(Value::Number(3.0))];

    let result = interpreter.with_pending_call(
        Value::Number(1.0),
        Some(Value::Number(2.0)),
        &arguments,
        |runtime, index| {
            let nested = runtime.with_pending_call(Value::Null, None, &[], |runtime, inner| {
                assert_eq!(runtime.pending_call(index).callee, Value::Number(1.0));
                assert_eq!(runtime.pending_call(inner).callee, Value::Null);

                Err::<Value, _>(JsError::resource_limit("test limit"))
            });

            assert!(nested.is_err());

            let call = runtime.pending_call(index);

            assert_eq!(call.receiver, Some(Value::Number(2.0)));
            assert_eq!(call.arguments, vec![Value::Number(3.0)]);

            Ok(Value::Undefined)
        },
    );

    assert!(result.is_ok());
    assert!(interpreter.execution_frames.is_empty());
}

#[test]
fn call_result_remains_rooted_when_call_frame_is_removed() {
    for throwing in [false, true] {
        let mut interpreter = Interpreter::new();
        let Value::Object(object) = Value::object([]) else {
            unreachable!();
        };
        let index = interpreter.execution_frames.len();
        interpreter
            .execution_frames
            .push(ExecutionFrame::PendingCall(super::PendingCall {
                callee: Value::Undefined,
                receiver: None,
                arguments: vec![],
                handoff_value: None,
            }));
        let pending = if throwing {
            Err(JsError::thrown(Value::Object(object.clone())))
        } else {
            Ok(Completion::Return(Value::Object(object.clone())))
        };

        let (result, ()) = interpreter
            .with_pending_completion(pending, |interpreter| {
                interpreter.remove_pending_call(index);
                let mut found = false;
                interpreter
                    .visit_runtime_roots(&mut |root| {
                        if let RuntimeRoot::Value(Value::Object(value)) = root {
                            found |= Rc::ptr_eq(value, &object);
                        }
                    })
                    .unwrap();
                assert!(found);
                Ok(())
            })
            .unwrap();

        assert_eq!(result.is_err(), throwing);
        assert!(interpreter.execution_frames.is_empty());
    }
}

#[test]
fn call_handoff_keeps_result_rooted_until_destination_accepts_it() {
    let mut interpreter = Interpreter::new();
    let program = parse_program("(function() { return {}; });").unwrap();
    let callback = interpreter.execute_script(&program).unwrap().into_value();
    let mut returned_object = None;

    interpreter
        .with_temporary_values_scope(|interpreter, destination| {
            interpreter.dispatch_call_with_handoff(
                callback,
                None,
                vec![],
                |interpreter, result| {
                    let value = result?;
                    let Value::Object(object) = &value else {
                        panic!("expected object result");
                    };
                    let mut roots = 0;
                    interpreter
                        .visit_runtime_roots(&mut |root| {
                            if let RuntimeRoot::Value(Value::Object(value)) = root {
                                roots += usize::from(Rc::ptr_eq(value, object));
                            }
                        })
                        .unwrap();
                    assert_eq!(roots, 1);
                    returned_object = Some(object.clone());
                    interpreter.frame_values_mut(destination).push(value);
                    Ok(())
                },
            )?;

            let object = returned_object.as_ref().expect("callback result");
            let mut roots = 0;
            interpreter
                .visit_runtime_roots(&mut |root| {
                    if let RuntimeRoot::Value(Value::Object(value)) = root {
                        roots += usize::from(Rc::ptr_eq(value, object));
                    }
                })
                .unwrap();
            assert_eq!(roots, 1);
            Ok(())
        })
        .unwrap();
    assert!(interpreter.execution_frames.is_empty());
}

#[test]
fn expression_handoff_retains_result_until_destination_accepts_it() {
    let mut interpreter = Interpreter::new();
    let program = parse_program("({value: 7});").unwrap();
    let [super::Statement::Expression(expression)] = program.as_slice() else {
        panic!("expected an expression");
    };
    let mut returned_object = None;

    interpreter
        .with_temporary_values_scope(|interpreter, destination| {
            interpreter.evaluate_expression_with_handoff(expression, |interpreter, result| {
                let value = result?;
                let Value::Object(object) = &value else {
                    panic!("expected an object");
                };
                let mut found = false;
                interpreter
                    .visit_runtime_roots(&mut |root| {
                        if let RuntimeRoot::Value(Value::Object(value)) = root {
                            found |= Rc::ptr_eq(value, object);
                        }
                    })
                    .unwrap();
                assert!(found, "expression completion must retain the object");
                returned_object = Some(object.clone());
                interpreter.frame_values_mut(destination).push(value);
                Ok(())
            })?;
            let object = returned_object.as_ref().unwrap();
            assert!(matches!(
                interpreter.frame_values(destination),
                [Value::Object(value)] if Rc::ptr_eq(value, object)
            ));
            Ok(())
        })
        .unwrap();
    assert!(interpreter.execution_frames.is_empty());

    let program = parse_program("missing;").unwrap();
    let [super::Statement::Expression(expression)] = program.as_slice() else {
        panic!("expected an expression");
    };
    let error = interpreter
        .evaluate_expression_with_handoff(expression, |_, result| result)
        .unwrap_err();
    assert!(matches!(error.kind, JsErrorKind::NativeException(_)));
    assert!(interpreter.execution_frames.is_empty());
}

#[test]
fn builtin_results_survive_temporary_and_nested_call_handoffs() {
    for nested in [false, true] {
        let mut interpreter = Interpreter::new();
        let call = super::PendingCall {
            callee: Value::Undefined,
            receiver: None,
            arguments: vec![],
            handoff_value: None,
        };
        let result = interpreter
            .with_call_frame_handoff(
                call,
                |interpreter, index| {
                    let receiver = Some(Value::NativeFunction(
                        super::NativeFunction::PromiseConstructor,
                    ));
                    let value = if nested {
                        interpreter.dispatch_call(
                            Value::NativeFunction(super::NativeFunction::PromiseResolve),
                            receiver,
                            vec![Value::Number(1.0)],
                        )?
                    } else {
                        interpreter.promise_resolve(receiver, vec![Value::Number(1.0)])?
                    };
                    let Value::Object(object) = &value else {
                        panic!("expected a promise");
                    };
                    assert!(matches!(
                        &interpreter.pending_call(index).handoff_value,
                        Some(Value::Object(retained)) if Rc::ptr_eq(retained, object)
                    ));
                    let mut found = false;
                    interpreter
                        .visit_runtime_roots(&mut |root| {
                            if let RuntimeRoot::Value(Value::Object(value)) = root {
                                found |= Rc::ptr_eq(value, object);
                            }
                        })
                        .unwrap();
                    assert!(found, "call must retain the result");
                    Ok(value)
                },
                |interpreter, result| {
                    let value = result?;
                    let Value::Object(object) = &value else {
                        panic!("expected a promise");
                    };
                    let mut found = false;
                    interpreter
                        .visit_runtime_roots(&mut |root| {
                            if let RuntimeRoot::Value(Value::Object(value)) = root {
                                found |= Rc::ptr_eq(value, object);
                            }
                        })
                        .unwrap();
                    assert!(found, "completion must retain the result");
                    Ok(value)
                },
            )
            .unwrap();
        assert!(matches!(result, Value::Object(_)));
        assert!(interpreter.execution_frames.is_empty());
    }
}

#[test]
fn user_function_result_is_retained_after_locals_are_restored() {
    let mut interpreter = Interpreter::new();
    interpreter
        .variables
        .insert("item".to_owned(), Value::Number(99.0));
    let program=parse_program(
        "(function () { let item = {value: 7}; try { return item; } finally { item.value = 8; } });",
    ).unwrap();
    let Value::Function(function) = interpreter.execute_script(&program).unwrap().into_value()
    else {
        panic!("expected a function");
    };
    let call = super::PendingCall {
        callee: Value::Undefined,
        receiver: None,
        arguments: vec![],
        handoff_value: None,
    };

    let result = interpreter
        .with_call_frame_handoff(
            call,
            |interpreter, index| {
                let result =
                    interpreter.call_function_value_with_receiver(&function, vec![], None)?;
                let Value::Object(object) = &result else {
                    panic!("expected an object");
                };
                assert_eq!(
                    *interpreter.variables.get("item").unwrap(),
                    Value::Number(99.0)
                );
                assert_eq!(
                    interpreter
                        .host_roots
                        .read_ordinary_own_property(object, "value"),
                    Some(Value::Number(8.0))
                );
                assert!(matches!(
                    &interpreter.pending_call(index).handoff_value,
                    Some(Value::Object(retained)) if Rc::ptr_eq(retained, object)
                ));
                let mut found = false;
                interpreter
                    .visit_runtime_roots(&mut |root| {
                        if let RuntimeRoot::Value(Value::Object(value)) = root {
                            found |= Rc::ptr_eq(value, object);
                        }
                    })
                    .unwrap();
                assert!(found, "call must retain the function result");
                Ok(result)
            },
            |_, result| result,
        )
        .unwrap();

    assert!(matches!(result, Value::Object(_)));
    assert!(interpreter.execution_frames.is_empty());
}

#[test]
fn active_function_retains_return_while_nested_statements_unwind() {
    for source in [
        "(function() { try { return {value: 7}; } finally { return {value: 8}; } });",
        "(function() { try { return {value: 8}; } finally { while (true) { break; } } });",
    ] {
        let mut interpreter = Interpreter::new();
        let program = parse_program(source).unwrap();
        let Value::Function(function) = interpreter.execute_script(&program).unwrap().into_value()
        else {
            panic!("expected a function");
        };
        let body = function.definition().body.clone();
        let index = interpreter.execution_frames.len();
        interpreter
            .execution_frames
            .push(ExecutionFrame::ActiveCall(super::ActiveCallFrame {
                callee: function,
                arguments: vec![],
                replaced_bindings: vec![],
                local_names: None,
                return_value: None,
            }));

        let completion = interpreter.execute(&body).unwrap();
        let Completion::Return(Value::Object(object)) = &completion else {
            panic!("expected an object return");
        };
        assert_eq!(
            interpreter
                .host_roots
                .read_ordinary_own_property(object, "value"),
            Some(Value::Number(8.0))
        );
        assert!(matches!(
            &interpreter.active_call_frame(index).return_value,
            Some(Value::Object(value)) if Rc::ptr_eq(value, object)
        ));
        let mut found = false;
        interpreter
            .visit_runtime_roots(&mut |root| {
                if let RuntimeRoot::Value(Value::Object(value)) = root {
                    found |= Rc::ptr_eq(value, object);
                }
            })
            .unwrap();
        assert!(found);

        assert!(
            interpreter
                .execute(&parse_program("throw 1;").unwrap())
                .is_err()
        );
        assert!(interpreter.active_call_frame(index).return_value.is_none());
        assert!(matches!(
            interpreter.execution_frames.pop(),
            Some(ExecutionFrame::ActiveCall(_))
        ));
    }
}

#[test]
fn map_callback_exception_stays_rooted_while_temporary_frames_unwind() {
    let mut interpreter = Interpreter::new();
    let program = parse_program("(function() { throw {}; });").unwrap();
    let callback = interpreter.execute_script(&program).unwrap().into_value();
    let call = super::PendingCall {
        callee: Value::Undefined,
        receiver: None,
        arguments: vec![],
        handoff_value: None,
    };

    let result = interpreter.with_call_frame_handoff(
        call,
        |interpreter, _| {
            let result = interpreter.with_temporary_values_scope(|interpreter, destination| {
                interpreter.with_frame_value(Value::Undefined, |interpreter, _| {
                    interpreter.dispatch_call_into_temporary_frame(
                        callback,
                        None,
                        vec![],
                        destination,
                    )
                })
            });

            let Err(JsError {
                kind: JsErrorKind::Thrown(Value::Object(object)),
                ..
            }) = &result
            else {
                panic!("expected an object exception");
            };
            let mut found = false;
            interpreter
                .visit_runtime_roots(&mut |root| {
                    if let RuntimeRoot::Value(Value::Object(value)) = root {
                        found |= Rc::ptr_eq(value, object);
                    }
                })
                .unwrap();
            assert!(found, "outer call must retain the exception");
            result.map(|()| Value::Undefined)
        },
        |interpreter, result| {
            let Err(JsError {
                kind: JsErrorKind::Thrown(Value::Object(object)),
                ..
            }) = &result
            else {
                panic!("expected an object exception");
            };
            let mut found = false;
            interpreter
                .visit_runtime_roots(&mut |root| {
                    if let RuntimeRoot::Value(Value::Object(value)) = root {
                        found |= Rc::ptr_eq(value, object);
                    }
                })
                .unwrap();
            assert!(found, "completion must retain the exception");
            result
        },
    );

    assert!(matches!(
        result,
        Err(JsError {
            kind: JsErrorKind::Thrown(_),
            ..
        })
    ));
    assert!(interpreter.execution_frames.is_empty());
}

#[test]
fn exception_scopes_transfer_and_clear_caught_errors() {
    let mut interpreter = Interpreter::new();
    let Value::Object(object) = Value::object([]) else {
        unreachable!();
    };
    interpreter
        .with_exception_scope(|interpreter| {
            let result = interpreter.with_exception_scope(|_| {
                Err::<(), _>(JsError::thrown(Value::Object(object.clone())))
            });
            assert!(result.is_err());

            let has_root = |interpreter: &Interpreter| {
                let mut found = false;
                interpreter
                    .visit_runtime_roots(&mut |root| {
                        if let RuntimeRoot::Value(Value::Object(value)) = root {
                            found |= Rc::ptr_eq(value, &object);
                        }
                    })
                    .unwrap();
                found
            };
            assert!(has_root(interpreter));
            interpreter.clear_exception();
            assert!(!has_root(interpreter));
            Ok(())
        })
        .unwrap();
    assert!(interpreter.execution_frames.is_empty());
}

#[test]
fn map_scopes_results_through_nested_callbacks_and_errors() {
    let mut interpreter = Interpreter::new();

    let program = parse_program(
        r#"
            let source = [1,,3];
            let calls = 0;

            let mapped = source.map(function(value, index) {
                calls = calls + 1;

                if (index === 0) {
                    source[2] = 4;
                }

                let nested = [2].map(function(n) {
                    return n + 1;
                });

                return {value: value + nested[0]};
            });

            mapped.length === 3 &&
                !mapped.hasOwnProperty(1) &&
                mapped[0].value === 4 &&
                mapped[2].value === 7 &&
                calls === 2;
            "#,
    )
    .unwrap();

    let result = interpreter.execute_script(&program).unwrap().into_value();

    assert_eq!(result, Value::Boolean(true));
    assert!(interpreter.execution_frames.is_empty());

    let program = parse_program(
        r#"
            [1, 2].map(function(value) {
                if (value === 2) {
                    throw 'error';
                }

                return {value: value};
            });
            "#,
    )
    .unwrap();

    assert!(interpreter.execute_script(&program).is_err());
    assert!(interpreter.execution_frames.is_empty());
}

#[test]
fn internal_callback_dispatch_unwinds_frames() {
    let mut interpreter = Interpreter::new();

    let program = parse_program(
        r#"
            let trace = '';

            let found = [1, 2, 3].some(function(value) {
                trace = trace + value;
                return value === 2;
            });

            let replaced = 'aba'.replace(/a/g, function() {
                trace = trace + 'r';
                return 'x';
            });

            let object = {
                valueOf: function() {
                    trace = trace + 'v';
                    return {};
                },

                toString: function() {
                    trace = trace + 's';
                    return '4';
                }
            };

            found &&
                replaced === 'xbx' &&
                object + 1 === '41' &&
                trace === '12rrvs';
            "#,
    )
    .unwrap();

    let result = interpreter.execute_script(&program).unwrap().into_value();

    assert_eq!(result, Value::Boolean(true));
    assert!(interpreter.execution_frames.is_empty());

    let program = parse_program("[1].some(function() { throw 'error'; });").unwrap();

    assert!(interpreter.execute_script(&program).is_err());
    assert!(interpreter.execution_frames.is_empty());
}

#[test]
fn host_roots_keep_owner_alive_and_release_without_registry_borrow() {
    use std::rc::Rc;

    let interpreter = Interpreter::new();
    let owner = Rc::downgrade(&interpreter.host_roots);

    let value = interpreter.export_value(Value::array(vec![Value::Number(7.0)]));
    let value_root = Rc::downgrade(&value.0);
    let clone = value.clone();

    let error = interpreter.export_error(JsError::thrown(Value::array(vec![])));

    let registry = owner.upgrade().unwrap();

    assert_eq!(
        registry
            .roots
            .borrow()
            .iter()
            .filter(|root| root.upgrade().is_some())
            .count(),
        2
    );

    drop(interpreter);
    drop(value);

    assert!(value_root.upgrade().is_some());

    let borrowed = registry.roots.borrow_mut();

    drop(clone);

    assert!(value_root.upgrade().is_none());

    drop(borrowed);

    assert_eq!(
        registry
            .roots
            .borrow()
            .iter()
            .filter(|root| root.upgrade().is_some())
            .count(),
        1
    );

    drop(error);
    drop(registry);

    assert!(owner.upgrade().is_none());
}

#[test]
fn host_owner_keeps_its_heap_alive_and_separate() {
    use crate::javascript::heap::{HeapArrayState, HeapObject, HeapObjectState};
    use std::rc::Rc;

    let interpreter = Interpreter::new();
    let weak_owner = Rc::downgrade(&interpreter.host_roots);

    let host_value = interpreter.export_value(Value::Boolean(true));

    let object = interpreter
        .host_roots
        .allocate_heap_object(HeapObject::Object(HeapObjectState::default()))
        .unwrap();

    let array = interpreter
        .host_roots
        .allocate_heap_object(HeapObject::Array(HeapArrayState::default()))
        .unwrap();

    let other = Interpreter::new();

    assert!(!other.host_roots.contains_heap_object(object).unwrap());

    drop(interpreter);

    let owner = weak_owner.upgrade().unwrap();
    assert!(owner.contains_heap_object(object).unwrap());
    assert!(owner.contains_heap_object(array).unwrap());
    drop(owner);
    drop(host_value);

    assert!(weak_owner.upgrade().is_none());
}

#[test]
fn host_heap_access_validates_references_and_releases_borrows() {
    use crate::javascript::heap::{
        HeapAccessError, HeapArrayState, HeapObject, HeapObjectState, HeapPrototype, HeapValue,
    };
    let interpreter = Interpreter::new();
    let owner = &interpreter.host_roots;
    let object = owner
        .allocate_heap_object(HeapObject::Object(HeapObjectState::default()))
        .unwrap();
    let array = owner
        .allocate_heap_object(HeapObject::Array(HeapArrayState::default()))
        .unwrap();
    let other = Interpreter::new();
    let foreign = other
        .host_roots
        .allocate_heap_object(HeapObject::Object(HeapObjectState::default()))
        .unwrap();
    assert_eq!(owner.read_heap_property(object, "x"), Ok(None));
    owner
        .write_heap_property(object, "x".into(), HeapValue::Number(42.0))
        .unwrap();
    assert_eq!(
        owner.read_heap_property(object, "x"),
        Ok(Some(HeapValue::Number(42.0)))
    );
    owner
        .write_heap_property(object, "link".into(), HeapValue::Reference(array))
        .unwrap();
    owner
        .write_heap_property(object, "self".into(), HeapValue::Reference(object))
        .unwrap();
    assert_eq!(
        owner.read_heap_property(object, "self"),
        Ok(Some(HeapValue::Reference(object)))
    );
    assert_eq!(
        owner.read_heap_property(object, "link"),
        Ok(Some(HeapValue::Reference(array)))
    );
    assert_eq!(
        owner.write_heap_property(object, "x".into(), HeapValue::Reference(foreign)),
        Err(HeapAccessError::InvalidReference(foreign))
    );
    assert_eq!(
        owner.read_heap_property(object, "x"),
        Ok(Some(HeapValue::Number(42.0)))
    );
    assert_eq!(
        owner.read_heap_property(foreign, "x"),
        Err(HeapAccessError::InvalidObject(foreign))
    );
    assert_eq!(
        owner.read_heap_property(array, "0"),
        Err(HeapAccessError::NotOrdinaryObject(array))
    );
    let bad = HeapObjectState {
        prototype: HeapPrototype::Object(foreign),
        ..HeapObjectState::default()
    };
    assert_eq!(
        owner.allocate_heap_object(HeapObject::Object(bad)),
        Err(HeapAccessError::InvalidReference(foreign))
    );
    {
        let _guard = owner.heap_cell().unwrap().borrow_mut();
        assert_eq!(
            owner.read_heap_property(object, "x"),
            Err(HeapAccessError::Borrowed)
        );
        assert_eq!(
            owner.allocate_heap_object(HeapObject::Object(HeapObjectState::default())),
            Err(HeapAccessError::Borrowed)
        );
    }
    owner.heap_cell().unwrap().borrow_mut().remove(array);
    assert_eq!(
        owner.read_heap_property(object, "link"),
        Err(HeapAccessError::InvalidReference(array))
    );
}

#[test]
fn host_root_snapshot_retains_live_values_and_prunes_dead_entries() {
    use std::rc::Rc;

    let interpreter = Interpreter::new();
    let registry = interpreter.host_roots.clone();
    let owner = Rc::downgrade(&registry);

    let first = interpreter.export_value(Value::Number(1.0));
    let clone = first.clone();

    let discarded = interpreter.export_value(Value::Number(99.0));
    drop(discarded);

    let error = interpreter.export_error(JsError::thrown(Value::array(vec![])));

    let roots = registry.snapshot();

    // Clones share one registration; the expired entry is removed.
    assert_eq!(roots.len(), 2);
    assert_eq!(registry.roots.borrow().len(), 2);
    assert!(matches!(roots[0].value, Value::Number(1.0)));
    assert!(matches!(roots[1].value, Value::Array(_)));

    drop(interpreter);
    drop(registry);
    drop(first);
    drop(clone);
    drop(error);

    assert!(owner.upgrade().is_some());

    drop(roots);

    assert!(owner.upgrade().is_none());
}

#[test]
fn visits_frame_roots_and_rejects_borrowed_bindings() {
    use std::collections::HashSet;

    let mut interpreter = Interpreter::new();

    interpreter
        .variables
        .insert("x".to_owned(), Value::Number(2.0));

    let names = vec!["x".to_owned()];

    let captured = interpreter
        .variables
        .capture_names(&names, &HashSet::from(["x".to_owned()]));

    let snapshot = interpreter
        .variables
        .shadow("x".to_owned(), Value::Undefined);

    let program = parse_program("function f(v) { return v; } Promise.resolve(7).then(f);").unwrap();

    interpreter.execute(&program).unwrap();

    let callee = interpreter.variables.get("f").unwrap().clone();

    let Value::Function(callee) = callee else {
        panic!("expected function");
    };

    let job = interpreter
        .microtasks
        .pop_front()
        .expect("expected promise job");

    let frames = vec![
        ExecutionFrame::PendingCompletion(Err(JsError::thrown(Value::Number(1.0)))),
        ExecutionFrame::ShadowedBindings(vec![snapshot]),
        ExecutionFrame::ForOfItems(vec![Value::Number(3.0)].into_iter()),
        ExecutionFrame::TemporaryValues(vec![Value::Number(4.0)]),
        ExecutionFrame::Reference(super::reference::Reference::Index {
            object: Value::Number(5.0),
            index: Value::Number(6.0),
        }),
        ExecutionFrame::PendingCall(super::PendingCall {
            callee: Value::Number(7.0),
            receiver: Some(Value::Number(8.0)),
            arguments: vec![Value::Number(9.0)],
            handoff_value: Some(Value::Number(12.0)),
        }),
        ExecutionFrame::ActiveCall(super::ActiveCallFrame {
            callee,
            arguments: vec![Value::Number(10.0)],
            replaced_bindings: Vec::new(),
            local_names: None,
            return_value: Some(Value::Number(13.0)),
        }),
        ExecutionFrame::Microtask(super::microtasks::Microtask::Callback(Value::Number(11.0))),
        ExecutionFrame::Microtask(job),
    ];

    let mut numbers = Vec::new();
    let mut functions = 0;
    let mut promise_objects = 0;
    let mut handler_values = 0;

    for frame in &frames {
        frame
            .visit_roots(&mut |root| match root {
                RuntimeRoot::Value(Value::Number(number)) => {
                    numbers.push(*number);
                }

                RuntimeRoot::Value(Value::Function(_)) => {
                    handler_values += 1;
                }

                RuntimeRoot::Function(_) => functions += 1,
                RuntimeRoot::Object(object)
                    if interpreter.host_roots.ordinary_promise(object).is_some() =>
                {
                    promise_objects += 1;
                }
                RuntimeRoot::Promise(_) => {}
                RuntimeRoot::Value(_) => {}
                RuntimeRoot::Object(_)
                | RuntimeRoot::Array(_)
                | RuntimeRoot::Document(_)
                | RuntimeRoot::RegExp(_)
                | RuntimeRoot::Arguments(_) => {}
            })
            .unwrap();
    }

    for number in 1..=13 {
        assert!(numbers.contains(&(number as f64)));
    }

    assert_eq!(functions, 1);
    assert_eq!(promise_objects, 1);
    assert_eq!(handler_values, 1);

    let _guard = captured.get("x").unwrap().borrow_mut();

    assert_eq!(
        frames[1].visit_roots(&mut |_| {}),
        Err(RootVisitError::BindingBorrowed)
    );
}

#[test]
fn runtime_roots_cover_host_globals_jobs_canvas_and_reject_borrowed_bindings() {
    use std::collections::HashMap;

    let mut interpreter = Interpreter::new();
    let mut document = crate::javascript::DocumentState::default();

    document.register_parsed_element("canvas", "c", String::new(), None);
    interpreter.set_document(document);

    let program = parse_program(
        "let marker = 22; \
            let ctx = document.getElementById('c').getContext('2d'); \
            ctx.fillStyle = 'red'; \
            ctx.save(); \
            ctx.fillStyle = 'blue'; \
            function f() {}",
    )
    .unwrap();

    interpreter.execute(&program).unwrap();

    let callee = interpreter.variables.get("f").unwrap().clone();

    let Value::Function(callee) = callee else {
        panic!("expected function");
    };

    interpreter.cleanups.push(super::Cleanup {
        callback: callee,
        captured: HashMap::from([("saved".into(), Value::Number(66.0))]),
    });

    interpreter
        .microtasks
        .push_back(super::microtasks::Microtask::Callback(Value::Number(77.0)));

    interpreter
        .execution_frames
        .push(ExecutionFrame::TemporaryValues(vec![Value::Number(55.0)]));

    let _host = interpreter.export_value(Value::Number(44.0));

    let other = Interpreter::new();
    let _foreign = other.export_value(Value::Number(88.0));

    let mut numbers = Vec::new();
    let mut saw_red = false;
    let mut saw_prototype = false;
    let mut saw_document = false;
    let mut saw_array = false;
    let mut functions = 0;

    let snapshot = interpreter
        .visit_runtime_roots(&mut |root| match root {
            RuntimeRoot::Value(Value::Number(number)) => {
                numbers.push(*number);
            }

            RuntimeRoot::Value(Value::String(value)) if value == "#ff0000" => {
                saw_red = true;
            }

            RuntimeRoot::Object(object) if Rc::ptr_eq(object, &interpreter.object_prototype) => {
                saw_prototype = true;
            }

            RuntimeRoot::Document(_) => saw_document = true,
            RuntimeRoot::Array(_) => saw_array = true,
            RuntimeRoot::Function(_) => functions += 1,
            _ => {}
        })
        .unwrap();

    for number in [22.0, 44.0, 55.0, 66.0, 77.0] {
        assert!(numbers.contains(&number));
    }

    assert!(!numbers.contains(&88.0));
    assert_eq!(snapshot.len(), 1);
    assert!(saw_red);
    assert!(saw_prototype);
    assert!(saw_document);
    assert!(saw_array);
    assert_eq!(functions, 1);

    let captured = interpreter.variables.capture_names(
        &["marker".to_owned()],
        &HashSet::from(["marker".to_owned()]),
    );

    let _guard = captured.get("marker").unwrap().borrow_mut();

    assert!(matches!(
        interpreter.visit_runtime_roots(&mut |_| {}),
        Err(RootVisitError::BindingBorrowed)
    ));
}

#[test]
fn visits_bound_function_references_and_rejects_borrowed_captures() {
    let mut interpreter = Interpreter::new();
    let program = parse_program(
        "function factory(x) { return function() { return x; }; } \
            let captured = {id: 1}; let receiver = {id: 2}; \
            let argument = {id: 3}; let f = factory(captured); \
            let bound = f.bind(receiver, argument);",
    )
    .unwrap();

    interpreter.execute(&program).unwrap();

    let value = interpreter.variables.get("bound").unwrap().clone();
    let Value::Function(bound) = value else {
        panic!("expected function");
    };

    let target = interpreter.variables.get("f").unwrap().clone();
    let captured = interpreter.variables.get("captured").unwrap().clone();
    let receiver = interpreter.variables.get("receiver").unwrap().clone();
    let argument = interpreter.variables.get("argument").unwrap().clone();

    let mut objects = 0;
    let mut found_target = false;
    let mut values = Vec::new();

    bound
        .visit_references(&mut |root| match root {
            RuntimeRoot::Object(_) => objects += 1,
            RuntimeRoot::Function(value) => {
                found_target = Value::Function(value.clone()) == target;
            }
            RuntimeRoot::Value(value) => values.push(value.clone()),
            _ => {}
        })
        .unwrap();

    assert_eq!(objects, 1);
    assert!(found_target);

    for expected in [captured, receiver, argument] {
        assert!(values.contains(&expected));
    }

    let _guard = bound.captures().get("x").unwrap().borrow_mut();

    assert_eq!(
        bound.visit_references(&mut |_| {}),
        Err(RootVisitError::BindingBorrowed)
    );
}
