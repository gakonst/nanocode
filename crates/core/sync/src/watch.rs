use crate::{Atom, Notify};
use std::{
    fmt,
    marker::PhantomData,
    ops::Deref,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
};

const VERSION_STEP: u64 = 2;

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

    #[derive(Debug, Clone)]
    pub struct RecvError(pub(super) ());

    impl fmt::Display for RecvError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("channel closed")
        }
    }

    impl std::error::Error for RecvError {}
}

use error::{RecvError, SendError};

#[derive(Clone)]
struct Versioned<T> {
    version: u64,
    value: T,
}

struct Shared<T> {
    value: Atom<Versioned<T>>,
    versions: AtomicU64,
    closed: AtomicBool,
    receivers: AtomicUsize,
    senders: AtomicUsize,
    changed: Notify,
    dropped: Notify,
}

pub struct Ref<'a, T> {
    inner: Arc<Versioned<T>>,
    has_changed: bool,
    borrowed: PhantomData<&'a T>,
}

impl<T> Ref<'_, T> {
    pub const fn has_changed(&self) -> bool {
        self.has_changed
    }
}

impl<T> Deref for Ref<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.inner.value
    }
}

impl<T: fmt::Debug> fmt::Debug for Ref<'_, T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.value.fmt(formatter)
    }
}

pub struct Sender<T> {
    shared: Arc<Shared<T>>,
}

pub struct Receiver<T> {
    shared: Arc<Shared<T>>,
    version: u64,
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

#[must_use]
pub fn channel<T>(init: T) -> (Sender<T>, Receiver<T>) {
    let shared = Arc::new(Shared {
        value: Atom::new(Versioned {
            version: 0,
            value: init,
        }),
        versions: AtomicU64::new(0),
        closed: AtomicBool::new(false),
        receivers: AtomicUsize::new(1),
        senders: AtomicUsize::new(1),
        changed: Notify::new(),
        dropped: Notify::new(),
    });
    (
        Sender {
            shared: Arc::clone(&shared),
        },
        Receiver { shared, version: 0 },
    )
}

impl<T> Shared<T> {
    fn version(&self) -> u64 {
        self.value.read(|current| current.version)
    }

    fn next_version(&self) -> u64 {
        self.versions.fetch_add(VERSION_STEP, Ordering::AcqRel) + VERSION_STEP
    }

    fn maybe_changed(&self, version: &mut u64) -> Option<Result<(), RecvError>> {
        let current = self.version();
        if *version != current {
            *version = current;
            return Some(Ok(()));
        }
        if self.closed.load(Ordering::Acquire) {
            return Some(Err(RecvError(())));
        }
        None
    }

    async fn changed(&self, version: &mut u64) -> Result<(), RecvError> {
        loop {
            let notified = self.changed.notified();
            if let Some(outcome) = self.maybe_changed(version) {
                return outcome;
            }
            notified.await;
        }
    }
}

impl<T> Receiver<T> {
    pub fn borrow(&self) -> Ref<'_, T> {
        let inner = self.shared.value.get();
        let has_changed = self.version != inner.version;
        Ref {
            inner,
            has_changed,
            borrowed: PhantomData,
        }
    }

    pub fn borrow_and_update(&mut self) -> Ref<'_, T> {
        let inner = self.shared.value.get();
        let has_changed = self.version != inner.version;
        self.version = inner.version;
        Ref {
            inner,
            has_changed,
            borrowed: PhantomData,
        }
    }

    pub fn has_changed(&self) -> Result<bool, RecvError> {
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(RecvError(()));
        }
        Ok(self.version != self.shared.version())
    }

    pub fn mark_changed(&mut self) {
        self.version = self.version.wrapping_sub(1);
    }

    pub fn mark_unchanged(&mut self) {
        self.version = self.shared.version();
    }

    pub async fn changed(&mut self) -> Result<(), RecvError> {
        self.shared.changed(&mut self.version).await
    }

    pub async fn wait_for(
        &mut self,
        mut predicate: impl FnMut(&T) -> bool,
    ) -> Result<Ref<'_, T>, RecvError> {
        let mut closed = false;
        loop {
            let inner = self.shared.value.get();
            let has_changed = self.version != inner.version;
            self.version = inner.version;
            if (!closed || has_changed) && predicate(&inner.value) {
                return Ok(Ref {
                    inner,
                    has_changed,
                    borrowed: PhantomData,
                });
            }
            if closed {
                return Err(RecvError(()));
            }
            closed = self.shared.changed(&mut self.version).await.is_err();
        }
    }

    pub fn same_channel(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }
}

impl<T> Clone for Receiver<T> {
    fn clone(&self) -> Self {
        self.shared.receivers.fetch_add(1, Ordering::AcqRel);
        Self {
            shared: Arc::clone(&self.shared),
            version: self.version,
        }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        if self.shared.receivers.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.shared.dropped.notify_waiters();
        }
    }
}

impl<T> Sender<T> {
    #[must_use]
    pub fn new(init: T) -> Self {
        let (sender, _) = channel(init);
        sender
    }

    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        if self.receiver_count() == 0 {
            return Err(SendError(value));
        }
        self.publish(value);
        Ok(())
    }

    fn publish(&self, value: T) -> Arc<Versioned<T>> {
        let version = self.shared.next_version();
        let previous = self.shared.value.set(Versioned { version, value });
        self.shared.changed.notify_waiters();
        previous
    }

    pub fn borrow(&self) -> Ref<'_, T> {
        Ref {
            inner: self.shared.value.get(),
            has_changed: false,
            borrowed: PhantomData,
        }
    }

    pub fn is_closed(&self) -> bool {
        self.receiver_count() == 0
    }

    pub async fn closed(&self) {
        while self.receiver_count() > 0 {
            let notified = self.shared.dropped.notified();
            if self.receiver_count() == 0 {
                return;
            }
            notified.await;
        }
    }

    pub fn subscribe(&self) -> Receiver<T> {
        self.shared.receivers.fetch_add(1, Ordering::AcqRel);
        Receiver {
            shared: Arc::clone(&self.shared),
            version: self.shared.version(),
        }
    }

    pub fn receiver_count(&self) -> usize {
        self.shared.receivers.load(Ordering::Acquire)
    }

    pub fn sender_count(&self) -> usize {
        self.shared.senders.load(Ordering::Acquire)
    }

    pub fn same_channel(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }
}

impl<T: Clone> Sender<T> {
    pub fn send_replace(&self, value: T) -> T {
        let previous = self.publish(value);
        Arc::try_unwrap(previous).map_or_else(|shared| shared.value.clone(), |owned| owned.value)
    }

    pub fn send_modify<F: FnMut(&mut T)>(&self, mut modify: F) {
        self.send_if_modified(|value| {
            modify(value);
            true
        });
    }

    pub fn send_if_modified<F: FnMut(&mut T) -> bool>(&self, mut modify: F) -> bool {
        let shared = &self.shared;
        let modified = shared.value.modify(|current| {
            if !modify(&mut current.value) {
                return Err(());
            }
            current.version = shared.next_version();
            Ok(())
        });
        if modified.is_err() {
            return false;
        }
        shared.changed.notify_waiters();
        true
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
            self.shared.closed.store(true, Ordering::Release);
            self.shared.changed.notify_waiters();
        }
    }
}
