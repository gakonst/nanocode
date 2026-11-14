use std::{
    cell::UnsafeCell,
    fmt,
    sync::atomic::{
        AtomicUsize,
        Ordering::{AcqRel, Acquire, Release},
    },
    task::Waker,
};

const WAITING: usize = 0;
const REGISTERING: usize = 0b01;
const WAKING: usize = 0b10;

pub struct AtomicWaker {
    state: AtomicUsize,
    waker: UnsafeCell<Option<Waker>>,
}

unsafe impl Send for AtomicWaker {}
unsafe impl Sync for AtomicWaker {}

impl Default for AtomicWaker {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for AtomicWaker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AtomicWaker")
    }
}

impl AtomicWaker {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: AtomicUsize::new(WAITING),
            waker: UnsafeCell::new(None),
        }
    }

    pub fn register(&self, waker: &Waker) {
        match self
            .state
            .compare_exchange(WAITING, REGISTERING, Acquire, Acquire)
            .unwrap_or_else(|actual| actual)
        {
            WAITING => {
                let slot = self.waker.get();
                let kept = unsafe { (*slot).as_ref() }.is_some_and(|old| old.will_wake(waker));
                if !kept {
                    unsafe { *slot = Some(waker.clone()) };
                }
                if let Err(actual) =
                    self.state
                        .compare_exchange(REGISTERING, WAITING, AcqRel, Acquire)
                {
                    assert_eq!(actual, REGISTERING | WAKING);
                    let registered = unsafe { (*slot).take() };
                    self.state.swap(WAITING, AcqRel);
                    if let Some(registered) = registered {
                        registered.wake();
                    }
                }
            }
            WAKING => waker.wake_by_ref(),
            state => assert!(state == REGISTERING || state == REGISTERING | WAKING),
        }
    }

    pub fn wake(&self) {
        if let Some(waker) = self.take() {
            waker.wake();
        }
    }

    pub fn take(&self) -> Option<Waker> {
        match self.state.fetch_or(WAKING, AcqRel) {
            WAITING => {
                let waker = unsafe { (*self.waker.get()).take() };
                let previous = self.state.swap(WAITING, Release);
                assert_eq!(previous, WAKING);
                waker
            }
            state => {
                assert!(state == REGISTERING || state == REGISTERING | WAKING || state == WAKING);
                None
            }
        }
    }
}
