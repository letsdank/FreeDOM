//! Identity and length for the current partial arguments object.
//!
//! <https://tc39.es/ecma262/#sec-arguments-exotic-objects>
//!
//! Indexed properties, parameter mapping, descriptors, and mutation are not
//! implemented yet.

use std::{fmt, rc::Rc};

#[derive(Clone)]
pub struct ArgumentsValue {
    identity: Rc<()>,
    length: usize,
}

impl ArgumentsValue {
    pub(super) fn new(length: usize) -> Self {
        Self {
            identity: Rc::new(()),
            length,
        }
    }

    pub(super) fn length(&self) -> usize {
        self.length
    }
}

impl fmt::Debug for ArgumentsValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArgumentsValue")
            .field("identity", &Rc::as_ptr(&self.identity))
            .field("length", &self.length)
            .finish()
    }
}

impl PartialEq for ArgumentsValue {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.identity, &other.identity)
    }
}
