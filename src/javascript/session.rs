//! Partial per-document classic script session.
//!
//! <https://html.spec.whatwg.org/multipage/webappapis.html#run-a-classic-script>
//!
//! This shares one interpreter across scripts; full realms and event loops
//! are incomplete.

use super::{Completion, DocumentState, Interpreter, JsError, Value, parse_program};

pub(crate) struct DocumentScriptSession {
    interpreter: Interpreter,
}

impl DocumentScriptSession {
    pub(crate) fn new(document: &DocumentState) -> Self {
        let mut interpreter = Interpreter::new();

        interpreter.set_document(document.clone());

        Self { interpreter }
    }

    /// Executes a script and updates the caller's document snapshot.
    ///
    /// The caller should not independently modify the snapshot between calls.
    pub(crate) fn evaluate(
        &mut self,
        source: &str,
        document: &mut DocumentState,
    ) -> Result<Value, JsError> {
        let program = parse_program(source)?;
        let result = self.interpreter.execute(&program);

        let Some(updated) = self.interpreter.snapshot_document() else {
            return Err(JsError::runtime(
                "TypeError: global document binding must remain a Document",
            ));
        };

        *document = updated;

        result.map(Completion::into_value)
    }
}

#[cfg(test)]
mod tests {
    use super::{DocumentScriptSession, DocumentState};
    use crate::javascript::{JsErrorKind, Value};

    #[test]
    fn shares_globals_and_document_between_scripts() {
        let mut document = DocumentState::with_title("Before");
        let mut session = DocumentScriptSession::new(&document);

        session
            .evaluate(
                "let prefix = 'Hello'; document.title = prefix;",
                &mut document,
            )
            .unwrap();

        session
            .evaluate("document.title = prefix + ' world';", &mut document)
            .unwrap();

        assert_eq!(document.title, "Hello world");
    }

    #[test]
    fn keeps_prior_effects_after_a_script_throws() {
        let mut document = DocumentState::with_title("Before");
        let mut session = DocumentScriptSession::new(&document);

        let error = session
            .evaluate(
                "let text = 'ready'; document.title = text; throw 'failed';",
                &mut document,
            )
            .unwrap_err();

        assert!(matches!(error.kind, JsErrorKind::Thrown(_)));
        assert_eq!(document.title, "ready");

        let result = session.evaluate("text;", &mut document).unwrap();

        assert_eq!(result, Value::String("ready".to_owned()));
    }

    #[test]
    fn independent_sessions_do_not_share_globals() {
        let mut first_document = DocumentState::default();
        let mut second_document = DocumentState::default();

        let mut first = DocumentScriptSession::new(&first_document);
        let mut second = DocumentScriptSession::new(&second_document);

        first
            .evaluate("let shared = 42;", &mut first_document)
            .unwrap();

        let result = second
            .evaluate("typeof shared;", &mut second_document)
            .unwrap();

        assert_eq!(result, Value::String("undefined".to_owned()));
    }
}
