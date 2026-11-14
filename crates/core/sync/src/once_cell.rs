use crate::{Semaphore, TryAcquireError};
use std::{fmt, future::Future, sync::OnceLock};

pub struct OnceCell<T> {
    value: OnceLock<T>,
    semaphore: Semaphore,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SetError<T> {
    AlreadyInitializedError(T),
    InitializingError(T),
}

impl<T> fmt::Display for SetError<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyInitializedError(_) => formatter.write_str("AlreadyInitializedError"),
            Self::InitializingError(_) => formatter.write_str("InitializingError"),
        }
    }
}

impl<T: fmt::Debug> std::error::Error for SetError<T> {}

impl<T> SetError<T> {
    pub const fn is_already_init_err(&self) -> bool {
        matches!(self, Self::AlreadyInitializedError(_))
    }

    pub const fn is_initializing_err(&self) -> bool {
        matches!(self, Self::InitializingError(_))
    }
}

impl<T> Default for OnceCell<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: fmt::Debug> fmt::Debug for OnceCell<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OnceCell")
            .field("value", &self.get())
            .finish()
    }
}

impl<T: Clone> Clone for OnceCell<T> {
    fn clone(&self) -> Self {
        Self::new_with(self.get().cloned())
    }
}

impl<T: PartialEq> PartialEq for OnceCell<T> {
    fn eq(&self, other: &Self) -> bool {
        self.get() == other.get()
    }
}

impl<T: Eq> Eq for OnceCell<T> {}

impl<T> From<T> for OnceCell<T> {
    fn from(value: T) -> Self {
        Self::new_with(Some(value))
    }
}

impl<T> OnceCell<T> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            value: OnceLock::new(),
            semaphore: Semaphore::new(1),
        }
    }

    #[must_use]
    pub fn new_with(value: Option<T>) -> Self {
        let cell = Self::new();
        if let Some(value) = value {
            assert!(cell.value.set(value).is_ok(), "a new cell is empty");
            cell.semaphore.close();
        }
        cell
    }

    pub fn initialized(&self) -> bool {
        self.value.get().is_some()
    }

    pub fn get(&self) -> Option<&T> {
        self.value.get()
    }

    pub fn get_mut(&mut self) -> Option<&mut T> {
        self.value.get_mut()
    }

    fn fill(&self, value: T) -> &T {
        assert!(
            self.value.set(value).is_ok(),
            "only the permit holder fills the cell"
        );
        self.semaphore.close();
        self.value.get().expect("the cell was just filled")
    }

    pub fn set(&self, value: T) -> Result<(), SetError<T>> {
        if self.initialized() {
            return Err(SetError::AlreadyInitializedError(value));
        }
        match self.semaphore.try_acquire() {
            Ok(permit) => {
                permit.forget();
                self.fill(value);
                Ok(())
            }
            Err(TryAcquireError::NoPermits) => Err(SetError::InitializingError(value)),
            Err(TryAcquireError::Closed) => Err(SetError::AlreadyInitializedError(value)),
        }
    }

    pub async fn get_or_init<F, Fut>(&self, init: F) -> &T
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        if let Some(value) = self.get() {
            return value;
        }
        let Ok(permit) = self.semaphore.acquire().await else {
            return self.get().expect("a closed cell holds its value");
        };
        if let Some(value) = self.get() {
            return value;
        }
        let value = init().await;
        permit.forget();
        self.fill(value)
    }

    pub async fn get_or_try_init<E, F, Fut>(&self, init: F) -> Result<&T, E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        if let Some(value) = self.get() {
            return Ok(value);
        }
        let Ok(permit) = self.semaphore.acquire().await else {
            return Ok(self.get().expect("a closed cell holds its value"));
        };
        if let Some(value) = self.get() {
            return Ok(value);
        }
        let value = init().await?;
        permit.forget();
        Ok(self.fill(value))
    }

    pub fn into_inner(self) -> Option<T> {
        self.value.into_inner()
    }

    pub fn take(&mut self) -> Option<T> {
        std::mem::take(self).into_inner()
    }
}
