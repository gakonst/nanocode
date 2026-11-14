use std::{
    marker::PhantomData,
    pin::Pin,
    ptr,
    sync::{
        Arc,
        atomic::{AtomicPtr, Ordering},
    },
};

pub struct AtomicBox<T> {
    pointer: AtomicPtr<T>,
    owned: PhantomData<Box<T>>,
}

unsafe impl<T: Send> Sync for AtomicBox<T> {}

impl<T> Default for AtomicBox<T> {
    fn default() -> Self {
        Self::empty()
    }
}

impl<T> AtomicBox<T> {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            pointer: AtomicPtr::new(ptr::null_mut()),
            owned: PhantomData,
        }
    }

    #[must_use]
    pub fn new(value: Option<Box<T>>) -> Self {
        Self {
            pointer: AtomicPtr::new(value.map_or(ptr::null_mut(), Box::into_raw)),
            owned: PhantomData,
        }
    }

    pub fn swap(&self, value: Option<Box<T>>) -> Option<Box<T>> {
        let raw = value.map_or(ptr::null_mut(), Box::into_raw);
        reclaim_box(self.pointer.swap(raw, Ordering::AcqRel))
    }

    pub fn take(&self) -> Option<Box<T>> {
        self.swap(None)
    }

    pub fn store(&self, value: Option<Box<T>>) {
        drop(self.swap(value));
    }
}

impl<T> Drop for AtomicBox<T> {
    fn drop(&mut self) {
        drop(reclaim_box(*self.pointer.get_mut()));
    }
}

fn reclaim_box<T>(raw: *mut T) -> Option<Box<T>> {
    if raw.is_null() {
        return None;
    }
    Some(unsafe { Box::from_raw(raw) })
}

pub struct AtomicPinBox<T> {
    inner: AtomicBox<T>,
}

impl<T> Default for AtomicPinBox<T> {
    fn default() -> Self {
        Self::empty()
    }
}

impl<T> AtomicPinBox<T> {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            inner: AtomicBox::empty(),
        }
    }

    #[must_use]
    pub fn new(value: T) -> Self {
        Self {
            inner: AtomicBox::new(Some(Box::new(value))),
        }
    }

    pub fn take(&self) -> Option<Pin<Box<T>>> {
        self.inner.take().map(Box::into_pin)
    }

    pub fn store(&self, value: Option<Pin<Box<T>>>) {
        self.inner
            .store(value.map(|value| unsafe { Pin::into_inner_unchecked(value) }));
    }
}

pub struct AtomicArc<T> {
    pointer: AtomicPtr<T>,
    owned: PhantomData<Arc<T>>,
}

impl<T> Default for AtomicArc<T> {
    fn default() -> Self {
        Self::empty()
    }
}

impl<T> AtomicArc<T> {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            pointer: AtomicPtr::new(ptr::null_mut()),
            owned: PhantomData,
        }
    }

    #[must_use]
    pub fn new(value: Option<Arc<T>>) -> Self {
        Self {
            pointer: AtomicPtr::new(arc_into_raw(value)),
            owned: PhantomData,
        }
    }

    pub fn swap(&self, value: Option<Arc<T>>) -> Option<Arc<T>> {
        reclaim_arc(self.pointer.swap(arc_into_raw(value), Ordering::AcqRel))
    }

    pub fn take(&self) -> Option<Arc<T>> {
        self.swap(None)
    }

    pub fn store(&self, value: Option<Arc<T>>) {
        drop(self.swap(value));
    }
}

impl<T> Drop for AtomicArc<T> {
    fn drop(&mut self) {
        drop(reclaim_arc(*self.pointer.get_mut()));
    }
}

fn arc_into_raw<T>(value: Option<Arc<T>>) -> *mut T {
    value.map_or(ptr::null_mut(), |value| Arc::into_raw(value).cast_mut())
}

fn reclaim_arc<T>(raw: *mut T) -> Option<Arc<T>> {
    if raw.is_null() {
        return None;
    }
    Some(unsafe { Arc::from_raw(raw) })
}

pub struct AtomicStatic<T: 'static> {
    pointer: AtomicPtr<T>,
    shared: PhantomData<Option<&'static T>>,
}

impl<T: 'static> Default for AtomicStatic<T> {
    fn default() -> Self {
        Self::empty()
    }
}

impl<T: 'static> AtomicStatic<T> {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            pointer: AtomicPtr::new(ptr::null_mut()),
            shared: PhantomData,
        }
    }

    #[must_use]
    pub fn load(&self) -> Option<&'static T> {
        let raw = self.pointer.load(Ordering::Acquire);
        unsafe { raw.as_ref() }
    }

    pub fn store(&self, value: Option<&'static T>) {
        self.pointer.store(static_into_raw(value), Ordering::Release);
    }

    pub fn compare_exchange(
        &self,
        current: Option<&'static T>,
        new: Option<&'static T>,
    ) -> Result<Option<&'static T>, Option<&'static T>> {
        self.pointer
            .compare_exchange(
                static_into_raw(current),
                static_into_raw(new),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map(|raw| unsafe { raw.as_ref() })
            .map_err(|raw| unsafe { raw.as_ref() })
    }
}

fn static_into_raw<T>(value: Option<&'static T>) -> *mut T {
    value.map_or(ptr::null_mut(), |value| ptr::from_ref(value).cast_mut())
}
