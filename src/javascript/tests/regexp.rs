//! Integration tests for RegExp in the JavaScript interpreter.
//!
//! These local tests do not execute the Test262 suite.

use crate::javascript::evaluate;

#[test]
fn constructs_and_uses_regexp_objects() {
    evaluate(
        r#"
            const re = new RegExp("a", "g");
            const alias = re;

            assert_equals(typeof re, "object");
            assert_equals(typeof re.test, "function");

            assert_true(re === alias);
            assert_false(re === new RegExp("a", "g"));

            assert_true(re.test("aba"));
            assert_equals(alias.lastIndex, 1);

            assert_true(alias.test("aba"));
            assert_equals(re.lastIndex, 3);

            assert_false(re.test("aba"));
            assert_equals(re.lastIndex, 0);
        "#,
    )
    .unwrap();
}

#[test]
fn regexp_constructor_defaults_and_test_default_argument() {
    evaluate(
        r#"
            const empty = new RegExp();
            assert_true(empty.test("text"));

            const re = new RegExp("undefined");
            assert_true(re.test());

            const implicit = new RegExp(undefined, undefined);
            assert_true(implicit.test("x"));
        "#,
    )
    .unwrap();
}

#[test]
fn regexp_constructor_evaluate_extra_arguments() {
    evaluate(
        r#"
            let count = 0;
            const re = new RegExp("a", "", count = 1);

            assert_equals(count, 1);
            assert_true(re.test("a"));
        "#,
    )
    .unwrap();
}

#[test]
fn regexp_last_index_preserves_assigned_values_until_matching() {
    evaluate(
        r#"
            const re = new RegExp("a", "g");
            const alias = re;

            re.lastIndex = 1.9;
            assert_equals(alias.lastIndex, 1.9);

            assert_true(alias.test("aba"));
            assert_equals(re.lastIndex, 3);

            re.lastIndex = 0 - 4;
            assert_true(re.test("aba"));
            assert_equals(alias.lastIndex, 1);

            re.lastIndex = true;
            assert_true(re.test("aba"));
            assert_equals(re.lastIndex, 3);

            re.lastIndex = null;
            assert_true(re.test("aba"));
            assert_equals(re.lastIndex, 1);

            re.lastIndex = undefined;
            assert_true(re.test("aba"));
            assert_equals(re.lastIndex, 1);
        "#,
    )
    .unwrap();
}

#[test]
fn regexp_stateless_test_preserves_original_last_index() {
    evaluate(
        r#"
            const re = new RegExp("a");
            re.lastIndex = 4.5;

            assert_true(re.test("a"));
            assert_equals(re.lastIndex, 4.5);

            assert_false(re.test("b"));
            assert_equals(re.lastIndex, 4.5);
        "#,
    )
    .unwrap();
}

#[test]
fn regexp_last_index_coerces_strings_at_match_time() {
    evaluate(
        r#"
            const re = new RegExp("a", "g");

            re.lastIndex = "2";
            assert_equals(re.lastIndex, "2");

            assert_true(re.test("aba"));
            assert_equals(re.lastIndex, 3);
        "#,
    )
    .unwrap();
}

#[test]
fn indexed_regexp_properties_share_state() {
    evaluate(
        r#"
            const re = new RegExp("a", "g");
            const key = "lastIndex";

            re[key] = "0x2";
            assert_equals(re.lastIndex, "0x2");

            assert_true(re["test"]("aba"));
            assert_equals(re[key], 3);

            re.lastIndex = "not-a-number";
            assert_true(re.test("aba"));
            assert_equals(re[key], 1);
        "#,
    )
    .unwrap();
}

#[test]
fn regexp_exec_returns_captures_properties_and_named_indices() {
    evaluate(
        r#"
            const re = new RegExp("(?<letter>a)(b)?", "dg");

            assert_equals(typeof re.exec, "function");

            const result = re.exec("zab");

            assert_equals(result.length, 3);
            assert_equals(result[0], "ab");
            assert_equals(result[1], "a");
            assert_equals(result[2], "b");

            assert_equals(result.index, 1);
            assert_equals(result.input, "zab");
            assert_equals(result.groups.letter, "a");

            assert_equals(result.indices[0][0], 1);
            assert_equals(result.indices[0][1], 3);
            assert_equals(result.indices[1][0], 1);
            assert_equals(result.indices[1][1], 2);

            assert_equals(result.indices.groups.letter[0], 1);
            assert_equals(re.lastIndex, 3);

            assert_equals(re["exec"]("zab"), null);
            assert_equals(re.lastIndex, 0);
        "#,
    )
    .unwrap();
}

#[test]
fn regexp_exec_keeps_unmatched_groups_and_absent_indices_undefined() {
    evaluate(
        r#"
            const re = new RegExp("(?<x>a)?b", "d");
            const result = re.exec("b");

            assert_equals(result[1], undefined);
            assert_equals(result.groups.x, undefined);
            assert_equals(result.indices[1], undefined);
            assert_equals(result.indices.groups.x, undefined);

            const plain = new RegExp("(a)").exec("a");

            assert_equals(plain.groups, undefined);
            assert_equals(plain.indices, undefined);

            const indexed = new RegExp("(a)", "d").exec("😀a");

            assert_equals(indexed.index, 2);
            assert_equals(indexed.indices[1][0], 2);
            assert_equals(indexed.indices[1][1], 3);
            assert_equals(indexed.indices.groups, undefined);
        "#,
    )
    .unwrap();
}

#[test]
fn regexp_exec_rejects_unpaired_surrogates_without_lossy_conversion() {
    let error = evaluate(r#"new RegExp(".", "g").exec("😀");"#).unwrap_err();

    assert!(error.message.contains("unpaired UTF-16 surrogate"))
}

#[test]
fn regexp_indices_named_group_reuses_the_same_array() {
    evaluate(
        r#"
            const result = new RegExp("(?<x>a)", "d").exec("a");

            assert_true(
                result.indices.groups.x === result.indices[1]
            );

            assert_false(
                result.indices.groups.x === result.indices[0]
            );

            assert_not_equals(
                result.indices[0],
                result.indices[1]
            );
        "#,
    )
    .unwrap();
}

#[test]
fn evaluates_regexp_literals_and_division_in_the_same_program() {
    let result = crate::javascript::evaluate(
        "let value = 8 / 2; \
        let matches = /b/i.test('ABC'); \
        matches && value === 4;",
    )
    .unwrap();

    assert_eq!(result, crate::javascript::Value::Boolean(true));
}

#[test]
fn regexp_literal_creates_fresh_object_each_time() {
    let result = crate::javascript::evaluate(
        "const make = function() { return /a/g; }; \
        let a = make(); \
        let b = make(); \
        a.test('a'); \
        b.lastIndex;",
    )
    .unwrap();

    assert_eq!(result, crate::javascript::Value::Number(0.0));
}

#[test]
fn regexp_literal_invalid_flags_are_checked_before_execution() {
    let error = crate::javascript::evaluate("if (false) { /a/gg; }").unwrap_err();

    assert!(error.message.contains("invalid RegExp flags"));
}

#[test]
fn parses_first_regexp_literal_from_wpt_harness() {
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/wpt/resources/testharness.js"
    ));

    let fragment = r"/^\(\)\s*=>\s*(?:{(.*)}\s*|(.*))$/";

    assert!(source.contains(fragment));

    let program = crate::javascript::parser::parse_program(&format!("{fragment};")).unwrap();

    assert!(matches!(
        program.as_slice(),
        [crate::javascript::Statement::Expression(
            crate::javascript::Expression::RegExpLiteral { .. }
        )]
    ));
}
