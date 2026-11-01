//! Flat binding storage used by the current interpreter.
//!
//! <https://tc39.es/ecma262/#sec-environment-records>
//!
//! This is not yet an ECMAScript Environment Record: scopes, lexical capture,
//! binding attributes and temporal dead zones are not implemented here.

use std::collections::HashMap;

use crate::javascript::Value;

#[derive(Default)]
pub(super) struct Bindings {
    values: HashMap<String, Binding>,
}

struct Binding {
    value: Value,
}

pub(super) struct BindingSnapshot {
    name: String,
    previous: Option<Binding>,
}

impl Bindings {
    pub(super) fn contains_key(&self, name: &str) -> bool {
        self.values.contains_key(name)
    }

    pub(super) fn get(&self, name: &str) -> Option<&Value> {
        self.values.get(name).map(|binding| &binding.value)
    }

    pub(super) fn get_mut(&mut self, name: &str) -> Option<&mut Value> {
        self.values.get_mut(name).map(|binding| &mut binding.value)
    }

    pub(super) fn insert(&mut self, name: String, value: Value) -> Option<Value> {
        self.values
            .insert(name, Binding { value })
            .map(|binding| binding.value)
    }

    pub(super) fn assign_existing(&mut self, name: &str, value: Value) -> Result<(), Value> {
        match self.values.get_mut(name) {
            Some(binding) => {
                binding.value = value;
                Ok(())
            }

            None => Err(value),
        }
    }

    pub(super) fn insert_if_absent(&mut self, name: &str, value: Value) {
        self.values
            .entry(name.to_owned())
            .or_insert(Binding { value });
    }

    pub(super) fn remove(&mut self, name: &str) -> Option<Value> {
        self.values.remove(name).map(|binding| binding.value)
    }

    pub(super) fn shadow(&mut self, name: String, value: Value) -> BindingSnapshot {
        let previous = self.values.insert(name.clone(), Binding { value });

        BindingSnapshot { name, previous }
    }

    pub(super) fn restore(&mut self, snapshot: BindingSnapshot) {
        match snapshot.previous {
            Some(binding) => {
                self.values.insert(snapshot.name, binding);
            }

            None => {
                self.values.remove(&snapshot.name);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Bindings, Value};

    #[test]
    fn inserts_replaces_and_removes_bindings() {
        let mut bindings = Bindings::default();

        assert!(!bindings.contains_key("x"));

        assert_eq!(bindings.insert("x".into(), Value::Number(1.0)), None);
        assert_eq!(bindings.get("x"), Some(&Value::Number(1.0)));

        assert_eq!(
            bindings.insert("x".into(), Value::Undefined),
            Some(Value::Number(1.0))
        );

        assert!(bindings.contains_key("x"));
        assert_eq!(bindings.remove("x"), Some(Value::Undefined));
        assert!(!bindings.contains_key("x"));
    }

    #[test]
    fn mutates_values_without_replacing_the_binding() {
        let mut bindings = Bindings::default();

        bindings.insert("x".into(), Value::Number(1.0));
        *bindings.get_mut("x").unwrap() = Value::Number(2.0);

        assert_eq!(bindings.get("x"), Some(&Value::Number(2.0)));
        assert!(bindings.get_mut("missing").is_none());
    }

    #[test]
    fn inserts_only_when_absent() {
        let mut bindings = Bindings::default();

        bindings.insert_if_absent("x", Value::Number(1.0));
        bindings.insert_if_absent("x", Value::Number(2.0));

        assert_eq!(bindings.get("x"), Some(&Value::Number(1.0)));
    }

    #[test]
    fn preserves_references_to_heap_values() {
        let mut bindings = Bindings::default();
        let value = Value::object_with_null_prototype();

        bindings.insert("object".into(), value.clone());

        let Value::Object(original) = value else {
            panic!("expected object");
        };

        let Some(Value::Object(stored)) = bindings.get("object") else {
            panic!("expected object");
        };

        assert!(std::rc::Rc::ptr_eq(&original, stored));
    }

    #[test]
    fn assigns_only_existing_bindings_and_returns_unconsumed_values() {
        let mut bindings = Bindings::default();
        let value = Value::object_with_null_prototype();

        let returned = bindings
            .assign_existing("missing", value.clone())
            .unwrap_err();

        assert!(!bindings.contains_key("missing"));

        let (Value::Object(original), Value::Object(returned)) = (value, returned) else {
            panic!("expected object");
        };

        assert!(std::rc::Rc::ptr_eq(&original, &returned));

        bindings.insert("x".to_owned(), Value::Undefined);

        assert!(bindings.assign_existing("x", Value::Number(3.0)).is_ok());
        assert_eq!(bindings.get("x"), Some(&Value::Number(3.0)));
    }

    #[test]
    fn restores_nested_shadowed_and_absent_bindings() {
        let mut bindings = Bindings::default();

        bindings.insert("x".into(), Value::Undefined);

        let first = bindings.shadow("x".into(), Value::Number(1.0));
        let second = bindings.shadow("x".into(), Value::Number(2.0));
        let missing = bindings.shadow("temporary".into(), Value::Undefined);

        bindings.restore(missing);
        assert!(!bindings.contains_key("temporary"));

        bindings.restore(second);
        assert_eq!(bindings.get("x"), Some(&Value::Number(1.0)));

        bindings.restore(first);
        assert_eq!(bindings.get("x"), Some(&Value::Undefined));
        assert!(bindings.contains_key("x"));
    }

    #[test]
    fn shadow_restores_the_original_object_reference() {
        let mut bindings = Bindings::default();
        let original = Value::object_with_null_prototype();

        bindings.insert("x".to_owned(), original.clone());

        let snapshot = bindings.shadow("x".to_owned(), Value::Number(9.0));

        bindings.restore(snapshot);

        let Value::Object(original) = original else {
            panic!("expected object");
        };

        let Some(Value::Object(restored)) = bindings.get("x") else {
            panic!("expected object");
        };

        assert!(std::rc::Rc::ptr_eq(&original, restored));
    }
}
