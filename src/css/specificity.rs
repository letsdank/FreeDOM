//! CSS selector specificity for the currently supported selectors.
//!
//! Specification: <https://www.w3.org/TR/selectors-4/#specificity-rules>

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
pub(super) struct Specificity {
    ids: u32,
    classes: u32,
    types: u32,
}

impl Specificity {
    #[cfg(test)]
    pub(super) const fn new(ids: u32, classes: u32, types: u32) -> Self {
        Self {
            ids,
            classes,
            types,
        }
    }

    pub(super) fn add_id(&mut self) {
        self.ids = self.ids.saturating_add(1);
    }

    pub(super) fn add_class(&mut self) {
        self.classes = self.classes.saturating_add(1);
    }

    pub(super) fn add_type(&mut self) {
        self.types = self.types.saturating_add(1);
    }
}
