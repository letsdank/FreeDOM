//! Flat binding storage used by the current interpreter.
//!
//! <https://tc39.es/ecma262/#sec-environment-records>
//!
//! This is not yet an ECMAScript Environment Record: scopes, lexical capture,
//! binding attributes and temporal dead zones are not implemented here.

use std::{
    cell::{Ref, RefCell, RefMut},
    collections::HashMap,
    rc::Rc,
};

use crate::javascript::Value;

#[derive(Default)]
pub(super) struct Bindings {
    values: HashMap<String, Rc<RefCell<Binding>>>,
}

struct Binding {
    value: Value,
}

pub(super) struct BindingSnapshot {
    name: String,
    previous: Option<Rc<RefCell<Binding>>>,
}

impl Bindings {
    pub(super) fn contains_key(&self, name: &str) -> bool {
        self.values.contains_key(name)
    }

    pub(super) fn get(&self, name: &str) -> Option<Ref<'_, Value>> {
        self.values
            .get(name)
            .map(|binding| Ref::map(binding.borrow(), |binding| &binding.value))
    }

    pub(super) fn get_mut(&mut self, name: &str) -> Option<RefMut<'_, Value>> {
        self.values
            .get_mut(name)
            .map(|binding| RefMut::map(binding.borrow_mut(), |binding| &mut binding.value))
    }

    pub(super) fn insert(&mut self, name: String, value: Value) -> Option<Value> {
        self.values
            .insert(name, Rc::new(RefCell::new(Binding { value })))
            .map(|binding| binding.borrow().value.clone())
    }

    pub(super) fn assign_existing(&mut self, name: &str, value: Value) -> Result<(), Value> {
        match self.values.get_mut(name) {
            Some(binding) => {
                binding.borrow_mut().value = value;
                Ok(())
            }

            None => Err(value),
        }
    }

    pub(super) fn insert_if_absent(&mut self, name: &str, value: Value) {
        self.values
            .entry(name.to_owned())
            .or_insert_with(|| Rc::new(RefCell::new(Binding { value })));
    }

    pub(super) fn assign_or_insert(&mut self, name: String, value: Value) {
        if let Err(value) = self.assign_existing(&name, value) {
            self.insert(name, value);
        }
    }

    pub(super) fn remove(&mut self, name: &str) -> Option<Value> {
        self.values
            .remove(name)
            .map(|binding| binding.borrow().value.clone())
    }

    pub(super) fn shadow(&mut self, name: String, value: Value) -> BindingSnapshot {
        let previous = self
            .values
            .insert(name.clone(), Rc::new(RefCell::new(Binding { value })));

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
        assert_eq!(bindings.get("x").as_deref(), Some(&Value::Number(1.0)));

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

        assert_eq!(bindings.get("x").as_deref(), Some(&Value::Number(2.0)));
        assert!(bindings.get_mut("missing").is_none());
    }

    #[test]
    fn inserts_only_when_absent() {
        let mut bindings = Bindings::default();

        bindings.insert_if_absent("x", Value::Number(1.0));
        bindings.insert_if_absent("x", Value::Number(2.0));

        assert_eq!(bindings.get("x").as_deref(), Some(&Value::Number(1.0)));
    }

    #[test]
    fn preserves_references_to_heap_values() {
        let mut bindings = Bindings::default();
        let value = Value::object_with_null_prototype();

        bindings.insert("object".into(), value.clone());

        let Value::Object(original) = value else {
            panic!("expected object");
        };

        let stored = bindings.get("object").unwrap();

        let Value::Object(stored) = &*stored else {
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
        assert_eq!(bindings.get("x").as_deref(), Some(&Value::Number(3.0)));
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
        assert_eq!(bindings.get("x").as_deref(), Some(&Value::Number(1.0)));

        bindings.restore(first);
        assert_eq!(bindings.get("x").as_deref(), Some(&Value::Undefined));
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

        let restored = bindings.get("x").unwrap();

        let Value::Object(restored) = &*restored else {
            panic!("expected object");
        };

        assert!(std::rc::Rc::ptr_eq(&original, restored));
    }

    #[test]
    fn assignment_updates_shared_cell_and_shadow_restores_its_identity() {
        let mut bindings = Bindings::default();

        bindings.insert("x".to_owned(), Value::Number(1.0));

        let original = bindings.values.get("x").unwrap().clone();

        bindings.assign_existing("x", Value::Number(2.0)).unwrap();

        assert_eq!(original.borrow().value, Value::Number(2.0));

        let snapshot = bindings.shadow("x".to_owned(), Value::Number(3.0));

        let shadow = bindings.values.get("x").unwrap().clone();

        assert!(!std::rc::Rc::ptr_eq(&original, &shadow));

        bindings.restore(snapshot);

        assert!(std::rc::Rc::ptr_eq(
            &original,
            bindings.values.get("x").unwrap()
        ));

        assert_eq!(bindings.get("x").as_deref(), Some(&Value::Number(2.0)));
    }

    #[test]
    fn loops_and_test_shim_restore_original_cells() {
        let mut interpreter = super::super::Interpreter::new();

        let program = crate::javascript::parse_program(
            "let x = 'outer'; \
            let o = Object.create(null); \
            o.a = 1;",
        )
        .unwrap();

        interpreter.execute(&program).unwrap();

        interpreter
            .variables
            .insert("this".to_owned(), Value::Number(7.0));

        let original_x = interpreter.variables.values.get("x").unwrap().clone();
        let original_this = interpreter.variables.values.get("this").unwrap().clone();

        let program = crate::javascript::parse_program(
            "for (let x in o) {} \
            for (let x of [1]) {} \
            for (let [x] of [[1]]) {} \
            test(function() {});",
        )
        .unwrap();

        interpreter.execute(&program).unwrap();

        assert!(std::rc::Rc::ptr_eq(
            &original_x,
            interpreter.variables.values.get("x").unwrap()
        ));

        assert!(std::rc::Rc::ptr_eq(
            &original_this,
            interpreter.variables.values.get("this").unwrap()
        ));

        assert_eq!(
            interpreter.variables.get("x").as_deref(),
            Some(&Value::String("outer".to_owned()))
        );
    }

    #[test]
    fn var_for_in_updates_existing_cell_instead_of_replacing_it() {
        let mut interpreter = super::super::Interpreter::new();

        let program = crate::javascript::parse_program(
            "var x = 'start'; \
            let o = Object.create(null); \
            o.a = 1;",
        )
        .unwrap();

        interpreter.execute(&program).unwrap();

        let original = interpreter.variables.values.get("x").unwrap().clone();

        let program = crate::javascript::parse_program("for (var x in o) {}").unwrap();

        interpreter.execute(&program).unwrap();

        assert!(std::rc::Rc::ptr_eq(
            &original,
            interpreter.variables.values.get("x").unwrap()
        ));

        assert_eq!(
            interpreter.variables.get("x").as_deref(),
            Some(&Value::String("a".to_owned()))
        );
    }
}
