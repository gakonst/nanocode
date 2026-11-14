use crate::{Atom, Notify, Vector};
use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

pub mod error {
    use std::fmt;

    #[derive(Debug)]
    pub struct SendError<T>(pub T);

    impl<T> fmt::Display for SendError<T> {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("channel closed")
        }
    }

    impl<T: fmt::Debug> std::error::Error for SendError<T> {}

    #[derive(Debug, PartialEq, Eq, Clone)]
    pub enum RecvError {
        Closed,
        Lagged(u64),
    }

    impl fmt::Display for RecvError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::Closed => formatter.write_str("channel closed"),
                Self::Lagged(missed) => write!(formatter, "channel lagged by {missed}"),
            }
        }
    }

    impl std::error::Error for RecvError {}

    #[derive(Debug, PartialEq, Eq, Clone)]
    pub enum TryRecvError {
        Empty,
        Closed,
        Lagged(u64),
    }

    impl fmt::Display for TryRecvError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::Empty => formatter.write_str("channel empty"),
                Self::Closed => formatter.write_str("channel closed"),
                Self::Lagged(missed) => write!(formatter, "channel lagged by {missed}"),
            }
        }
    }

    impl std::error::Error for TryRecvError {}
}

use error::{RecvError, SendError, TryRecvError};

struct Ring<T> {
    tail: u64,
    receivers: usize,
    closed: bool,
    slots: Vector<Option<Arc<(u64, T)>>>,
}

impl<T> Clone for Ring<T> {
    fn clone(&self) -> Self {
        Self {
            tail: self.tail,
            receivers: self.receivers,
            closed: self.closed,
            slots: self.slots.clone(),
        }
    }
}

struct Shared<T> {
    ring: Atom<Ring<T>>,
    capacity: u64,
    senders: AtomicUsize,
    weak: AtomicUsize,
    sent: Notify,
    last_receiver: Notify,
}

impl<T> Shared<T> {
    fn index(&self, position: u64) -> usize {
        usize::try_from(position % self.capacity).expect("a ring index fits usize")
    }
}

pub struct Sender<T> {
    shared: Arc<Shared<T>>,
}

pub struct WeakSender<T> {
    shared: Arc<Shared<T>>,
}

pub struct Receiver<T> {
    shared: Arc<Shared<T>>,
    next: u64,
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

impl<T> fmt::Debug for WeakSender<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("WeakSender").finish_non_exhaustive()
    }
}

fn shared<T: Clone>(receivers: usize, capacity: usize) -> Arc<Shared<T>> {
    assert!(capacity > 0, "broadcast channel capacity cannot be zero");
    assert!(
        capacity <= usize::MAX >> 1,
        "broadcast channel capacity exceeded `usize::MAX / 2`"
    );
    let capacity = capacity.next_power_of_two();
    let mut slots = Vector::new();
    for _ in 0..capacity {
        slots.push_back(None);
    }
    Arc::new(Shared {
        ring: Atom::new(Ring {
            tail: 0,
            receivers,
            closed: receivers == 0,
            slots,
        }),
        capacity: u64::try_from(capacity).expect("a capacity fits u64"),
        senders: AtomicUsize::new(1),
        weak: AtomicUsize::new(0),
        sent: Notify::new(),
        last_receiver: Notify::new(),
    })
}

#[must_use]
pub fn channel<T: Clone>(capacity: usize) -> (Sender<T>, Receiver<T>) {
    let shared = shared(1, capacity);
    (
        Sender {
            shared: Arc::clone(&shared),
        },
        Receiver { shared, next: 0 },
    )
}

fn new_receiver<T: Clone>(shared: &Arc<Shared<T>>) -> Receiver<T> {
    let next = shared.ring.update(|ring| {
        if ring.receivers == 0 {
            ring.closed = false;
        }
        ring.receivers += 1;
        ring.tail
    });
    Receiver {
        shared: Arc::clone(shared),
        next,
    }
}

impl<T: Clone> Sender<T> {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            shared: shared(0, capacity),
        }
    }

    pub fn send(&self, value: T) -> Result<usize, SendError<T>> {
        let shared = &self.shared;
        let entry = Arc::new((0, value));
        let sent = shared.ring.modify(|ring| {
            if ring.receivers == 0 {
                return Err(());
            }
            let position = ring.tail;
            ring.tail = ring.tail.wrapping_add(1);
            let index = shared.index(position);
            ring.slots[index] = Some(Arc::new((position, entry.1.clone())));
            Ok(ring.receivers)
        });
        match sent {
            Ok(receivers) => {
                shared.sent.notify_waiters();
                Ok(receivers)
            }
            Err(()) => Err(SendError(
                Arc::try_unwrap(entry).map_or_else(|entry| entry.1.clone(), |entry| entry.1),
            )),
        }
    }

    pub fn subscribe(&self) -> Receiver<T> {
        new_receiver(&self.shared)
    }

    pub fn receiver_count(&self) -> usize {
        self.shared.ring.read(|ring| ring.receivers)
    }
}

impl<T> Sender<T> {
    #[must_use]
    pub fn downgrade(&self) -> WeakSender<T> {
        self.shared.weak.fetch_add(1, Ordering::AcqRel);
        WeakSender {
            shared: Arc::clone(&self.shared),
        }
    }

    pub fn same_channel(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }

    pub async fn closed(&self) {
        loop {
            let notified = self.shared.last_receiver.notified();
            if self.shared.ring.read(|ring| ring.closed) {
                return;
            }
            notified.await;
        }
    }

    pub fn strong_count(&self) -> usize {
        self.shared.senders.load(Ordering::Acquire)
    }

    pub fn weak_count(&self) -> usize {
        self.shared.weak.load(Ordering::Acquire)
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        self.shared.senders.fetch_add(1, Ordering::AcqRel);
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        if self.shared.senders.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.shared.ring.update(|ring| ring.closed = true);
            self.shared.sent.notify_waiters();
        }
    }
}

impl<T> WeakSender<T> {
    #[must_use]
    pub fn upgrade(&self) -> Option<Sender<T>> {
        let mut current = self.shared.senders.load(Ordering::Acquire);
        loop {
            if current == 0 {
                return None;
            }
            match self.shared.senders.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Some(Sender {
                        shared: Arc::clone(&self.shared),
                    });
                }
                Err(actual) => current = actual,
            }
        }
    }

    pub fn strong_count(&self) -> usize {
        self.shared.senders.load(Ordering::Acquire)
    }

    pub fn weak_count(&self) -> usize {
        self.shared.weak.load(Ordering::Acquire)
    }
}

impl<T> Clone for WeakSender<T> {
    fn clone(&self) -> Self {
        self.shared.weak.fetch_add(1, Ordering::AcqRel);
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<T> Drop for WeakSender<T> {
    fn drop(&mut self) {
        self.shared.weak.fetch_sub(1, Ordering::AcqRel);
    }
}

impl<T> Receiver<T> {
    pub fn len(&self) -> usize {
        let tail = self.shared.ring.read(|ring| ring.tail);
        usize::try_from(tail.wrapping_sub(self.next)).expect("a backlog fits usize")
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn same_channel(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }

    pub fn sender_strong_count(&self) -> usize {
        self.shared.senders.load(Ordering::Acquire)
    }

    pub fn sender_weak_count(&self) -> usize {
        self.shared.weak.load(Ordering::Acquire)
    }

    pub fn is_closed(&self) -> bool {
        self.shared.senders.load(Ordering::Acquire) == 0
    }
}

impl<T: Clone> Receiver<T> {
    #[must_use]
    pub fn resubscribe(&self) -> Self {
        new_receiver(&self.shared)
    }

    pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
        let shared = &self.shared;
        let next = self.next;
        let (slot, tail, closed) = shared.ring.read(|ring| {
            (
                ring.slots.get(shared.index(next)).cloned().flatten(),
                ring.tail,
                ring.closed,
            )
        });
        let position = slot
            .as_ref()
            .map_or(next.wrapping_sub(shared.capacity), |slot| slot.0);
        if position == next {
            self.next = next.wrapping_add(1);
            return Ok(slot.expect("a matching slot holds its value").1.clone());
        }
        if position.wrapping_add(shared.capacity) == next {
            return Err(if closed {
                TryRecvError::Closed
            } else {
                TryRecvError::Empty
            });
        }
        let oldest = tail.wrapping_sub(shared.capacity);
        let missed = oldest.wrapping_sub(next);
        if missed == 0 {
            self.next = next.wrapping_add(1);
            return Ok(slot.expect("a lagging slot holds a value").1.clone());
        }
        self.next = oldest;
        Err(TryRecvError::Lagged(missed))
    }

    pub async fn recv(&mut self) -> Result<T, RecvError> {
        loop {
            let shared = Arc::clone(&self.shared);
            let notified = shared.sent.notified();
            let mut notified = std::pin::pin!(notified);
            notified.as_mut().enable();
            match self.try_recv() {
                Ok(value) => return Ok(value),
                Err(TryRecvError::Closed) => return Err(RecvError::Closed),
                Err(TryRecvError::Lagged(missed)) => return Err(RecvError::Lagged(missed)),
                Err(TryRecvError::Empty) => notified.await,
            }
        }
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn blocking_recv(&mut self) -> Result<T, RecvError> {
        crate::blocking::block_on(self.recv())
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let last = self.shared.ring.update(|ring| {
            ring.receivers -= 1;
            if ring.receivers == 0 {
                ring.closed = true;
                return true;
            }
            false
        });
        if last {
            self.shared.last_receiver.notify_waiters();
        }
    }
}
