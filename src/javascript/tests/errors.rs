//! Tests for failure classification at the interpreter boundary.
//!
//! These tests do not assert JavaScript exception semantics.

use crate::javascript::{JsErrorKind, evaluate, parse_json};

#[test]
fn classifies_syntax_errors_and_preserves_offsets() {
    let error = evaluate("let x = @;").unwrap_err();

    assert_eq!(error.kind, JsErrorKind::Syntax);
    assert_eq!(error.offset, Some(8));
    assert_eq!(error.to_string(), "unexpected character '@' at byte 8");
}

#[test]
fn classifies_legacy_runtime_failures_as_unclassified() {
    let error = evaluate("let n = 1; n();").unwrap_err();

    assert_eq!(error.kind, JsErrorKind::UnclassifiedRuntime);
    assert_eq!(error.message, "TypeError: this value is not callable");
    assert_eq!(error.offset, None);
}

#[test]
fn classifies_unsupported_regexp_copy_as_engine_limitation() {
    let error = evaluate("new RegExp(/a/);").unwrap_err();

    assert_eq!(error.kind, JsErrorKind::UnsupportedFeature);
    assert_eq!(
        error.message,
        "RegExp construction from another RegExp is not implemented yet"
    );
}

#[test]
fn json_lexing_errors_are_classified_as_syntax() {
    let error = parse_json("@").unwrap_err();

    assert_eq!(error.kind, JsErrorKind::Syntax);
    assert_eq!(error.offset, Some(0));
}

#[test]
fn preserves_thrown_primitive_values() {
    let error = evaluate("throw false;").unwrap_err();

    assert!(matches!(
        error.kind,
        JsErrorKind::Thrown(crate::javascript::Value::Boolean(false))
    ));
    assert_eq!(error.message, "Uncaught JavaScript exception");
}

#[test]
fn preserves_thrown_object_value() {
    let error = evaluate("throw { code: 7 };").unwrap_err();

    let JsErrorKind::Thrown(crate::javascript::Value::Object(object)) = error.kind else {
        panic!("expected a thrown object");
    };

    assert_eq!(
        object.borrow().get("code"),
        Some(&crate::javascript::Value::Number(7.0))
    );
}

#[test]
fn thrown_value_propagates_through_function_calls() {
    let mut document = crate::javascript::DocumentState::default();

    let error = crate::javascript::evaluate_with_document(
        "function f() { \
            document.title = 'before throw'; \
            throw 42; \
        } \
        f(); \
        document.title = 'after';",
        &mut document,
    )
    .unwrap_err();

    assert!(matches!(
        error.kind,
        JsErrorKind::Thrown(crate::javascript::Value::Number(42.0))
    ));
    assert_eq!(document.title, "before throw");
}

#[test]
fn throw_expression_failures_are_not_mistaken_for_explicit_throw() {
    let error = evaluate("throw missing;").unwrap_err();

    assert!(matches!(
        error.kind,
        JsErrorKind::NativeException(crate::javascript::Value::NativeError(_))
    ));
}

#[test]
fn classifies_unhandled_reference_error_as_native_exception() {
    let error = evaluate("missing;").unwrap_err();

    assert_eq!(error.message, "ReferenceError: missing is not defined");
    assert!(matches!(
        error.kind,
        JsErrorKind::NativeException(crate::javascript::Value::NativeError(_))
    ));
}
