use crate::Atom;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

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
