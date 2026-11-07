use crate::{Atom, Slot};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

pub struct Shelf<T> {
    slots: Box<[Slot<T>]>,
    latest: AtomicUsize,
}

impl<T> Shelf<T> {
    #[must_use]
    pub fn new(slots: usize) -> Self {
        Self {
            slots: (0..slots).map(|_| Slot::default()).collect(),
            latest: AtomicUsize::new(0),
        }
    }

    pub fn put(&self, value: T) -> Option<T> {
        let mut carried = value;
        for (index, slot) in self.slots.iter().enumerate() {
            if !slot.is_empty() {
                continue;
            }
            match slot.put(carried) {
                None => {
                    self.latest.store(index, Ordering::Release);
                    return None;
                }
                Some(displaced) => carried = displaced,
            }
        }
        Some(carried)
    }

    pub fn take(&self) -> Option<T> {
        let count = self.slots.len();
        let latest = self.latest.load(Ordering::Acquire);
        (0..count).find_map(|offset| self.slots[(latest + offset) % count].take())
    }

    pub fn take_all(&self) -> Vec<T> {
        self.slots.iter().filter_map(Slot::take).collect()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.iter().filter(|slot| !slot.is_empty()).count()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(Slot::is_empty)
    }
}

pub struct Armed<T: ?Sized> {
    armed: AtomicBool,
    value: Atom<Option<Arc<T>>>,
}

impl<T: ?Sized> Default for Armed<T> {
    fn default() -> Self {
        Self {
            armed: AtomicBool::new(false),
            value: Atom::new(None),
        }
    }
}

impl<T: ?Sized> Armed<T> {
    pub fn arm(&self, value: Arc<T>) {
        self.value.set(Some(value));
        self.armed.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn get(&self) -> Option<Arc<T>> {
        if !self.armed.load(Ordering::Acquire) {
            return None;
        }
        (*self.value.get()).clone()
    }
}
