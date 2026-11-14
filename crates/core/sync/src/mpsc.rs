use crate::{Semaphore, semaphore::Acquire, waiters::Waiters};
use nanocode_sys::{
    AtomicWaker,
    queue::{Consumer, Pop, Queue},
};
use std::{
    fmt,
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll},
};

const RECEIVER_CLOSED: usize = 1;
const MESSAGE: usize = 2;

pub mod error {
    use std::fmt;

    #[derive(PartialEq, Eq, Clone, Copy)]
    pub struct SendError<T>(pub T);

    impl<T> fmt::Debug for SendError<T> {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.debug_struct("SendError").finish_non_exhaustive()
        }
    }

    impl<T> fmt::Display for SendError<T> {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("channel closed")
        }
    }

    impl<T> std::error::Error for SendError<T> {}

    #[derive(PartialEq, Eq, Clone, Copy)]
    pub enum TrySendError<T> {
        Full(T),
        Closed(T),
    }

    impl<T> TrySendError<T> {
        pub fn into_inner(self) -> T {
            match self {
                Self::Full(value) | Self::Closed(value) => value,
            }
        }
    }

    impl<T> fmt::Debug for TrySendError<T> {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::Full(..) => fmt::Debug::fmt("Full(..)", formatter),
                Self::Closed(..) => fmt::Debug::fmt("Closed(..)", formatter),
            }
        }
    }

    impl<T> fmt::Display for TrySendError<T> {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::Full(..) => formatter.write_str("no available capacity"),
                Self::Closed(..) => formatter.write_str("channel closed"),
            }
        }
    }

    impl<T> std::error::Error for TrySendError<T> {}

    impl<T> From<SendError<T>> for TrySendError<T> {
        fn from(error: SendError<T>) -> Self {
            Self::Closed(error.0)
        }
    }

    #[derive(PartialEq, Eq, Clone, Copy, Debug)]
    pub enum TryRecvError {
        Empty,
        Disconnected,
    }

    impl fmt::Display for TryRecvError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::Empty => formatter.write_str("receiving on an empty channel"),
                Self::Disconnected => formatter.write_str("receiving on a closed channel"),
            }
        }
    }

    impl std::error::Error for TryRecvError {}
}

use error::{SendError, TryRecvError, TrySendError};

struct Chan<T> {
    queue: Queue<T>,
    messages: AtomicUsize,
    senders: AtomicUsize,
    weak: AtomicUsize,
    receiver: AtomicWaker,
    closed: Waiters,
    bound: Option<(Semaphore, usize)>,
}

impl<T> Chan<T> {
    fn new(bound: Option<usize>) -> Arc<Self> {
        Arc::new(Self {
            queue: Queue::new(),
            messages: AtomicUsize::new(0),
            senders: AtomicUsize::new(1),
            weak: AtomicUsize::new(0),
            receiver: AtomicWaker::new(),
            closed: Waiters::default(),
            bound: bound.map(|capacity| (Semaphore::new(capacity), capacity)),
        })
    }

    fn is_closed(&self) -> bool {
        self.messages.load(Ordering::Acquire) & RECEIVER_CLOSED != 0
    }

    fn push(&self, value: T) {
        self.messages.fetch_add(MESSAGE, Ordering::AcqRel);
        self.queue.push(value);
        self.receiver.wake();
    }

    fn push_open(&self, value: T) -> Result<(), SendError<T>> {
        let mut current = self.messages.load(Ordering::Acquire);
        loop {
            if current & RECEIVER_CLOSED != 0 {
                return Err(SendError(value));
            }
            match self.messages.compare_exchange_weak(
                current,
                current + MESSAGE,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
        self.queue.push(value);
        self.receiver.wake();
        Ok(())
    }

    fn unreserve(&self, permits: usize) {
        if let Some((semaphore, _)) = &self.bound {
            semaphore.release(permits);
        }
        if self.is_closed() {
            self.receiver.wake();
        }
    }

    fn idle(&self) -> bool {
        self.bound.as_ref().is_none_or(|(semaphore, capacity)| {
            semaphore.available_permits() + self.queued() >= *capacity
        })
    }

    fn queued(&self) -> usize {
        self.messages.load(Ordering::Acquire) / MESSAGE
    }

    fn finished(&self) -> bool {
        let messages = self.messages.load(Ordering::Acquire);
        if messages / MESSAGE != 0 {
            return false;
        }
        if self.senders.load(Ordering::Acquire) == 0 {
            return true;
        }
        messages & RECEIVER_CLOSED != 0 && self.idle()
    }

    fn close(&self) {
        let previous = self.messages.fetch_or(RECEIVER_CLOSED, Ordering::AcqRel);
        if let Some((semaphore, _)) = &self.bound {
            semaphore.close();
        }
        if previous & RECEIVER_CLOSED == 0 {
            self.closed.wake_all();
        }
    }

    fn add_sender(&self) {
        self.senders.fetch_add(1, Ordering::AcqRel);
    }

    fn drop_sender(&self) {
        if self.senders.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.receiver.wake();
        }
    }

    fn upgrade(&self) -> bool {
        let mut current = self.senders.load(Ordering::Acquire);
        loop {
            if current == 0 {
                return false;
            }
            match self.senders.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(actual) => current = actual,
            }
        }
    }

    fn poll_closed(&self, ticket: &mut Option<u64>, context: &mut Context<'_>) -> Poll<()> {
        if self.is_closed() {
            return Poll::Ready(());
        }
        *ticket = Some(self.closed.register(*ticket, context.waker()));
        if self.is_closed() {
            return Poll::Ready(());
        }
        Poll::Pending
    }
}

struct ClosedWait<'a, T> {
    chan: &'a Chan<T>,
    ticket: Option<u64>,
}

impl<T> Future for ClosedWait<'_, T> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        this.chan.poll_closed(&mut this.ticket, context)
    }
}

impl<T> Drop for ClosedWait<'_, T> {
    fn drop(&mut self) {
        if let Some(ticket) = self.ticket {
            self.chan.closed.remove(ticket);
        }
    }
}

enum Popped<T> {
    Value(T),
    Empty,
    Pushing,
}

struct Rx<T> {
    chan: Arc<Chan<T>>,
    consumer: Consumer,
}

impl<T> Rx<T> {
    fn new(chan: Arc<Chan<T>>) -> Self {
        let consumer = chan
            .queue
            .consumer()
            .expect("a new channel has no receiver yet");
        Self { chan, consumer }
    }

    fn pop(&mut self) -> Popped<T> {
        match self.chan.queue.pop(&mut self.consumer) {
            Pop::Value(value) => {
                self.chan.messages.fetch_sub(MESSAGE, Ordering::AcqRel);
                if let Some((semaphore, _)) = &self.chan.bound {
                    semaphore.release(1);
                }
                Popped::Value(value)
            }
            Pop::Empty => Popped::Empty,
            Pop::Pushing => Popped::Pushing,
        }
    }

    fn pop_settled(&mut self) -> Option<T> {
        loop {
            match self.pop() {
                Popped::Value(value) => return Some(value),
                Popped::Empty => return None,
                Popped::Pushing => std::thread::yield_now(),
            }
        }
    }

    fn poll_recv(&mut self, context: &mut Context<'_>) -> Poll<Option<T>> {
        if let Popped::Value(value) = self.pop() {
            return Poll::Ready(Some(value));
        }
        self.chan.receiver.register(context.waker());
        match self.pop() {
            Popped::Value(value) => Poll::Ready(Some(value)),
            Popped::Pushing => Poll::Pending,
            Popped::Empty if self.chan.finished() => Poll::Ready(None),
            Popped::Empty => Poll::Pending,
        }
    }

    fn poll_recv_many(
        &mut self,
        context: &mut Context<'_>,
        buffer: &mut Vec<T>,
        limit: usize,
    ) -> Poll<usize> {
        if limit == 0 {
            return Poll::Ready(0);
        }
        let Poll::Ready(first) = self.poll_recv(context) else {
            return Poll::Pending;
        };
        let Some(first) = first else {
            return Poll::Ready(0);
        };
        buffer.push(first);
        let mut taken = 1;
        while taken < limit {
            let Popped::Value(value) = self.pop() else {
                break;
            };
            buffer.push(value);
            taken += 1;
        }
        Poll::Ready(taken)
    }

    fn try_recv(&mut self) -> Result<T, TryRecvError> {
        match self.pop_settled() {
            Some(value) => Ok(value),
            None if self.chan.finished() => Err(TryRecvError::Disconnected),
            None => Err(TryRecvError::Empty),
        }
    }

    fn close(&self) {
        self.chan.close();
    }

    fn is_closed(&self) -> bool {
        self.chan.is_closed() || self.chan.senders.load(Ordering::Acquire) == 0
    }
}

impl<T> Drop for Rx<T> {
    fn drop(&mut self) {
        self.chan.close();
        while self.pop_settled().is_some() {}
    }
}

pub struct Sender<T> {
    chan: Arc<Chan<T>>,
}

pub struct Receiver<T> {
    rx: Rx<T>,
}

pub struct WeakSender<T> {
    chan: Arc<Chan<T>>,
}

pub struct UnboundedSender<T> {
    chan: Arc<Chan<T>>,
}

pub struct UnboundedReceiver<T> {
    rx: Rx<T>,
}

pub struct WeakUnboundedSender<T> {
    chan: Arc<Chan<T>>,
}

#[must_use]
pub fn channel<T>(buffer: usize) -> (Sender<T>, Receiver<T>) {
    assert!(buffer > 0, "mpsc bounded channel requires buffer > 0");
    let chan = Chan::new(Some(buffer));
    (
        Sender {
            chan: Arc::clone(&chan),
        },
        Receiver { rx: Rx::new(chan) },
    )
}

#[must_use]
pub fn unbounded_channel<T>() -> (UnboundedSender<T>, UnboundedReceiver<T>) {
    let chan = Chan::new(None);
    (
        UnboundedSender {
            chan: Arc::clone(&chan),
        },
        UnboundedReceiver { rx: Rx::new(chan) },
    )
}

macro_rules! sender_common {
    ($sender:ident, $weak:ident) => {
        impl<T> Clone for $sender<T> {
            fn clone(&self) -> Self {
                self.chan.add_sender();
                Self {
                    chan: Arc::clone(&self.chan),
                }
            }
        }

        impl<T> Drop for $sender<T> {
            fn drop(&mut self) {
                self.chan.drop_sender();
            }
        }

        impl<T> fmt::Debug for $sender<T> {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_struct(stringify!($sender))
                    .finish_non_exhaustive()
            }
        }

        impl<T> $sender<T> {
            pub fn is_closed(&self) -> bool {
                self.chan.is_closed()
            }

            pub async fn closed(&self) {
                ClosedWait {
                    chan: &self.chan,
                    ticket: None,
                }
                .await;
            }

            pub fn same_channel(&self, other: &Self) -> bool {
                Arc::ptr_eq(&self.chan, &other.chan)
            }

            #[must_use]
            pub fn downgrade(&self) -> $weak<T> {
                self.chan.weak.fetch_add(1, Ordering::AcqRel);
                $weak {
                    chan: Arc::clone(&self.chan),
                }
            }

            pub fn strong_count(&self) -> usize {
                self.chan.senders.load(Ordering::Acquire)
            }

            pub fn weak_count(&self) -> usize {
                self.chan.weak.load(Ordering::Acquire)
            }
        }

        impl<T> $weak<T> {
            pub fn upgrade(&self) -> Option<$sender<T>> {
                self.chan.upgrade().then(|| $sender {
                    chan: Arc::clone(&self.chan),
                })
            }

            pub fn strong_count(&self) -> usize {
                self.chan.senders.load(Ordering::Acquire)
            }

            pub fn weak_count(&self) -> usize {
                self.chan.weak.load(Ordering::Acquire)
            }
        }

        impl<T> Clone for $weak<T> {
            fn clone(&self) -> Self {
                self.chan.weak.fetch_add(1, Ordering::AcqRel);
                Self {
                    chan: Arc::clone(&self.chan),
                }
            }
        }

        impl<T> Drop for $weak<T> {
            fn drop(&mut self) {
                self.chan.weak.fetch_sub(1, Ordering::AcqRel);
            }
        }

        impl<T> fmt::Debug for $weak<T> {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_struct(stringify!($weak))
                    .finish_non_exhaustive()
            }
        }
    };
}

sender_common!(Sender, WeakSender);
sender_common!(UnboundedSender, WeakUnboundedSender);

macro_rules! receiver_common {
    ($receiver:ident) => {
        impl<T> fmt::Debug for $receiver<T> {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_struct(stringify!($receiver))
                    .finish_non_exhaustive()
            }
        }

        impl<T> $receiver<T> {
            pub async fn recv(&mut self) -> Option<T> {
                std::future::poll_fn(|context| self.rx.poll_recv(context)).await
            }

            pub async fn recv_many(&mut self, buffer: &mut Vec<T>, limit: usize) -> usize {
                std::future::poll_fn(|context| self.rx.poll_recv_many(context, buffer, limit)).await
            }

            pub fn poll_recv(&mut self, context: &mut Context<'_>) -> Poll<Option<T>> {
                self.rx.poll_recv(context)
            }

            pub fn poll_recv_many(
                &mut self,
                context: &mut Context<'_>,
                buffer: &mut Vec<T>,
                limit: usize,
            ) -> Poll<usize> {
                self.rx.poll_recv_many(context, buffer, limit)
            }

            pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
                self.rx.try_recv()
            }

            #[cfg(not(target_family = "wasm"))]
            pub fn blocking_recv(&mut self) -> Option<T> {
                crate::blocking::block_on(self.recv())
            }

            pub fn close(&mut self) {
                self.rx.close();
            }

            pub fn is_closed(&self) -> bool {
                self.rx.is_closed()
            }

            pub fn is_empty(&self) -> bool {
                self.rx.chan.queued() == 0
            }

            pub fn len(&self) -> usize {
                self.rx.chan.queued()
            }

            pub fn sender_strong_count(&self) -> usize {
                self.rx.chan.senders.load(Ordering::Acquire)
            }

            pub fn sender_weak_count(&self) -> usize {
                self.rx.chan.weak.load(Ordering::Acquire)
            }
        }
    };
}

receiver_common!(Receiver);
receiver_common!(UnboundedReceiver);

impl<T> UnboundedSender<T> {
    pub fn send(&self, message: T) -> Result<(), SendError<T>> {
        self.chan.push_open(message)
    }
}

impl<T> Receiver<T> {
    pub fn capacity(&self) -> usize {
        self.rx.chan.semaphore().available_permits()
    }

    pub fn max_capacity(&self) -> usize {
        self.rx.chan.capacity()
    }
}

impl<T> Chan<T> {
    const fn semaphore(&self) -> &Semaphore {
        &self
            .bound
            .as_ref()
            .expect("a bounded channel has a semaphore")
            .0
    }

    const fn capacity(&self) -> usize {
        self.bound
            .as_ref()
            .expect("a bounded channel has a capacity")
            .1
    }
}

impl<T> Sender<T> {
    pub async fn send(&self, message: T) -> Result<(), SendError<T>> {
        match Acquire::new(self.chan.semaphore(), 1).await {
            Ok(()) => {
                self.chan.push(message);
                Ok(())
            }
            Err(_) => Err(SendError(message)),
        }
    }

    pub fn try_send(&self, message: T) -> Result<(), TrySendError<T>> {
        match self.chan.semaphore().try_acquire() {
            Ok(permit) => {
                permit.forget();
                self.chan.push(message);
                Ok(())
            }
            Err(crate::TryAcquireError::NoPermits) => Err(TrySendError::Full(message)),
            Err(crate::TryAcquireError::Closed) => Err(TrySendError::Closed(message)),
        }
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn blocking_send(&self, message: T) -> Result<(), SendError<T>> {
        crate::blocking::block_on(self.send(message))
    }

    pub async fn reserve(&self) -> Result<Permit<'_, T>, SendError<()>> {
        Acquire::new(self.chan.semaphore(), 1)
            .await
            .map_err(|_| SendError(()))?;
        Ok(Permit { chan: &self.chan })
    }

    pub async fn reserve_many(&self, count: usize) -> Result<PermitIterator<'_, T>, SendError<()>> {
        Acquire::new(self.chan.semaphore(), count)
            .await
            .map_err(|_| SendError(()))?;
        Ok(PermitIterator {
            chan: &self.chan,
            count,
        })
    }

    pub async fn reserve_owned(self) -> Result<OwnedPermit<T>, SendError<()>> {
        Acquire::new(self.chan.semaphore(), 1)
            .await
            .map_err(|_| SendError(()))?;
        Ok(OwnedPermit { chan: Some(self) })
    }

    pub fn try_reserve(&self) -> Result<Permit<'_, T>, TrySendError<()>> {
        self.try_take(1)?;
        Ok(Permit { chan: &self.chan })
    }

    pub fn try_reserve_many(
        &self,
        count: usize,
    ) -> Result<PermitIterator<'_, T>, TrySendError<()>> {
        self.try_take(count)?;
        Ok(PermitIterator {
            chan: &self.chan,
            count,
        })
    }

    pub fn try_reserve_owned(self) -> Result<OwnedPermit<T>, TrySendError<Self>> {
        match self.try_take(1) {
            Ok(()) => Ok(OwnedPermit { chan: Some(self) }),
            Err(TrySendError::Full(())) => Err(TrySendError::Full(self)),
            Err(TrySendError::Closed(())) => Err(TrySendError::Closed(self)),
        }
    }

    fn try_take(&self, count: usize) -> Result<(), TrySendError<()>> {
        let count = u32::try_from(count).map_err(|_| TrySendError::Full(()))?;
        match self.chan.semaphore().try_acquire_many(count) {
            Ok(permit) => {
                permit.forget();
                Ok(())
            }
            Err(crate::TryAcquireError::NoPermits) => Err(TrySendError::Full(())),
            Err(crate::TryAcquireError::Closed) => Err(TrySendError::Closed(())),
        }
    }

    pub fn capacity(&self) -> usize {
        self.chan.semaphore().available_permits()
    }

    pub fn max_capacity(&self) -> usize {
        self.chan.capacity()
    }
}

pub struct Permit<'a, T> {
    chan: &'a Chan<T>,
}

impl<T> Permit<'_, T> {
    pub fn send(self, value: T) {
        self.chan.push(value);
        std::mem::forget(self);
    }
}

impl<T> Drop for Permit<'_, T> {
    fn drop(&mut self) {
        self.chan.unreserve(1);
    }
}

impl<T> fmt::Debug for Permit<'_, T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Permit").finish_non_exhaustive()
    }
}

pub struct PermitIterator<'a, T> {
    chan: &'a Chan<T>,
    count: usize,
}

impl<'a, T> Iterator for PermitIterator<'a, T> {
    type Item = Permit<'a, T>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.count == 0 {
            return None;
        }
        self.count -= 1;
        Some(Permit { chan: self.chan })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.count, Some(self.count))
    }
}

impl<T> ExactSizeIterator for PermitIterator<'_, T> {}

impl<T> Drop for PermitIterator<'_, T> {
    fn drop(&mut self) {
        if self.count > 0 {
            self.chan.unreserve(self.count);
        }
    }
}

pub struct OwnedPermit<T> {
    chan: Option<Sender<T>>,
}

impl<T> OwnedPermit<T> {
    pub fn send(mut self, value: T) -> Sender<T> {
        let sender = self.chan.take().expect("an owned permit holds its sender");
        sender.chan.push(value);
        sender
    }

    pub fn release(mut self) -> Sender<T> {
        let sender = self.chan.take().expect("an owned permit holds its sender");
        sender.chan.unreserve(1);
        sender
    }
}

impl<T> Drop for OwnedPermit<T> {
    fn drop(&mut self) {
        if let Some(sender) = self.chan.take() {
            sender.chan.unreserve(1);
        }
    }
}

impl<T> fmt::Debug for OwnedPermit<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OwnedPermit")
            .finish_non_exhaustive()
    }
}
