use std::sync::Arc;

use crossbeam_utils::atomic::AtomicCell;

pub trait Pointer: Sized {
    type Base;

    fn into_slot(self) -> Option<Arc<Self::Base>>;

    fn from_slot(slot: Option<Arc<Self::Base>>) -> Self;

    fn as_ptr(&self) -> *const Self::Base;
}

impl<T> Pointer for Arc<T> {
    type Base = T;

    fn into_slot(self) -> Option<Self> {
        Some(self)
    }

    fn from_slot(slot: Option<Self>) -> Self {
        slot.expect("a swapped value stays present between loads")
    }

    fn as_ptr(&self) -> *const T {
        Self::as_ptr(self)
    }
}

impl<T> Pointer for Option<Arc<T>> {
    type Base = T;

    fn into_slot(self) -> Self {
        self
    }

    fn from_slot(slot: Self) -> Self {
        slot
    }

    fn as_ptr(&self) -> *const T {
        self.as_ref().map_or(std::ptr::null(), Arc::as_ptr)
    }
}

pub struct Swapped<P: Pointer>(AtomicCell<Option<Arc<P::Base>>>);

pub type Swap<T> = Swapped<Arc<T>>;

pub type SwapOption<T> = Swapped<Option<Arc<T>>>;

pub struct Guard<P>(P);

impl<P> Guard<P> {
    pub fn into_inner(guard: Self) -> P {
        guard.0
    }
}

impl<P> std::ops::Deref for Guard<P> {
    type Target = P;

    fn deref(&self) -> &P {
        &self.0
    }
}

impl<P: std::fmt::Debug> std::fmt::Debug for Guard<P> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl<P: Pointer> Swapped<P> {
    pub fn new(value: P) -> Self {
        Self(AtomicCell::new(value.into_slot()))
    }

    pub fn load(&self) -> Guard<P> {
        Guard(self.load_full())
    }

    pub fn load_full(&self) -> P {
        let held = self.0.take();
        self.0.store(held.clone());
        P::from_slot(held)
    }

    pub fn store(&self, value: P) {
        drop(self.swap(value));
    }

    pub fn swap(&self, value: P) -> P {
        P::from_slot(self.0.swap(value.into_slot()))
    }

    pub fn compare_and_swap<N: Into<P>>(&self, current: &P, new: N) -> Guard<P> {
        let previous = self.load_full();
        if previous.as_ptr() == current.as_ptr() {
            self.store(new.into());
        }
        Guard(previous)
    }

    pub fn rcu<R, F>(&self, mut change: F) -> P
    where
        F: FnMut(&P) -> R,
        R: Into<P>,
    {
        let mut current = self.load_full();
        loop {
            let previous = Guard::into_inner(self.compare_and_swap(&current, change(&current)));
            if previous.as_ptr() == current.as_ptr() {
                return previous;
            }
            current = previous;
        }
    }
}

impl<T> Swap<T> {
    pub fn from_pointee(value: T) -> Self {
        Self::new(Arc::new(value))
    }
}

pub struct Enclosed<T>(Swap<T>);

impl<T> Enclosed<T> {
    pub fn from_pointee(value: T) -> Self {
        Self(Swap::from_pointee(value))
    }

    pub fn load_full(&self) -> Arc<T> {
        self.0.load_full()
    }

    pub fn with<R>(&self, read: impl FnOnce(&T) -> R) -> R {
        read(&self.0.load())
    }

    pub fn swap(&self, value: Arc<T>) -> Arc<T> {
        self.0.swap(value)
    }

    pub fn compare_and_swap(&self, current: &Arc<T>, new: Arc<T>) -> Arc<T> {
        Guard::into_inner(self.0.compare_and_swap(current, new))
    }
}

impl<T> SwapOption<T> {
    pub const fn const_empty() -> Self {
        Self(AtomicCell::new(None))
    }

    pub const fn empty() -> Self {
        Self::const_empty()
    }

    pub fn from_pointee(value: impl Into<Option<T>>) -> Self {
        Self::new(value.into().map(Arc::new))
    }
}

impl<P: Pointer + Default> Default for Swapped<P> {
    fn default() -> Self {
        Self::new(P::default())
    }
}

impl<P: Pointer> From<P> for Swapped<P> {
    fn from(value: P) -> Self {
        Self::new(value)
    }
}

impl<P: Pointer + std::fmt::Debug> std::fmt::Debug for Swapped<P> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("Swap")
            .field(&self.load_full())
            .finish()
    }
}
