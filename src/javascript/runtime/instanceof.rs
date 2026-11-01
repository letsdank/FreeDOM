//! Partial instanceof and OrdinaryHasInstance support.
//!
//! <https://tc39.es/ecma262/#sec-instanceofoperator>
//! <https://tc39.es/ecma262/#sec-ordinaryhasinstance>
//!
//! Symbol.hasInstance, bound functions and exotic prototype chains are incomplete.

use super::{Interpreter, JsError, Value};
use crate::javascript::value::ObjectState;

impl Interpreter {
    pub(super) fn evaluate_instanceof(&self, left: Value, right: Value) -> Result<Value, JsError> {
        let result = match right {
            Value::ElementConstructor(tag) => match left {
                Value::Element(node_id) => self
                    .current_document_element(node_id)
                    .map(|element| element.tag_name == tag)
                    .unwrap_or(false),

                _ => false,
            },

            Value::Function(function) => {
                Self::ordinary_has_instance(&left, function.get("prototype"))?
            }

            Value::ObjectConstructor => {
                Self::ordinary_has_instance(&left, Value::Object(self.object_prototype.clone()))?
            }

            Value::Undefined
            | Value::Null
            | Value::Boolean(_)
            | Value::Number(_)
            | Value::String(_) => {
                return Err(JsError::type_error(
                    "right-hand side of instanceof is not an object",
                ));
            }

            _ => {
                return Err(JsError::unsupported(
                    "instanceof with this right-hand side is not implemented",
                ));
            }
        };

        Ok(Value::Boolean(result))
    }

    fn ordinary_has_instance(value: &Value, prototype: Value) -> Result<bool, JsError> {
        if matches!(
            value,
            Value::Undefined
                | Value::Null
                | Value::Boolean(_)
                | Value::Number(_)
                | Value::String(_)
        ) {
            return Ok(false);
        }

        let prototype = match prototype {
            Value::Object(object) => object,

            Value::Undefined
            | Value::Null
            | Value::Boolean(_)
            | Value::Number(_)
            | Value::String(_) => {
                return Err(JsError::type_error(
                    "function has non-object prototype in instanceof",
                ));
            }

            _ => {
                return Err(JsError::unsupported(
                    "instanceof with this prototype type is not implemented",
                ));
            }
        };

        match value {
            Value::Object(object) => {
                ObjectState::has_prototype(object, &prototype).ok_or_else(|| {
                    JsError::unsupported("instanceof reached an unimplemented prototype")
                })
            }

            _ => Err(JsError::unsupported(
                "instanceof for this left-hand object is not implemented",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::javascript::value::ObjectState;
    use crate::javascript::{JsErrorKind, Value, evaluate};

    #[test]
    fn checks_new_instances_and_prototype_identity() {
        let result = evaluate(
            r#"
                function F() {}
                function G() {}

                let instance = new F();

                (instance instanceof F) &&
                    !(instance instanceof G) &&
                    !(F.prototype instanceof F) &&
                    (instance instanceof Object) &&
                    !(Object.prototype instanceof Object);
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn follows_indirect_prototypes_and_replacement() {
        let result = evaluate(
            r#"
                function F() {}

                let original = F.prototype;
                let parent = Object.create(original);
                let child = Object.create(parent);

                let before = child instanceof F;

                F.prototype = {};

                before && !(child instanceof F);
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn handles_null_prototype_and_primitive_left_values() {
        let result = evaluate(
            r#"
                function F() {}

                F.prototype = 1;

                let primitives =
                    !(null instanceof F) &&
                    !(1 instanceof F);

                let empty = Object.create(null);

                primitives && !(empty instanceof Object);
            "#,
        )
        .unwrap();

        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn rejects_non_object_function_prototypes_and_primitives_right_values() {
        for source in [
            "function F() {} F.prototype = 1; let x = {}; x instanceof F;",
            "let x = {}; x instanceof 1;",
            "let arrow = () => 1; let x = {}; x instanceof arrow;",
        ] {
            let error = evaluate(source).unwrap_err();

            assert!(
                error.message.starts_with("TypeError:"),
                "{source}: {error:?}"
            );
        }
    }

    #[test]
    fn unknown_and_exotic_prototypes_remain_explicit() {
        let Value::Object(unknown) = Value::object(std::iter::empty()) else {
            panic!("expected object");
        };

        let Value::Object(target) = Value::object_with_null_prototype() else {
            panic!("expected object");
        };

        assert_eq!(ObjectState::has_prototype(&unknown, &target), None);

        let error =
            evaluate("function F() {} F.prototype = []; let x = {}; x instanceof F;").unwrap_err();

        assert_eq!(error.kind, JsErrorKind::UnsupportedFeature);
    }
}
