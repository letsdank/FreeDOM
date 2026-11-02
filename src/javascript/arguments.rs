//! Partial arguments object with immutable snapshots of unmapped indices.
//!
//! <https://tc39.es/ecma262/#sec-arguments-exotic-objects>
//!
//! Potentially mapped indices, missing-property lookup, descriptors, and
//! mutation are not implemented yet.

use std::{fmt, rc::Rc};

use super::{JsError, Value};

#[derive(Clone)]
pub struct ArgumentsValue {
    inner: Rc<ArgumentsData>,
}

struct ArgumentsData {
    values: Vec<Value>,
    potentially_mapped: usize,
}

impl ArgumentsValue {
    pub(super) fn new(values: Vec<Value>, potentially_mapped: usize) -> Self {
        Self {
            inner: Rc::new(ArgumentsData {
                values,
                potentially_mapped,
            }),
        }
    }

    pub(super) fn length(&self) -> usize {
        self.inner.values.len()
    }

    pub(super) fn get_index(&self, index: usize) -> Result<Value, JsError> {
        if index >= self.inner.values.len() {
            return Err(JsError::unsupported(
                "arguments missing indexed property lookup is not implemented",
            ));
        }

        if index < self.inner.potentially_mapped {
            return Err(JsError::unsupported(
                "potentially mapped arguments indices are not implemented",
            ));
        }

        Ok(self.inner.values[index].clone())
    }
}

impl fmt::Debug for ArgumentsValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArgumentsValue")
            .field("identity", &Rc::as_ptr(&self.inner))
            .field("length", &self.length())
            .finish()
    }
}

impl PartialEq for ArgumentsValue {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.inner, &other.inner)
    }
}
