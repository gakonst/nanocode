use core::cell::Cell;
use core::fmt;

pub struct AtomicCell<T> {
    value: Cell<T>,
}

impl<T> AtomicCell<T> {
    #[inline]
    #[must_use]
    pub const fn new(value: T) -> Self {
        Self {
            value: Cell::new(value),
        }
    }

    #[inline]
    pub fn into_inner(self) -> T {
        self.value.into_inner()
    }

    #[inline]
    pub fn store(&self, value: T) {
        self.value.set(value);
    }

    #[inline]
    pub fn swap(&self, value: T) -> T {
        self.value.replace(value)
    }

    #[inline]
    #[must_use]
    pub const fn as_ptr(&self) -> *mut T {
        self.value.as_ptr()
    }
}

impl<T: Default> AtomicCell<T> {
    #[inline]
    pub fn take(&self) -> T {
        self.value.take()
    }
}

impl<T: Copy> AtomicCell<T> {
    #[inline]
    pub fn load(&self) -> T {
        self.value.get()
    }
}

impl<T: Copy + Eq> AtomicCell<T> {
    #[inline]
    pub fn compare_exchange(&self, current: T, new: T) -> Result<T, T> {
        let previous = self.value.get();
        if previous == current {
            self.value.set(new);
            Ok(previous)
        } else {
            Err(previous)
        }
    }

    #[inline]
    pub fn fetch_update<F>(&self, mut update: F) -> Result<T, T>
    where
        F: FnMut(T) -> Option<T>,
    {
        let mut previous = self.load();
        while let Some(next) = update(previous) {
            match self.compare_exchange(previous, next) {
                Ok(value) => return Ok(value),
                Err(current) => previous = current,
            }
        }
        Err(previous)
    }
}

macro_rules! impl_integer_operations {
    ($($integer:ty),* $(,)?) => {
        $(
            impl AtomicCell<$integer> {
                #[inline]
                pub fn fetch_add(&self, value: $integer) -> $integer {
                    self.value.replace(self.value.get().wrapping_add(value))
                }

                #[inline]
                pub fn fetch_sub(&self, value: $integer) -> $integer {
                    self.value.replace(self.value.get().wrapping_sub(value))
                }

                #[inline]
                pub fn fetch_and(&self, value: $integer) -> $integer {
                    self.value.replace(self.value.get() & value)
                }

                #[inline]
                pub fn fetch_nand(&self, value: $integer) -> $integer {
                    self.value.replace(!(self.value.get() & value))
                }

                #[inline]
                pub fn fetch_or(&self, value: $integer) -> $integer {
                    self.value.replace(self.value.get() | value)
                }

                #[inline]
                pub fn fetch_xor(&self, value: $integer) -> $integer {
                    self.value.replace(self.value.get() ^ value)
                }

                #[inline]
                pub fn fetch_max(&self, value: $integer) -> $integer {
                    self.value.replace(self.value.get().max(value))
                }

                #[inline]
                pub fn fetch_min(&self, value: $integer) -> $integer {
                    self.value.replace(self.value.get().min(value))
                }
            }
        )*
    };
}

impl_integer_operations!(
    u8, i8, u16, i16, u32, i32, u64, i64, u128, i128, usize, isize,
);

impl AtomicCell<bool> {
    #[inline]
    pub fn fetch_and(&self, value: bool) -> bool {
        self.value.replace(self.value.get() & value)
    }

    #[inline]
    pub fn fetch_nand(&self, value: bool) -> bool {
        self.value.replace(!(self.value.get() & value))
    }

    #[inline]
    pub fn fetch_or(&self, value: bool) -> bool {
        self.value.replace(self.value.get() | value)
    }

    #[inline]
    pub fn fetch_xor(&self, value: bool) -> bool {
        self.value.replace(self.value.get() ^ value)
    }
}

impl<T: Default> Default for AtomicCell<T> {
    #[inline]
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T> From<T> for AtomicCell<T> {
    #[inline]
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

impl<T: Copy + fmt::Debug> fmt::Debug for AtomicCell<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AtomicCell")
            .field("value", &self.load())
            .finish()
    }
}
