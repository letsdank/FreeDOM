//! Partial own data-property storage shared by runtime and heap objects.
//!
//! <https://tc39.es/ecma262/#sec-ordinaryownpropertykeys>
//!
//! Accessors, symbols and full property descriptors are not implemented.

#[derive(Clone, Debug, PartialEq)]
struct Property<V> {
    name: String,
    value: V,
    enumerable: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ObjectProperties<V> {
    entries: Vec<Property<V>>,
}

impl<V> Default for ObjectProperties<V> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl<V> ObjectProperties<V> {
    pub(super) fn get(&self, name: &str) -> Option<&V> {
        self.entries
            .iter()
            .find(|property| property.name == name)
            .map(|property| &property.value)
    }

    pub(super) fn set(&mut self, name: String, value: V, enumerable_if_new: bool) {
        if let Some(property) = self
            .entries
            .iter_mut()
            .find(|property| property.name == name)
        {
            property.value = value;
        } else {
            self.entries.push(Property {
                name,
                value,
                enumerable: enumerable_if_new,
            });
        }
    }

    pub(super) fn define(&mut self, name: String, value: V, enumerable: bool) {
        if let Some(property) = self
            .entries
            .iter_mut()
            .find(|property| property.name == name)
        {
            property.value = value;
            property.enumerable = enumerable;
        } else {
            self.entries.push(Property {
                name,
                value,
                enumerable,
            });
        }
    }

    pub(super) fn ordered_keys(&self) -> Vec<(String, bool)> {
        let mut indices = Vec::new();
        let mut strings = Vec::new();
        for property in &self.entries {
            let key = &property.name;
            match key.parse::<u32>() {
                Ok(index) if index != u32::MAX && index.to_string() == *key => {
                    indices.push((index, key.clone(), property.enumerable));
                }
                _ => strings.push((key.clone(), property.enumerable)),
            }
        }
        indices.sort_unstable_by_key(|(index, _, _)| *index);
        indices
            .into_iter()
            .map(|(_, key, enumerable)| (key, enumerable))
            .chain(strings)
            .collect()
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = (&str, &V)> {
        self.entries
            .iter()
            .map(|property| (property.name.as_str(), &property.value))
    }

    #[cfg(test)]
    pub(super) fn clear(&mut self) {
        self.entries.clear();
    }
}
