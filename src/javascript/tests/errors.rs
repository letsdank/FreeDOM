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
    let error = evaluate("missing;").unwrap_err();

    assert_eq!(error.kind, JsErrorKind::UnclassifiedRuntime);
    assert_eq!(error.message, "ReferenceError: missing is not defined");
    assert_eq!(error.offset, None);
}

#[test]
fn classifies_unsupported_try_as_engine_limitation() {
    let error = evaluate("try {} finally {}").unwrap_err();

    assert_eq!(error.kind, JsErrorKind::UnsupportedFeature);
    assert_eq!(
        error.message,
        "try/catch/finally execution is not implemented yet"
    );
}

#[test]
fn json_lexing_errors_are_classified_as_syntax() {
    let error = parse_json("@").unwrap_err();

    assert_eq!(error.kind, JsErrorKind::Syntax);
    assert_eq!(error.offset, Some(0));
}
