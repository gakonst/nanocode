use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use pin_project_lite::pin_project;

use crate::span::Span;

pub trait Instrument: Sized {
    fn instrument(self, span: Span) -> Instrumented<Self> {
        Instrumented {
            inner: Some(self),
            span,
        }
    }

    fn in_current_span(self) -> Instrumented<Self> {
        self.instrument(Span::current())
    }
}

impl<T: Sized> Instrument for T {}

pin_project! {
    #[derive(Debug, Clone)]
    #[must_use = "futures do nothing unless you `.await` or poll them"]
    pub struct Instrumented<T> {
        #[pin]
        inner: Option<T>,
        span: Span,
    }

    impl<T> PinnedDrop for Instrumented<T> {
        fn drop(this: Pin<&mut Self>) {
            let mut this = this.project();
            let _enter = this.span.enter();
            this.inner.set(None);
        }
    }
}

impl<T: Future> Future for Instrumented<T> {
    type Output = T::Output;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let _enter = this.span.enter();
        this.inner
            .as_pin_mut()
            .expect("an instrumented future is polled only before it is dropped")
            .poll(context)
    }
}

impl<T> Instrumented<T> {
    pub const fn span(&self) -> &Span {
        &self.span
    }

    pub const fn span_mut(&mut self) -> &mut Span {
        &mut self.span
    }
}
