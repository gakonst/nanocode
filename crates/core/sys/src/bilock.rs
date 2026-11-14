use crate::AtomicWaker;
use std::{
    cell::UnsafeCell,
    ops::{Deref, DerefMut},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
};

const SIDES: usize = 2;

struct Inner<T> {
    locked: AtomicBool,
    wakers: [AtomicWaker; SIDES],
    value: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for Inner<T> {}
unsafe impl<T: Send> Sync for Inner<T> {}

pub struct BiLock<T> {
    inner: Arc<Inner<T>>,
    side: usize,
}

impl<T> BiLock<T> {
    pub fn new(value: T) -> (Self, Self) {
        let inner = Arc::new(Inner {
            locked: AtomicBool::new(false),
            wakers: [AtomicWaker::new(), AtomicWaker::new()],
            value: UnsafeCell::new(value),
        });
        (
            Self {
                inner: Arc::clone(&inner),
                side: 0,
            },
            Self { inner, side: 1 },
        )
    }

    pub fn poll_lock(&self, context: &mut Context<'_>) -> Poll<BiLockGuard<'_, T>> {
        self.inner.wakers[self.side].register(context.waker());
        match self
            .inner
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        {
            Ok(_) => Poll::Ready(BiLockGuard { lock: self }),
            Err(_) => Poll::Pending,
        }
    }

    pub fn is_pair_of(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner) && self.side != other.side
    }
}

pub struct BiLockGuard<'a, T> {
    lock: &'a BiLock<T>,
}

impl<T> BiLockGuard<'_, T> {
    pub fn as_pin_mut(&mut self) -> Pin<&mut T> {
        unsafe { Pin::new_unchecked(&mut *self.lock.inner.value.get()) }
    }
}

impl<T: Unpin> Deref for BiLockGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { &*self.lock.inner.value.get() }
    }
}

impl<T: Unpin> DerefMut for BiLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.inner.value.get() }
    }
}

impl<T> Drop for BiLockGuard<'_, T> {
    fn drop(&mut self) {
        let inner = &self.lock.inner;
        let previous = inner.locked.swap(false, Ordering::Release);
        assert!(previous, "a held bilock is locked until its guard drops");
        inner.wakers[SIDES - 1 - self.lock.side].wake();
    }
}
