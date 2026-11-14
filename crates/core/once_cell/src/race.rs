use std::{fmt, ptr, sync::OnceLock};

pub struct OnceBox<T> {
    cell: OnceLock<Box<T>>,
}

impl<T> OnceBox<T> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            cell: OnceLock::new(),
        }
    }

    #[must_use]
    pub fn get(&self) -> Option<&T> {
        self.cell.get().map(Box::as_ref)
    }

    pub fn set(&self, value: Box<T>) -> Result<(), Box<T>> {
        self.cell.set(value)
    }

    pub fn get_or_init<F>(&self, init: F) -> &T
    where
        F: FnOnce() -> Box<T>,
    {
        self.cell.get_or_init(init)
    }
}

impl<T> Default for OnceBox<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> fmt::Debug for OnceBox<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let address = self.get().map_or(ptr::null(), ptr::from_ref);
        write!(formatter, "OnceBox({address:?})")
    }
}
