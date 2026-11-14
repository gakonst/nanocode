use std::{
    fmt,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

pub trait Scope {
    type Entered<'a>
    where
        Self: 'a;

    fn enter(&self) -> Self::Entered<'_>;
}

#[derive(Clone)]
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct Scoped<T, S: Scope> {
    inner: Option<T>,
    scope: S,
}

impl<T: Unpin, S: Scope> Unpin for Scoped<T, S> {}

impl<T, S: Scope> Scoped<T, S> {
    pub const fn new(inner: T, scope: S) -> Self {
        Self {
            inner: Some(inner),
            scope,
        }
    }

    pub const fn scope(&self) -> &S {
        &self.scope
    }

    pub const fn scope_mut(&mut self) -> &mut S {
        &mut self.scope
    }
}

impl<T: fmt::Debug, S: Scope + fmt::Debug> fmt::Debug for Scoped<T, S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Scoped")
            .field("inner", &self.inner)
            .field("scope", &self.scope)
            .finish()
    }
}

impl<T: Future, S: Scope> Future for Scoped<T, S> {
    type Output = T::Output;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<T::Output> {
        let this = unsafe { self.get_unchecked_mut() };
        let _entered = this.scope.enter();
        let inner = this
            .inner
            .as_mut()
            .expect("a scoped future is polled only before it is dropped");
        unsafe { Pin::new_unchecked(inner) }.poll(context)
    }
}

impl<T, S: Scope> Drop for Scoped<T, S> {
    fn drop(&mut self) {
        let _entered = self.scope.enter();
        self.inner = None;
    }
}
