use std::{
    cell::Cell,
    future::poll_fn,
    task::{Context, Poll},
};

const INITIAL: u8 = 128;

thread_local! {
    static LEFT: Cell<Option<u8>> = const { Cell::new(None) };
}

pub(crate) struct Budget(Option<u8>);

impl Budget {
    pub(crate) fn enter() -> Self {
        Self(LEFT.with(|left| left.replace(Some(INITIAL))))
    }
}

impl Drop for Budget {
    fn drop(&mut self) {
        LEFT.with(|left| left.set(self.0));
    }
}

#[must_use]
pub fn has_budget_remaining() -> bool {
    LEFT.with(Cell::get).is_none_or(|left| left > 0)
}

pub fn poll_proceed(context: &mut Context<'_>) -> Poll<()> {
    LEFT.with(|left| match left.get() {
        None => Poll::Ready(()),
        Some(0) => {
            context.waker().wake_by_ref();
            Poll::Pending
        }
        Some(remaining) => {
            left.set(Some(remaining - 1));
            Poll::Ready(())
        }
    })
}

pub(crate) fn poll_budget(context: &mut Context<'_>) -> Poll<()> {
    if has_budget_remaining() {
        return Poll::Ready(());
    }
    context.waker().wake_by_ref();
    Poll::Pending
}

pub(crate) fn spend() {
    LEFT.with(|left| {
        if let Some(remaining) = left.get() {
            left.set(Some(remaining.saturating_sub(1)));
        }
    });
}

pub async fn consume_budget() {
    poll_fn(poll_proceed).await;
}
