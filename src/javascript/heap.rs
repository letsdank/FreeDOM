//! Generational storage for the future JavaScript heap.
//!
//! ECMAScript values and objects:
//! <https://tc39.es/ecma262/#sec-ecmascript-data-types-and-values>
//!
//! This is storage only: tracing, collection and runtime integration are not
//! implemented. An object may be removed only when no live root can reach it.

use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_HEAP_ID: AtomicUsize = AtomicUsize::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct ObjectId {
    heap: usize,
    index: usize,
    generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HeapError {
    IdentityExhausted,
}

struct Slot<T> {
    generation: u64,
    value: Option<T>,
}

pub(super) struct Heap<T> {
    id: usize,
    slots: Vec<Slot<T>>,
    free: Vec<usize>,
}

impl<T> Heap<T> {
    pub(super) fn new() -> Result<Self, HeapError> {
        let mut candidate = NEXT_HEAP_ID.load(Ordering::Relaxed);

        let id = loop {
            let next = candidate
                .checked_add(1)
                .ok_or(HeapError::IdentityExhausted)?;

            match NEXT_HEAP_ID.compare_exchange_weak(
                candidate,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break candidate,
                Err(actual) => candidate = actual,
            }
        };

        Ok(Self {
            id,
            slots: Vec::new(),
            free: Vec::new(),
        })
    }

    pub(super) fn allocate(&mut self, value: T) -> ObjectId {
        let index = match self.free.pop() {
            Some(index) => {
                self.slots[index].value = Some(value);
                index
            }

            None => {
                let index = self.slots.len();

                self.slots.push(Slot {
                    generation: 0,
                    value: Some(value),
                });

                index
            }
        };

        ObjectId {
            heap: self.id,
            index,
            generation: self.slots[index].generation,
        }
    }

    pub(super) fn get(&self, id: ObjectId) -> Option<&T> {
        if id.heap != self.id {
            return None;
        }

        let slot = self.slots.get(id.index)?;

        if slot.generation != id.generation {
            return None;
        }

        slot.value.as_ref()
    }

    pub(super) fn get_mut(&mut self, id: ObjectId) -> Option<&mut T> {
        if id.heap != self.id {
            return None;
        }

        let slot = self.slots.get_mut(id.index)?;

        if slot.generation != id.generation {
            return None;
        }

        slot.value.as_mut()
    }

    pub(super) fn remove(&mut self, id: ObjectId) -> Option<T> {
        if id.heap != self.id {
            return None;
        }

        let slot = self.slots.get_mut(id.index)?;

        if slot.generation != id.generation {
            return None;
        }

        let value = slot.value.take()?;

        if let Some(next) = slot.generation.checked_add(1) {
            slot.generation = next;
            self.free.push(id.index);
        }

        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::Heap;

    #[test]
    fn rejects_foreign_and_stale_handles_after_reuse() {
        let mut first = Heap::new().unwrap();
        let mut second = Heap::new().unwrap();

        let old = first.allocate(10);
        let other = second.allocate(20);

        assert_eq!(first.get(old), Some(&10));
        assert!(first.get(other).is_none());
        assert!(first.get_mut(other).is_none());
        assert!(first.remove(other).is_none());

        *first.get_mut(old).unwrap() = 11;
        assert_eq!(first.remove(old), Some(11));

        let replacement = first.allocate(30);

        assert_eq!(old.index, replacement.index);
        assert_ne!(old.generation, replacement.generation);
        assert_eq!(first.get(replacement), Some(&30));

        assert!(first.get(old).is_none());
        assert!(first.get_mut(old).is_none());
        assert!(first.remove(old).is_none());
        assert_eq!(second.get(other), Some(&20));
    }

    #[test]
    fn retires_slots_instead_of_wrapping_generation() {
        let mut heap = Heap::new().unwrap();
        let first = heap.allocate(1);

        heap.slots[first.index].generation = u64::MAX;

        let last = super::ObjectId {
            generation: u64::MAX,
            ..first
        };

        assert_eq!(heap.remove(last), Some(1));
        assert!(heap.get(last).is_none());

        let next = heap.allocate(2);

        assert_ne!(next.index, last.index);
        assert!(heap.get(last).is_none());
        assert_eq!(heap.get(next), Some(&2));
    }
}
