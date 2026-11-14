use nanocode_sys::{AtomicBox, AtomicWaker};
use std::{
    fmt,
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    task::{Context, Poll},
};

const VALUE_SENT: u8 = 0b001;
const RECEIVER_CLOSED: u8 = 0b010;
const SENDER_DROPPED: u8 = 0b100;

pub mod error {
    use std::fmt;

    #[derive(Debug, Eq, PartialEq, Clone)]
    pub struct RecvError(pub(super) ());

    impl fmt::Display for RecvError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("channel closed")
        }
    }

    impl std::error::Error for RecvError {}

    #[derive(Debug, Eq, PartialEq, Clone)]
    pub enum TryRecvError {
        Empty,
        Closed,
    }

    impl fmt::Display for TryRecvError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::Empty => formatter.write_str("channel empty"),
                Self::Closed => formatter.write_str("channel closed"),
            }
        }
    }

    impl std::error::Error for TryRecvError {}
}

use error::{RecvError, TryRecvError};

struct Inner<T> {
    state: AtomicU8,
    value: AtomicBox<T>,
    receiver: AtomicWaker,
    sender: AtomicWaker,
}

pub struct Sender<T> {
    inner: Option<Arc<Inner<T>>>,
}

pub struct Receiver<T> {
    inner: Option<Arc<Inner<T>>>,
    closed: Closed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Closed {
    Open,
    WithValue,
    Empty,
}

#[must_use]
pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let inner = Arc::new(Inner {
        state: AtomicU8::new(0),
        value: AtomicBox::empty(),
        receiver: AtomicWaker::new(),
        sender: AtomicWaker::new(),
    });
    (
        Sender {
            inner: Some(Arc::clone(&inner)),
        },
        Receiver {
            inner: Some(inner),
            closed: Closed::Open,
        },
    )
}

impl<T> fmt::Debug for Sender<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Sender").finish_non_exhaustive()
    }
}

impl<T> fmt::Debug for Receiver<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Receiver").finish_non_exhaustive()
    }
}

impl<T> Sender<T> {
    pub fn send(mut self, value: T) -> Result<(), T> {
        let inner = self
            .inner
            .take()
            .expect("a sender holds its channel until it sends");
        if inner.state.load(Ordering::Acquire) & RECEIVER_CLOSED != 0 {
            return Err(value);
        }
        inner.value.store(Some(Box::new(value)));
        let previous = inner.state.fetch_or(VALUE_SENT, Ordering::AcqRel);
        if previous & RECEIVER_CLOSED != 0 {
            let returned = inner
                .value
                .take()
                .expect("a receiver closed before the value never takes it");
            return Err(*returned);
        }
        inner.receiver.wake_by_ref();
        Ok(())
    }

    fn inner(&self) -> &Inner<T> {
        self.inner
            .as_ref()
            .expect("a sender holds its channel until it sends")
    }

    pub fn is_closed(&self) -> bool {
        self.inner().state.load(Ordering::Acquire) & RECEIVER_CLOSED != 0
    }

    pub fn poll_closed(&mut self, context: &mut Context<'_>) -> Poll<()> {
        let inner = self.inner();
        if inner.state.load(Ordering::Acquire) & RECEIVER_CLOSED != 0 {
            return Poll::Ready(());
        }
        inner.sender.register(context.waker());
        if inner.state.load(Ordering::Acquire) & RECEIVER_CLOSED != 0 {
            return Poll::Ready(());
        }
        Poll::Pending
    }

    pub async fn closed(&mut self) {
        std::future::poll_fn(|context| self.poll_closed(context)).await;
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take() {
            let previous = inner.state.fetch_or(SENDER_DROPPED, Ordering::AcqRel);
            if previous & RECEIVER_CLOSED == 0 {
                inner.receiver.wake_by_ref();
            }
        }
    }
}

impl<T> Receiver<T> {
    fn ready(&self, inner: &Inner<T>) -> Option<Result<T, RecvError>> {
        if self.closed == Closed::Empty {
            return Some(Err(RecvError(())));
        }
        let state = inner.state.load(Ordering::Acquire);
        if state & VALUE_SENT != 0 {
            let value = inner
                .value
                .take()
                .expect("a sent value waits for its one receiver");
            return Some(Ok(*value));
        }
        if state & SENDER_DROPPED != 0 {
            return Some(Err(RecvError(())));
        }
        None
    }

    pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
        let Some(inner) = self.inner.as_ref() else {
            return Err(TryRecvError::Closed);
        };
        match self.ready(inner) {
            Some(Ok(value)) => {
                self.inner = None;
                Ok(value)
            }
            Some(Err(_)) => {
                self.inner = None;
                Err(TryRecvError::Closed)
            }
            None => Err(TryRecvError::Empty),
        }
    }

    pub fn close(&mut self) {
        let Some(inner) = self.inner.as_ref() else {
            return;
        };
        let previous = inner.state.fetch_or(RECEIVER_CLOSED, Ordering::AcqRel);
        if self.closed == Closed::Open {
            self.closed = if previous & VALUE_SENT != 0 {
                Closed::WithValue
            } else {
                Closed::Empty
            };
        }
        if previous & (VALUE_SENT | SENDER_DROPPED) == 0 {
            inner.sender.wake_by_ref();
        }
    }

    pub const fn is_terminated(&self) -> bool {
        self.inner.is_none()
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn blocking_recv(self) -> Result<T, RecvError> {
        crate::blocking::block_on(self)
    }
}

impl<T> Future for Receiver<T> {
    type Output = Result<T, RecvError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let inner = Arc::clone(this.inner.as_ref().expect("called after complete"));
        if let Some(outcome) = this.ready(&inner) {
            this.inner = None;
            return Poll::Ready(outcome);
        }
        inner.receiver.register(context.waker());
        if let Some(outcome) = this.ready(&inner) {
            this.inner = None;
            return Poll::Ready(outcome);
        }
        Poll::Pending
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take() {
            let previous = inner.state.fetch_or(RECEIVER_CLOSED, Ordering::AcqRel);
            if previous & (VALUE_SENT | SENDER_DROPPED) == 0 {
                inner.sender.wake_by_ref();
            }
            if previous & VALUE_SENT != 0 {
                drop(inner.value.take());
            }
        }
    }
}
