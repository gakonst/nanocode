use core::ptr::{self, NonNull};
pub use core::sync::atomic::*;

#[cfg(not(feature = "threading"))]
mod cell;

#[cfg(not(feature = "threading"))]
pub use cell::AtomicCell;
#[cfg(feature = "threading")]
pub use crossbeam_utils::atomic::AtomicCell;

mod sealed {
    pub trait Sealed {}
}
pub trait PyAtomicScalar: sealed::Sealed {
    type Radium: Radium<Item = Self>;
}

pub type PyAtomic<T> = <T as PyAtomicScalar>::Radium;

#[cfg(feature = "threading")]
macro_rules! atomic_ty {
    ($i:ty, $atomic:ty) => {
        $atomic
    };
}
#[cfg(not(feature = "threading"))]
macro_rules! atomic_ty {
    ($i:ty, $atomic:ty) => {
        core::cell::Cell<$i>
    };
}
macro_rules! impl_atomic_scalar {
    ($(($i:ty, $atomic:ty),)*) => {
        $(
            impl sealed::Sealed for $i {}
            impl PyAtomicScalar for $i {
                type Radium = atomic_ty!($i, $atomic);
            }
        )*
    };
}
impl_atomic_scalar!(
    (u8, AtomicU8),
    (i8, AtomicI8),
    (u16, AtomicU16),
    (i16, AtomicI16),
    (u32, AtomicU32),
    (i32, AtomicI32),
    (u64, AtomicU64),
    (i64, AtomicI64),
    (usize, AtomicUsize),
    (isize, AtomicIsize),
    (bool, AtomicBool),
);

impl<T> sealed::Sealed for *mut T {}
impl<T> PyAtomicScalar for *mut T {
    type Radium = atomic_ty!(*mut T, AtomicPtr<T>);
}

/// Atomic `f64`. The value is stored as its bits in an `AtomicU64`.
#[repr(transparent)]
pub struct AtomicF64 {
    bits: AtomicU64,
}

impl AtomicF64 {
    #[inline]
    #[must_use]
    pub const fn new(value: f64) -> Self {
        Self {
            bits: AtomicU64::new(value.to_bits()),
        }
    }

    #[inline]
    #[must_use]
    pub fn load(&self, order: Ordering) -> f64 {
        f64::from_bits(self.bits.load(order))
    }

    #[inline]
    pub fn store(&self, value: f64, order: Ordering) {
        self.bits.store(value.to_bits(), order);
    }
}

pub struct OncePtr<T> {
    inner: PyAtomic<*mut T>,
}

impl<T> Default for OncePtr<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> OncePtr<T> {
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Radium::new(ptr::null_mut()),
        }
    }

    pub fn get(&self) -> Option<NonNull<T>> {
        NonNull::new(self.inner.load(Ordering::Acquire))
    }

    pub fn set(&self, value: NonNull<T>) -> Result<(), NonNull<T>> {
        let exchange = self.inner.compare_exchange(
            ptr::null_mut(),
            value.as_ptr(),
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        match exchange {
            Ok(_) => Ok(()),
            Err(_) => Err(value),
        }
    }

    pub fn get_or_init<F>(&self, f: F) -> NonNull<T>
    where
        F: FnOnce() -> Box<T>,
    {
        enum Void {}
        match self.get_or_try_init(|| Ok::<_, Void>(f())) {
            Ok(val) => val,
            Err(void) => match void {},
        }
    }

    pub fn get_or_try_init<F, E>(&self, f: F) -> Result<NonNull<T>, E>
    where
        F: FnOnce() -> Result<Box<T>, E>,
    {
        if let Some(val) = self.get() {
            return Ok(val);
        }

        Ok(self.initialize(f()?))
    }

    #[cold]
    fn initialize(&self, val: Box<T>) -> NonNull<T> {
        let ptr = Box::into_raw(val);
        let exchange =
            self.inner
                .compare_exchange(ptr::null_mut(), ptr, Ordering::AcqRel, Ordering::Acquire);
        let ptr = match exchange {
            Ok(_) => ptr,
            Err(winner) => {
                drop(unsafe { Box::from_raw(ptr) });
                winner
            }
        };
        unsafe { NonNull::new_unchecked(ptr) }
    }
}

pub trait Radium {
    type Item;

    fn new(value: Self::Item) -> Self;
    fn get_mut(&mut self) -> &mut Self::Item;
    fn into_inner(self) -> Self::Item;
    fn load(&self, order: Ordering) -> Self::Item;
    fn store(&self, value: Self::Item, order: Ordering);
    fn swap(&self, value: Self::Item, order: Ordering) -> Self::Item;
    fn compare_exchange(
        &self,
        current: Self::Item,
        new: Self::Item,
        success: Ordering,
        failure: Ordering,
    ) -> Result<Self::Item, Self::Item>;
    fn compare_exchange_weak(
        &self,
        current: Self::Item,
        new: Self::Item,
        success: Ordering,
        failure: Ordering,
    ) -> Result<Self::Item, Self::Item>;
    fn fetch_update<F>(
        &self,
        set_order: Ordering,
        fetch_order: Ordering,
        function: F,
    ) -> Result<Self::Item, Self::Item>
    where
        F: FnMut(Self::Item) -> Option<Self::Item>;
}

pub trait RadiumBits: Radium {
    fn fetch_and(&self, value: Self::Item, order: Ordering) -> Self::Item;
    fn fetch_or(&self, value: Self::Item, order: Ordering) -> Self::Item;
    fn fetch_xor(&self, value: Self::Item, order: Ordering) -> Self::Item;
}

pub trait RadiumNumber: RadiumBits {
    fn fetch_add(&self, value: Self::Item, order: Ordering) -> Self::Item;
    fn fetch_sub(&self, value: Self::Item, order: Ordering) -> Self::Item;
}

macro_rules! radium_atomic {
    ($item:ty, $atomic:ty) => {
        impl Radium for $atomic {
            type Item = $item;

            #[inline]
            fn new(value: $item) -> Self {
                <$atomic>::new(value)
            }

            #[inline]
            fn get_mut(&mut self) -> &mut $item {
                <$atomic>::get_mut(self)
            }

            #[inline]
            fn into_inner(self) -> $item {
                <$atomic>::into_inner(self)
            }

            #[inline]
            fn load(&self, order: Ordering) -> $item {
                <$atomic>::load(self, order)
            }

            #[inline]
            fn store(&self, value: $item, order: Ordering) {
                <$atomic>::store(self, value, order);
            }

            #[inline]
            fn swap(&self, value: $item, order: Ordering) -> $item {
                <$atomic>::swap(self, value, order)
            }

            #[inline]
            fn compare_exchange(
                &self,
                current: $item,
                new: $item,
                success: Ordering,
                failure: Ordering,
            ) -> Result<$item, $item> {
                <$atomic>::compare_exchange(self, current, new, success, failure)
            }

            #[inline]
            fn compare_exchange_weak(
                &self,
                current: $item,
                new: $item,
                success: Ordering,
                failure: Ordering,
            ) -> Result<$item, $item> {
                <$atomic>::compare_exchange_weak(self, current, new, success, failure)
            }

            #[inline]
            fn fetch_update<F>(&self, set_order: Ordering, fetch_order: Ordering, function: F) -> Result<$item, $item>
            where
                F: FnMut($item) -> Option<$item>,
            {
                <$atomic>::try_update(self, set_order, fetch_order, function)
            }
        }

        impl Radium for core::cell::Cell<$item> {
            type Item = $item;

            #[inline]
            fn new(value: $item) -> Self {
                core::cell::Cell::new(value)
            }

            #[inline]
            fn get_mut(&mut self) -> &mut $item {
                core::cell::Cell::get_mut(self)
            }

            #[inline]
            fn into_inner(self) -> $item {
                core::cell::Cell::into_inner(self)
            }

            #[inline]
            fn load(&self, _order: Ordering) -> $item {
                self.get()
            }

            #[inline]
            fn store(&self, value: $item, _order: Ordering) {
                self.set(value);
            }

            #[inline]
            fn swap(&self, value: $item, _order: Ordering) -> $item {
                self.replace(value)
            }

            #[inline]
            fn compare_exchange(
                &self,
                current: $item,
                new: $item,
                _success: Ordering,
                _failure: Ordering,
            ) -> Result<$item, $item> {
                let previous = self.get();
                if previous == current {
                    self.set(new);
                    Ok(previous)
                } else {
                    Err(previous)
                }
            }

            #[inline]
            fn compare_exchange_weak(
                &self,
                current: $item,
                new: $item,
                success: Ordering,
                failure: Ordering,
            ) -> Result<$item, $item> {
                Radium::compare_exchange(self, current, new, success, failure)
            }

            #[inline]
            fn fetch_update<F>(&self, _set_order: Ordering, _fetch_order: Ordering, mut function: F) -> Result<$item, $item>
            where
                F: FnMut($item) -> Option<$item>,
            {
                let previous = self.get();
                match function(previous) {
                    Some(next) => {
                        self.set(next);
                        Ok(previous)
                    }
                    None => Err(previous),
                }
            }
        }
    };
}

macro_rules! radium_bits {
    ($item:ty, $atomic:ty) => {
        impl RadiumBits for $atomic {
            #[inline]
            fn fetch_and(&self, value: $item, order: Ordering) -> $item {
                <$atomic>::fetch_and(self, value, order)
            }

            #[inline]
            fn fetch_or(&self, value: $item, order: Ordering) -> $item {
                <$atomic>::fetch_or(self, value, order)
            }

            #[inline]
            fn fetch_xor(&self, value: $item, order: Ordering) -> $item {
                <$atomic>::fetch_xor(self, value, order)
            }
        }

        impl RadiumBits for core::cell::Cell<$item> {
            #[inline]
            fn fetch_and(&self, value: $item, _order: Ordering) -> $item {
                let previous = self.get();
                self.set(previous & value);
                previous
            }

            #[inline]
            fn fetch_or(&self, value: $item, _order: Ordering) -> $item {
                let previous = self.get();
                self.set(previous | value);
                previous
            }

            #[inline]
            fn fetch_xor(&self, value: $item, _order: Ordering) -> $item {
                let previous = self.get();
                self.set(previous ^ value);
                previous
            }
        }
    };
}

macro_rules! radium_number {
    ($item:ty, $atomic:ty) => {
        radium_atomic!($item, $atomic);
        radium_bits!($item, $atomic);

        impl RadiumNumber for $atomic {
            #[inline]
            fn fetch_add(&self, value: $item, order: Ordering) -> $item {
                <$atomic>::fetch_add(self, value, order)
            }

            #[inline]
            fn fetch_sub(&self, value: $item, order: Ordering) -> $item {
                <$atomic>::fetch_sub(self, value, order)
            }
        }

        impl RadiumNumber for core::cell::Cell<$item> {
            #[inline]
            fn fetch_add(&self, value: $item, _order: Ordering) -> $item {
                let previous = self.get();
                self.set(previous.wrapping_add(value));
                previous
            }

            #[inline]
            fn fetch_sub(&self, value: $item, _order: Ordering) -> $item {
                let previous = self.get();
                self.set(previous.wrapping_sub(value));
                previous
            }
        }
    };
}

radium_number!(u8, AtomicU8);
radium_number!(i8, AtomicI8);
radium_number!(u16, AtomicU16);
radium_number!(i16, AtomicI16);
radium_number!(u32, AtomicU32);
radium_number!(i32, AtomicI32);
radium_number!(u64, AtomicU64);
radium_number!(i64, AtomicI64);
radium_number!(usize, AtomicUsize);
radium_number!(isize, AtomicIsize);
radium_atomic!(bool, AtomicBool);
radium_bits!(bool, AtomicBool);

impl<T> Radium for AtomicPtr<T> {
    type Item = *mut T;

    #[inline]
    fn new(value: *mut T) -> Self {
        Self::new(value)
    }

    #[inline]
    fn get_mut(&mut self) -> &mut *mut T {
        Self::get_mut(self)
    }

    #[inline]
    fn into_inner(self) -> *mut T {
        Self::into_inner(self)
    }

    #[inline]
    fn load(&self, order: Ordering) -> *mut T {
        Self::load(self, order)
    }

    #[inline]
    fn store(&self, value: *mut T, order: Ordering) {
        Self::store(self, value, order);
    }

    #[inline]
    fn swap(&self, value: *mut T, order: Ordering) -> *mut T {
        Self::swap(self, value, order)
    }

    #[inline]
    fn compare_exchange(&self, current: *mut T, new: *mut T, success: Ordering, failure: Ordering) -> Result<*mut T, *mut T> {
        Self::compare_exchange(self, current, new, success, failure)
    }

    #[inline]
    fn compare_exchange_weak(&self, current: *mut T, new: *mut T, success: Ordering, failure: Ordering) -> Result<*mut T, *mut T> {
        Self::compare_exchange_weak(self, current, new, success, failure)
    }

    #[inline]
    fn fetch_update<F>(&self, set_order: Ordering, fetch_order: Ordering, function: F) -> Result<*mut T, *mut T>
    where
        F: FnMut(*mut T) -> Option<*mut T>,
    {
        Self::try_update(self, set_order, fetch_order, function)
    }
}

impl<T> Radium for core::cell::Cell<*mut T> {
    type Item = *mut T;

    #[inline]
    fn new(value: *mut T) -> Self {
        Self::new(value)
    }

    #[inline]
    fn get_mut(&mut self) -> &mut *mut T {
        Self::get_mut(self)
    }

    #[inline]
    fn into_inner(self) -> *mut T {
        Self::into_inner(self)
    }

    #[inline]
    fn load(&self, _order: Ordering) -> *mut T {
        self.get()
    }

    #[inline]
    fn store(&self, value: *mut T, _order: Ordering) {
        self.set(value);
    }

    #[inline]
    fn swap(&self, value: *mut T, _order: Ordering) -> *mut T {
        self.replace(value)
    }

    #[inline]
    fn compare_exchange(&self, current: *mut T, new: *mut T, _success: Ordering, _failure: Ordering) -> Result<*mut T, *mut T> {
        let previous = self.get();
        if core::ptr::eq(previous, current) {
            self.set(new);
            Ok(previous)
        } else {
            Err(previous)
        }
    }

    #[inline]
    fn compare_exchange_weak(&self, current: *mut T, new: *mut T, success: Ordering, failure: Ordering) -> Result<*mut T, *mut T> {
        Radium::compare_exchange(self, current, new, success, failure)
    }

    #[inline]
    fn fetch_update<F>(&self, _set_order: Ordering, _fetch_order: Ordering, mut function: F) -> Result<*mut T, *mut T>
    where
        F: FnMut(*mut T) -> Option<*mut T>,
    {
        let previous = self.get();
        match function(previous) {
            Some(next) => {
                self.set(next);
                Ok(previous)
            }
            None => Err(previous),
        }
    }
}
