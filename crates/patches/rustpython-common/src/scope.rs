use core::ops::{Deref, DerefMut};

pub struct ScopeGuard<T, F: FnOnce(T)> {
    inner: Option<(T, F)>,
}

#[must_use]
pub const fn guard<T, F: FnOnce(T)>(value: T, on_drop: F) -> ScopeGuard<T, F> {
    ScopeGuard {
        inner: Some((value, on_drop)),
    }
}

impl<T, F: FnOnce(T)> ScopeGuard<T, F> {
    pub fn into_inner(mut guard: Self) -> T {
        let (value, _) = guard
            .inner
            .take()
            .expect("a scope guard holds its value until it is dropped or taken");
        value
    }
}

impl<T, F: FnOnce(T)> Deref for ScopeGuard<T, F> {
    type Target = T;

    fn deref(&self) -> &T {
        &self
            .inner
            .as_ref()
            .expect("a scope guard holds its value until it is dropped or taken")
            .0
    }
}

impl<T, F: FnOnce(T)> DerefMut for ScopeGuard<T, F> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self
            .inner
            .as_mut()
            .expect("a scope guard holds its value until it is dropped or taken")
            .0
    }
}

impl<T, F: FnOnce(T)> Drop for ScopeGuard<T, F> {
    fn drop(&mut self) {
        if let Some((value, on_drop)) = self.inner.take() {
            on_drop(value);
        }
    }
}

#[macro_export]
macro_rules! defer {
    ($($body:tt)*) => {
        let _guard = $crate::scope::guard((), |()| {
            $($body)*
        });
    };
}
