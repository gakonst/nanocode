use crate::Notify;
use std::{
    fmt,
    sync::{
        OnceLock,
        atomic::{Ordering, fence},
    },
};

pub struct SetOnce<T> {
    value: OnceLock<T>,
    notify: OnceLock<Notify>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct SetOnceError<T>(pub T);

impl<T> fmt::Display for SetOnceError<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SetOnceError")
    }
}

impl<T: fmt::Debug> std::error::Error for SetOnceError<T> {}

impl<T> Default for SetOnce<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: fmt::Debug> fmt::Debug for SetOnce<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SetOnce")
            .field("value", &self.get())
            .finish()
    }
}

impl<T> SetOnce<T> {
    #[must_use]
    pub const fn new() -> Self {
        Self::const_new()
    }

    #[must_use]
    pub const fn const_new() -> Self {
        Self {
            value: OnceLock::new(),
            notify: OnceLock::new(),
        }
    }

    #[must_use]
    pub fn new_with(value: Option<T>) -> Self {
        let cell = Self::new();
        if let Some(value) = value {
            assert!(cell.value.set(value).is_ok(), "a new cell is empty");
        }
        cell
    }

    pub fn initialized(&self) -> bool {
        self.value.get().is_some()
    }

    pub fn get(&self) -> Option<&T> {
        self.value.get()
    }

    pub fn set(&self, value: T) -> Result<(), SetOnceError<T>> {
        self.value.set(value).map_err(SetOnceError)?;
        fence(Ordering::SeqCst);
        if let Some(notify) = self.notify.get() {
            notify.notify_waiters();
        }
        Ok(())
    }

    pub fn into_inner(self) -> Option<T> {
        self.value.into_inner()
    }

    pub async fn wait(&self) -> &T {
        loop {
            if let Some(value) = self.value.get() {
                return value;
            }
            let notify = self.notify.get_or_init(Notify::new);
            let notified = notify.notified();
            let mut notified = std::pin::pin!(notified);
            notified.as_mut().enable();
            fence(Ordering::SeqCst);
            if let Some(value) = self.value.get() {
                return value;
            }
            notified.await;
        }
    }
}
