use std::{
    cell::RefCell,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Origin {
    pub context: Option<Arc<str>>,
    pub session: Option<Arc<str>>,
}

impl Origin {
    pub fn scope<F: Future>(self, future: F) -> Scoped<F> {
        Scoped {
            origin: self,
            future: Box::pin(future),
        }
    }
}

thread_local! {
    static CURRENT: RefCell<Origin> = const {
        RefCell::new(Origin {
            context: None,
            session: None,
        })
    };
}

#[must_use]
pub fn current() -> Option<Arc<str>> {
    CURRENT.with_borrow(|origin| origin.context.clone())
}

#[must_use]
pub fn session() -> Option<Arc<str>> {
    CURRENT.with_borrow(|origin| origin.session.clone())
}

#[must_use]
pub fn captured() -> Origin {
    CURRENT.with_borrow(Clone::clone)
}

pub fn scope<F: Future>(origin: Option<Arc<str>>, future: F) -> Scoped<F> {
    Origin {
        context: origin,
        session: None,
    }
    .scope(future)
}

pub struct Scoped<F> {
    origin: Origin,
    future: Pin<Box<F>>,
}

struct Entered<'a> {
    origin: &'a mut Origin,
    previous: Origin,
}

impl Drop for Entered<'_> {
    fn drop(&mut self) {
        *self.origin = CURRENT.replace(std::mem::take(&mut self.previous));
    }
}

impl<F: Future> Future for Scoped<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        let this = self.get_mut();
        let previous = CURRENT.replace(std::mem::take(&mut this.origin));
        let _entered = Entered {
            origin: &mut this.origin,
            previous,
        };
        this.future.as_mut().poll(cx)
    }
}
