use crate::javascript::{Value, evaluate};

#[test]
fn evaluates_user_function_with_arguments() {
    let result = evaluate("const f = function(a, b) { return a + b; }; f(2, 3);").unwrap();

    assert_eq!(result, Value::Number(5.0));
}

#[test]
fn restores_shadowed_binding_after_user_function() {
    let result = evaluate(
        "let x = 9; \
            const f = function(x) { let y = 3; return x + y; }; \
            f(2); x;",
    )
    .unwrap();

    assert_eq!(result, Value::Number(9.0));
}

#[test]
fn restores_outer_parameter_after_nested_function_call() {
    let result = evaluate(
        "const outer = function(x) { \
                const inner = function(x) { return x + 1; }; \
                inner(2); \
                return x; \
            }; \
            outer(7);",
    )
    .unwrap();

    assert_eq!(result, Value::Number(7.0));
}

#[test]
fn bare_return_returns_undefined() {
    for source in [
        "function f() { return; } f();",
        "function f() { return } f();",
        "function f() { return\n; } f();",
    ] {
        assert_eq!(evaluate(source).unwrap(), Value::Undefined, "{source:?}");
    }
}

#[test]
fn return_line_terminator_prevents_returning_the_next_expression() {
    for separator in [
        "\n",
        "\r",
        "\r\n",
        "\u{2028}",
        "\u{2029}",
        " /*\n*/ ",
        " // comment\n",
        " // comment\r",
        " // comment\u{2028}",
        " // comment\u{2029}",
    ] {
        let source = format!("function f() {{ return{separator}42; }} f();");

        assert_eq!(evaluate(&source).unwrap(), Value::Undefined, "{source:?}");
    }
}

#[test]
fn return_with_expression_and_comment_returns_the_value() {
    let result = evaluate("function f() { return /* comment */ 42; } f();").unwrap();

    assert_eq!(result, Value::Number(42.0));
}

#[test]
fn return_supports_regexp_literal() {
    let result = evaluate("function f() { return /x/; } f().test('x');").unwrap();

    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn evaluates_default_parameters_in_order() {
    let result = evaluate(
        "function f(a = 4, b = a + 1) { return b; } \
            f() === 5 && f(10) === 11 && f(undefined, 8) === 8;",
    )
    .unwrap();

    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn defaults_apply_only_to_undefined_and_arguments_are_flexible() {
    let result = evaluate(
        "function f(x = 9) { return x; } \
            f(null) === null && \
            f(0) === 0 && \
            f(undefined) === 9 && \
            f(3, 4) === 3;",
    )
    .unwrap();

    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn evaluates_default_each_call_and_skips_when_argument_is_supplied() {
    let result = evaluate(
        "let count = 0; \
            function f(x = (count += 1)) { return x; } \
            f(10); \
            f(); \
            f(); \
            count;",
    )
    .unwrap();

    assert_eq!(result, Value::Number(2.0));
}

#[test]
fn restores_parameter_binding_if_default_throws() {
    let result = evaluate(
        "let x = 7; \
            let caught = ''; \
            function f(x = missing) {} \
            try { f(); } catch (e) { caught = e.name; } \
            x === 7 && caught === 'ReferenceError';",
    )
    .unwrap();

    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn collects_remaining_arguments_in_a_fresh_array() {
    let result = evaluate(
        "function f(a, ...rest) { return rest; } \
            let x = f(1, 2, 3); \
            let y = f(1, 2, 3); \
            x[0] = 9; \
            x.length === 2 && \
            x[0] === 9 && \
            x[1] === 3 && \
            y[0] === 2 && \
            x !== y;",
    )
    .unwrap();

    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn rest_parameter_handles_missing_arguments_and_defaults() {
    let result = evaluate(
        "function f(a = 2, ...rest) { \
                return a + rest.length; \
            } \
            f() === 2 && \
            f(undefined, 4, 5) === 4 && \
            f(7) === 7;",
    )
    .unwrap();

    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn rest_elements_keep_object_identity() {
    let result = evaluate(
        "let object = {}; \
            function f(...args) { return args[0]; } \
            f(object) === object;",
    )
    .unwrap();

    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn restores_outer_rest_binding_on_default_failure() {
    let result = evaluate(
        "let rest = 42; \
            let error = ''; \
            function f(a = missing, ...rest) {} \
            try { f(); } catch (e) { error = e.name; } \
            rest === 42 && error === 'ReferenceError';",
    )
    .unwrap();

    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn function_identity_is_shared_by_aliases_but_not_identical_definitions() {
    let result = evaluate(
        "let f = function() { return 1; }; \
            let alias = f; \
            let other = function() { return 1; }; \
            let obj = { f }; \
            f === alias && obj.f === f && f !== other;",
    )
    .unwrap();

    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn evaluating_the_same_function_expression_twice_creates_distinct_functions() {
    let result = evaluate(
        "function make() { \
                return function() { return 1; }; \
            } \
            let first = make(); \
            let second = make(); \
            first !== second && \
            first() === 1 && \
            second() === 1;",
    )
    .unwrap();

    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn same_value_assertions_use_function_identity() {
    let result = evaluate(
        "let f = function() {}; \
            let alias = f; \
            let other = function() {}; \
            assert_equals(f, alias); \
            assert_not_equals(f, other); \
            true;",
    )
    .unwrap();

    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn ordinary_function_prototype_is_shared_and_replaceable() {
    let result = evaluate(
        "function F() {} \
            let alias = F; \
            let original = F.prototype; \
            original.x = 3; \
            let before = alias['prototype'].x; \
            let replacement = Object.create(null); \
            F['prototype'] = replacement; \
            before === 3 && \
            alias.prototype === replacement && \
            original !== replacement;",
    )
    .unwrap();

    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn ordinary_function_get_distinct_prototypes_and_arrows_start_without_one() {
    let result = evaluate(
        "let f = function() {}; \
            let g = function() {}; \
            let a = () => 1; \
            let b = x => x; \
            let empty = \
                a.prototype === undefined && \
                b.prototype === undefined; \
            a.prototype = 7; \
            empty && \
            a.prototype === 7 && \
            f.prototype !== g.prototype;",
    )
    .unwrap();

    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn assigning_function_properties_through_aliases_is_observable() {
    let result = evaluate(
        "function f() {} \
            let alias = f; \
            alias['count'] = 1; \
            f.count += 2; \
            alias.count === 3;",
    )
    .unwrap();

    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn executes_prototype_assignment_statement_from_testharness() {
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/wpt/resources/testharness.js"
    ));

    let fragment = "DedicatedWorkerTestEnvironment.prototype = Object.create(WorkerTestEnvironment.prototype);";

    assert!(source.contains(fragment));

    let script = format!(
        "function WorkerTestEnvironment() {{}} \
            function DedicatedWorkerTestEnvironment() {{}} \
            WorkerTestEnvironment.prototype.marker = 42; \
            {fragment} \
            DedicatedWorkerTestEnvironment.prototype.marker === 42;"
    );

    assert_eq!(evaluate(&script).unwrap(), Value::Boolean(true));
}

#[test]
fn debug_format_of_self_referencing_function_is_finite() {
    let error = evaluate(
        "function f() {} \
            f.self = f; \
            assert_not_equals(f, f);",
    )
    .unwrap_err();

    assert!(error.message.contains("FunctionValue"));
}

#[test]
fn restores_parameter_binding_when_default_initializer_throws() {
    let result = evaluate(
        r#"
                let value = 9;

                function f(value = missing) {
                    return value;
                }

                try {
                    f();
                } catch (error) {}

                value;
            "#,
    )
    .unwrap();

    assert_eq!(result, Value::Number(9.0));
}

#[test]
fn returned_closures_share_cells_but_separate_calls_do_not() {
    let result = evaluate(
        r#"
                function make() {
                    let count = 0;

                    return {
                        inc: function() {
                            count += 1;
                            return count;
                        },

                        read: function() {
                            return count;
                        }
                    };
                }

                let first = make();
                let second = make();

                first.inc() === 1 &&
                    first.inc() === 2 &&
                    first.read() === 2 &&
                    second.read() === 0 &&
                    second.inc() === 1 &&
                    first.read() === 2;
            "#,
    )
    .unwrap();

    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn nested_closure_keeps_captures_of_its_creator() {
    let result = evaluate(
        r#"
                function outer() {
                    let count = 10;

                    return function() {
                        let step = 2;

                        return function() {
                            count += step;
                            return count;
                        };
                    };
                }

                let middle = outer();
                let inner = middle();

                inner() === 12 && inner() === 14;
            "#,
    )
    .unwrap();

    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn restores_caller_bindings_when_closure_throws() {
    let result = evaluate(
        r#"
                function make() {
                    let x = 3;

                    return function() {
                        x += 1;
                        throw x;
                    };
                }

                let x = 99;
                let f = make();
                let caught = 0;

                try {
                    f();
                } catch (e) {
                    caught = e;
                }

                x === 99 && caught === 4;
            "#,
    )
    .unwrap();

    assert_eq!(result, Value::Boolean(true));

    let objects = evaluate(
        r#"
                let item = { value: 1 };
                let expected = { value: 2 };
                function forward(item) { return item; }
                function fail(item) { throw item; }
                let returned = forward(expected);
                let caught;
                try { fail(expected); } catch (error) { caught = error; }
                returned === expected && caught === expected && item.value === 1;
            "#,
    )
    .unwrap();
    assert_eq!(objects, Value::Boolean(true));
}

#[test]
fn closures_work_with_call_bind_and_construction() {
    let result = evaluate(
        r#"
                function make(base) {
                    return function(n) {
                        this.value = base + n;
                        return this.value;
                    };
                }

                let f = make(10);
                let a = {};
                let b = {};

                let bound = f.bind(b, 2);
                let called = f.call(a, 1);
                let bound_result = bound();
                let instance = new f(3);

                called === 11 &&
                    a.value === 11 &&
                    bound_result === 12 &&
                    b.value === 12 &&
                    instance.value === 13;
            "#,
    )
    .unwrap();

    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn closure_created_in_default_parameter_keeps_parameter_binding() {
    let result = evaluate(
        r#"
                function make(value, reader = function() {
                    return value;
                }) {
                    return reader;
                }

                let first = make(3);
                let second = make(5);

                first() === 3 && second() === 5;
            "#,
    )
    .unwrap();

    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn creates_async_functions_but_does_not_execute_them() {
    use crate::javascript::{DocumentScriptSession, DocumentState, JsErrorKind};

    let mut document = DocumentState::with_title("");
    let mut session = DocumentScriptSession::new(&document);

    let result = session
        .evaluate(
            "let count = 0; \
            async function f(x = (count = 1)) { \
                count = 2; \
                return await x; \
            } \
            typeof f === 'function' && f.prototype === undefined;",
            &mut document,
        )
        .unwrap();

    assert_eq!(result, Value::Boolean(true));

    let error = session.evaluate("f();", &mut document).unwrap_err();

    assert_eq!(error.kind, JsErrorKind::UnsupportedFeature);

    assert_eq!(
        session.evaluate("count;", &mut document).unwrap(),
        Value::Number(0.0)
    );

    assert!(matches!(
        evaluate("async function f() {} new f();").unwrap_err().kind,
        JsErrorKind::NativeException(_)
    ));
}

#[test]
fn functions_without_return_ignore_last_statement_value() {
    let result = evaluate(
        r#"
                let shared = {};

                function f() {
                    42;
                }

                function g() {
                    shared;
                }

                let arrow = () => 42;

                let block_arrow = () => {
                    42;
                };

                f() === undefined &&
                    g() === undefined &&
                    arrow() === 42 &&
                    block_arrow() === undefined;
            "#,
    )
    .unwrap();

    assert_eq!(result, Value::Boolean(true));

    // A top-level script still returns its last expression value.
    assert_eq!(evaluate("1; 2;").unwrap(), Value::Number(2.0));
}

#[test]
fn constructor_fallthrough_keeps_instance_but_explicit_object_return_wins() {
    let result = evaluate(
        r#"
                let replacement = { marker: 2 };

                function A() {
                    this.items = [];
                }

                A.prototype.marker = 1;

                function B() {
                    this.items = [];
                    return replacement;
                }

                let a = new A();
                let b = new B();

                a instanceof A &&
                    a.marker === 1 &&
                    a.items.length === 0 &&
                    b === replacement &&
                    b.marker === 2;
            "#,
    )
    .unwrap();

    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn promise_callback_without_return_fulfills_with_undefined() {
    use crate::javascript::{DocumentScriptSession, DocumentState};

    let mut document = DocumentState::with_title("");
    let mut session = DocumentScriptSession::new(&document);

    session
        .evaluate(
            r#"
                let result = 'unset';

                Promise.resolve(1)
                    .then(function(x) {
                        x + 1;
                    })
                    .then(function(x) {
                        result = typeof x;
                    });
            "#,
            &mut document,
        )
        .unwrap();

    assert_eq!(
        session.evaluate("result;", &mut document).unwrap(),
        Value::String("undefined".to_owned())
    );
}

#[test]
fn constructs_shell_environment_from_wpt_testharness() {
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/wpt/resources/testharness.js"
    ));

    let start = source
        .find("    function ShellTestEnvironment() {")
        .expect("WPT constructor must exist");

    let end = source[start..]
        .find("    function create_test_environment() {")
        .expect("WPT section end must exist")
        + start;

    let fragment = &source[start..end];

    let script = format!(
        "{fragment}\n\
            let environment = new ShellTestEnvironment(); \
            environment.test_timeout() === null && \
                environment.message_ports.length === 0 && \
                typeof environment.add_on_loaded_callback === 'function';"
    );

    assert_eq!(evaluate(&script).unwrap(), Value::Boolean(true));
}

#[test]
fn arguments_length_counts_actual_arguments_and_is_an_object() {
    let result = evaluate(
        r#"
                function count(a, b) {
                    return arguments.length;
                }

                count() === 0 &&
                    count(1) === 1 &&
                    count(1, 2, 3) === 3 &&
                    typeof count(1, 2) === 'number';
            "#,
    )
    .unwrap();

    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn arguments_identity_type_and_indexed_length() {
    let result = evaluate(
        r#"
                function get() {
                    return arguments;
                }

                let a = get(1, 2);
                let same = a;
                let b = get(1, 2);

                typeof a === 'object' &&
                    a === same &&
                    a !== b &&
                    a.length === 2 &&
                    a['length'] === 2 &&
                    Object.prototype.toString.call(a) === '[object Arguments]';
            "#,
    )
    .unwrap();

    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn arrows_capture_outer_arguments_and_ordinary_functions_get_their_own() {
    let result = evaluate(
        r#"
                function outer() {
                    let arrow = () => arguments.length;

                    function inner() {
                        return arguments.length;
                    }

                    return [arrow, inner];
                }

                let functions = outer(1, 2, 3);

                functions[0]() === 3 &&
                    functions[1](1) === 1;
            "#,
    )
    .unwrap();

    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn arguments_are_available_in_defaults_and_parameter_shadowing_is_preserved() {
    let result = evaluate(
        r#"
                function defaults(a = arguments.length) {
                    return a;
                }

                function named(arguments) {
                    return arguments.length;
                }

                defaults() === 0 &&
                    defaults(undefined, 4) === 2 &&
                    named([1, 2, 3]) === 3;
            "#,
    )
    .unwrap();

    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn restores_outer_arguments_after_return_and_throw() {
    let result = evaluate(
        r#"
                let arguments = 99;

                function count() {
                    return arguments.length;
                }

                function fail() {
                    throw arguments.length;
                }

                let result = 0;

                try {
                    fail(1, 2);
                } catch (error) {
                    result = error;
                }

                count(1) === 1 &&
                    result === 2 &&
                    arguments === 99;
            "#,
    )
    .unwrap();

    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn unsupported_arguments_features_are_explicit() {
    use crate::javascript::JsErrorKind;

    for source in [
        "function f(a) { return arguments[0]; } f(3);",
        "function f() { return arguments.callee; } f();",
        "function f() { arguments.length = 2; } f();",
        "function f() { var arguments; } f();",
    ] {
        assert_eq!(
            evaluate(source).unwrap_err().kind,
            JsErrorKind::UnsupportedFeature,
            "{source}"
        );
    }
}

#[test]
fn reads_arguments_length_in_real_wpt_step_fragment() {
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/wpt/resources/testharness.js",
    ));

    let fragment = source
        .lines()
        .map(str::trim)
        .find(|line| *line == "if (arguments.length === 1) {")
        .expect("WPT step condition must exist");

    let script = format!(
        "function f(a, b) {{ \
                {fragment} \
                    return 1; \
                }} \
                return 0; \
            }} \
            f(9) === 1 && f(9, 8) === 0;"
    );

    assert_eq!(evaluate(&script).unwrap(), Value::Boolean(true));
}

#[test]
fn reads_unmapped_indices_and_preserves_argument_identity() {
    let result = evaluate(
        r#"
                let object = {};

                function read() {
                    return arguments;
                }

                let args = read(object, 'b');

                args[0] === object &&
                    args['1'] === 'b' &&
                    args.length === 2;
            "#,
    )
    .unwrap();

    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn extra_arguments_are_readable_but_mapped_candidates_are_rejected() {
    use crate::javascript::JsErrorKind;

    assert_eq!(
        evaluate(
            "function f(a) { \
                    a = 8; \
                    return arguments[1]; \
                } \
                f(3, 7);"
        )
        .unwrap(),
        Value::Number(7.0)
    );

    for source in [
        "function f(a) { return arguments[0]; } f(3);",
        "function f(a, a) { return arguments[0]; } f(1, 2);",
        "function f() { return arguments[2]; } f(1);",
        "function f() { return arguments['01']; } f(1);",
    ] {
        assert_eq!(
            evaluate(source).unwrap_err().kind,
            JsErrorKind::UnsupportedFeature,
            "{source}"
        );
    }
}

#[test]
fn default_and_rest_parameters_have_unmapped_indices() {
    let result = evaluate(
        r#"
                function defaults(a = 1) {
                    a = 9;
                    return arguments[0];
                }

                function rest(a, ...others) {
                    a = 9;
                    return arguments[0] + arguments[1];
                }

                defaults(3) === 3 &&
                    defaults(undefined) === undefined &&
                    rest(4, 5) === 9;
            "#,
    )
    .unwrap();

    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn escaped_arguments_and_arrow_closures_retain_unmapped_values() {
    let result = evaluate(
        r#"
                function get() {
                    return () => arguments[0];
                }

                let first = get('a');
                let second = get('b');

                first() === 'a' && second() === 'b'
            "#,
    )
    .unwrap();

    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn closures_capture_nested_function_and_catch_bindings() {
    let result = evaluate(
        r#"
            function outer() {
                let a = 10;

                try {
                    throw 2;
                } catch (x) {
                    function inner() {
                        try {
                            throw 3;
                        } catch (y) {
                            return function() {
                                return a + x + y;
                            };
                        }
                    }

                    return inner();
                }
            }

            let get = outer();
            get() === 15;
            "#,
    )
    .unwrap();

    assert_eq!(result, Value::Boolean(true));
}
