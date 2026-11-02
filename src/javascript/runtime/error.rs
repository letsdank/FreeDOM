//! Partial Error constructor and Error.prototype.toString.
//!
//! <https://tc39.es/ecma262/#sec-error-constructor>
//! <https://tc39.es/ecma262/#sec-error.prototype.tostring>
//!
//! Error causes, stack traces, subclass prototypes, and general object-to-string
//! conversion are not implemented.

use super::{Interpreter, JsError, NativeFunction, ObjectStorage, Value};
use crate::javascript::value::{ObjectState, PropertyLookup};

impl Interpreter {
    pub(super) fn construct_error(&self, arguments: Vec<Value>) -> Result<Value, JsError> {
        self.construct_error_with_prototype(arguments, self.error_prototype.clone())
    }

    pub(super) fn construct_error_with_prototype(
        &self,
        arguments: Vec<Value>,
        prototype: ObjectStorage,
    ) -> Result<Value, JsError> {
        if !matches!(
            arguments.get(1),
            None | Some(
                Value::Undefined
                    | Value::Null
                    | Value::Boolean(_)
                    | Value::Number(_)
                    | Value::String(_)
            )
        ) {
            return Err(JsError::unsupported(
                "Error options object is not implemented",
            ));
        }

        let message = match arguments.first() {
            None | Some(Value::Undefined) => None,

            Some(value) => {
                let Value::String(message) = Self::string_constructor(vec![value.clone()])? else {
                    unreachable!("string conversion returns a string");
                };

                Some(message)
            }
        };

        Ok(Value::error_with_prototype(message, prototype))
    }

    pub(super) fn error_to_string(receiver: Option<Value>) -> Result<Value, JsError> {
        let (name, message) = match receiver.unwrap_or(Value::Undefined) {
            Value::NativeError(error) => (error.get("name"), error.get("message")),

            Value::Object(object) => {
                let read = |key| match ObjectState::lookup(&object, key) {
                    PropertyLookup::Found(value) => Ok(value),

                    PropertyLookup::Absent => Ok(Value::Undefined),

                    PropertyLookup::Unknown => Err(JsError::unsupported(
                        "Error.prototype.toString encountered an unimplemented prototype",
                    )),
                };

                (read("name")?, read("message")?)
            }

            Value::Undefined
            | Value::Null
            | Value::Boolean(_)
            | Value::Number(_)
            | Value::String(_) => {
                return Err(JsError::type_error(
                    "Error.prototype.toString receiver is not an object",
                ));
            }

            _ => {
                return Err(JsError::unsupported(
                    "Error.prototype.toString receiver is not implemented",
                ));
            }
        };

        let convert = |value: Value, default: &str| -> Result<String, JsError> {
            if matches!(value, Value::Undefined) {
                return Ok(default.to_owned());
            }

            let Value::String(string) = Self::string_constructor(vec![value])? else {
                unreachable!("string conversion returns a string");
            };

            Ok(string)
        };

        let name = convert(name, "Error")?;
        let message = convert(message, "")?;

        let result = if name.is_empty() {
            message
        } else if message.is_empty() {
            name
        } else {
            format!("{name}: {message}")
        };

        Ok(Value::String(result))
    }

    pub(super) fn make_error_subtype_prototype(
        parent: &ObjectStorage,
        name: &str,
        constructor: NativeFunction,
    ) -> ObjectStorage {
        let Value::Object(prototype) = Value::object_with_prototype(parent.clone()) else {
            unreachable!("object constructor returns an object");
        };

        {
            let mut properties = prototype.borrow_mut();

            properties.define_own_data_property(
                "name".to_owned(),
                Value::String(name.to_owned()),
                false,
            );

            properties.define_own_data_property(
                "message".to_owned(),
                Value::String(String::new()),
                false,
            );

            properties.define_own_data_property(
                "constructor".to_owned(),
                Value::NativeFunction(constructor),
                false,
            );
        }

        prototype
    }

    pub(super) fn attach_native_error_prototype(&self, value: &mut Value) {
        if let Value::NativeError(error) = value {
            let prototype = match error.builtin_name() {
                "TypeError" => Some(&self.type_error_prototype),
                "ReferenceError" => Some(&self.reference_error_prototype),
                "Error" => Some(&self.error_prototype),
                _ => None,
            };

            if let Some(prototype) = prototype {
                error.attach_prototype_if_missing(prototype.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::{JsErrorKind, Value, evaluate};

    #[test]
    fn error_function_and_constructor_create_errors_with_prototype() {
        let result = evaluate(
            r#"
                let a = Error('message');
                let b = new Error();

                a.name === 'Error' &&
                    a.message === 'message' &&
                    a.hasOwnProperty('message') &&
                    b.message === '' &&
                    !b.hasOwnProperty('message') &&
                    a !== b &&
                    a instanceof Error &&
                    b instanceof Error &&
                    a instanceof Object &&
                    !(Error.prototype instanceof Error) &&
                    a.constructor === Error;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn uses_current_error_prototype_properties_and_preserves_own_message() {
        let result = evaluate(
            r#"
                let a = new Error();
                let b = Error('own');

                Error.prototype.message = 'inherited';
                Error.prototype.name = 'Custom';

                a.message === 'inherited' &&
                    b.message === 'own' &&
                    a.toString() === 'Custom: inherited' &&
                    b.toString() === 'Custom: own';
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn error_to_string_handles_missing_and_empty_properties() {
        let result = evaluate(
            r#"
                let f = Error.prototype.toString;

                f.call({}) === 'Error' &&
                    f.call({ name: '', message: 'oops' }) === 'oops' &&
                    f.call({ name: 'X', message: '' }) === 'X' &&
                    f.call({ name: 'X', message: 4 }) === 'X: 4';
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn error_prototype_and_optional_message_are_non_enumerable() {
        let result = evaluate(
            r#"
                let a = Error(undefined);
                let b = Error('');

                Object.keys(Error.prototype).length === 0 &&
                    !a.hasOwnProperty('message') &&
                    b.hasOwnProperty('message') &&
                    Object.prototype.toString.call(b) === '[object Error]' &&
                    b.stack === undefined;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn explicitly_rejects_unimplemented_features() {
        for source in ["Error({});", "Error('x', { cause: 1 });", "Error(1.5);"] {
            assert_eq!(
                evaluate(source).unwrap_err().kind,
                JsErrorKind::UnsupportedFeature,
                "{source}"
            );
        }
    }

    #[test]
    fn executes_wpt_error_prototype_setup() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/resources/testharness.js"
        ));

        let line = source
            .lines()
            .map(str::trim)
            .find(|line| *line == "AssertionError.prototype = Object.create(Error.prototype);")
            .expect("WPT prototype setup must exist");

        let script = format!(
            "function AssertionError(message) {{ \
                this.message = message; \
            }} \
            {line} \
            let e = new AssertionError('bad'); \
            e.message === 'bad' && \
                e instanceof AssertionError && \
                e instanceof Error && \
                e instanceof Object;"
        );

        assert_eq!(evaluate(&script).unwrap(), Value::Boolean(true));
    }

    #[test]
    fn native_reference_and_type_errors_have_correct_prototype_chains() {
        let result = evaluate(
            r#"
                let reference = false;
                let type = false;

                function Other() {}

                try {
                    missing;
                } catch (e) {
                    reference =
                        e instanceof ReferenceError &&
                        e instanceof Error &&
                        e instanceof Object &&
                        !(e instanceof TypeError) &&
                        !(e instanceof Other) &&
                        e.name === 'ReferenceError' &&
                        e.constructor === ReferenceError &&
                        e.hasOwnProperty('message');
                }

                try {
                    null.x;
                } catch (e) {
                    type =
                        e instanceof TypeError &&
                        e instanceof Error &&
                        !(e instanceof ReferenceError) &&
                        e.name === 'TypeError' &&
                        e.constructor === TypeError;
                }

                reference && type;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn native_error_identity_survives_rethrow_and_mutated_name() {
        let result = evaluate(
            r#"
                let saved;

                try {
                    try {
                        missing;
                    } catch (e) {
                        saved = e;
                        e.name = 'Custom';
                        throw e;
                    }
                } catch (e) {
                    saved === e &&
                        e.name === 'Custom' &&
                        e instanceof ReferenceError &&
                        e instanceof Error &&
                        !(e instanceof TypeError);
                }
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn constructors_create_independent_error_subtypes() {
        let result = evaluate(
            r#"
                let a = TypeError('bad');
                let b = new ReferenceError('missing');

                a instanceof TypeError &&
                    a instanceof Error &&
                    !(a instanceof ReferenceError) &&
                    a.message === 'bad' &&
                    b instanceof ReferenceError &&
                    b instanceof Error &&
                    !(b instanceof TypeError) &&
                    b.message === 'missing' &&
                    TypeError.prototype instanceof Error &&
                    ReferenceError.prototype instanceof Error &&
                    Object.keys(TypeError.prototype).length === 0;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn native_errors_use_realm_prototypes_even_if_globals_change() {
        let result = evaluate(
            r#"
                let constructor = ReferenceError;
                ReferenceError = 1;

                let result = false;

                try {
                    missing;
                } catch (e) {
                    result =
                        e instanceof constructor &&
                        e instanceof Error &&
                        e.constructor === constructor;
                }

                result;
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn promise_reactions_bind_native_exception_prototypes() {
        use crate::javascript::{DocumentScriptSession, DocumentState};

        let mut document = DocumentState::with_title("");
        let mut session = DocumentScriptSession::new(&document);

        session
            .evaluate(
                r#"
                let callbackError = false;
                let cycleError = false;

                Promise.resolve(1)
                    .then(Error.prototype.toString)
                    .then(undefined, function(e) {
                        callbackError =
                            e instanceof TypeError &&
                            e instanceof Error;
                    });

                let p;

                p = Promise.resolve().then(function() {
                    return p;
                });

                p.then(undefined, function(e) {
                    cycleError =
                        e instanceof TypeError &&
                        e instanceof Error;
                });
            "#,
                &mut document,
            )
            .unwrap();

        assert_eq!(
            session
                .evaluate("callbackError && cycleError;", &mut document)
                .unwrap(),
            Value::Boolean(true)
        );
    }
}
