#![allow(clippy::missing_safety_doc)]

use core::{
    cell::UnsafeCell,
    fmt,
    marker::PhantomData,
    mem,
    num::NonZero,
    ops::{Deref, DerefMut},
    ptr::NonNull,
};

pub struct GuardSend(());

pub struct GuardNoSend(PhantomData<*mut ()>);

unsafe impl Sync for GuardNoSend {}

pub unsafe trait RawMutex {
    const INIT: Self;
    type GuardMarker;
    fn lock(&self);
    fn try_lock(&self) -> bool;
    unsafe fn unlock(&self);
    fn is_locked(&self) -> bool;
}

pub unsafe trait RawRwLock {
    const INIT: Self;
    type GuardMarker;
    fn lock_shared(&self);
    fn try_lock_shared(&self) -> bool;
    unsafe fn unlock_shared(&self);
    fn lock_exclusive(&self);
    fn try_lock_exclusive(&self) -> bool;
    unsafe fn unlock_exclusive(&self);
    fn is_locked(&self) -> bool;
    fn is_locked_exclusive(&self) -> bool {
        let acquired = self.try_lock_shared();
        if acquired {
            unsafe { self.unlock_shared() };
        }
        !acquired
    }
}

pub unsafe trait RawRwLockDowngrade: RawRwLock {
    unsafe fn downgrade(&self);
}

pub unsafe trait RawRwLockRecursive: RawRwLock {
    fn lock_shared_recursive(&self);
    fn try_lock_shared_recursive(&self) -> bool;
}

pub unsafe trait RawRwLockUpgrade: RawRwLock {
    fn lock_upgradable(&self);
    fn try_lock_upgradable(&self) -> bool;
    unsafe fn unlock_upgradable(&self);
    unsafe fn upgrade(&self);
    unsafe fn try_upgrade(&self) -> bool;
}

pub unsafe trait RawRwLockUpgradeDowngrade: RawRwLockUpgrade + RawRwLockDowngrade {
    unsafe fn downgrade_upgradable(&self);
    unsafe fn downgrade_to_upgradable(&self);
}

pub unsafe trait GetThreadId {
    const INIT: Self;
    fn nonzero_thread_id(&self) -> NonZero<usize>;
}

struct LockedPlaceholder;

impl fmt::Debug for LockedPlaceholder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<locked>")
    }
}

pub struct Mutex<R, T: ?Sized> {
    raw: R,
    data: UnsafeCell<T>,
}

unsafe impl<R: RawMutex + Send, T: ?Sized + Send> Send for Mutex<R, T> {}
unsafe impl<R: RawMutex + Sync, T: ?Sized + Send> Sync for Mutex<R, T> {}

impl<R: RawMutex, T> Mutex<R, T> {
    #[inline]
    pub const fn new(value: T) -> Self {
        Self {
            raw: R::INIT,
            data: UnsafeCell::new(value),
        }
    }

    #[inline]
    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }
}

impl<R: RawMutex, T: ?Sized> Mutex<R, T> {
    #[inline]
    unsafe fn make_guard_unchecked(&self) -> MutexGuard<'_, R, T> {
        MutexGuard {
            mutex: self,
            marker: PhantomData,
        }
    }

    #[inline]
    pub fn lock(&self) -> MutexGuard<'_, R, T> {
        self.raw.lock();
        unsafe { self.make_guard_unchecked() }
    }

    #[inline]
    pub fn try_lock(&self) -> Option<MutexGuard<'_, R, T>> {
        if self.raw.try_lock() {
            Some(unsafe { self.make_guard_unchecked() })
        } else {
            None
        }
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.raw.is_locked()
    }

    #[inline]
    pub fn get_mut(&mut self) -> &mut T {
        self.data.get_mut()
    }

    #[inline]
    pub unsafe fn raw(&self) -> &R {
        &self.raw
    }

    #[inline]
    pub fn data_ptr(&self) -> *mut T {
        self.data.get()
    }
}

impl<R: RawMutex, T: Default> Default for Mutex<R, T> {
    #[inline]
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<R: RawMutex, T> From<T> for Mutex<R, T> {
    #[inline]
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

impl<R: RawMutex, T: ?Sized + fmt::Debug> fmt::Debug for Mutex<R, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.try_lock() {
            Some(guard) => f.debug_struct("Mutex").field("data", &&*guard).finish(),
            None => f
                .debug_struct("Mutex")
                .field("data", &LockedPlaceholder)
                .finish(),
        }
    }
}

#[must_use = "if unused the Mutex will immediately unlock"]
pub struct MutexGuard<'a, R: RawMutex, T: ?Sized> {
    mutex: &'a Mutex<R, T>,
    marker: PhantomData<(&'a mut T, R::GuardMarker)>,
}

unsafe impl<R: RawMutex + Sync, T: ?Sized + Sync> Sync for MutexGuard<'_, R, T> {}

impl<'a, R: RawMutex, T: ?Sized> MutexGuard<'a, R, T> {
    #[inline]
    pub const fn mutex(s: &Self) -> &'a Mutex<R, T> {
        s.mutex
    }

    #[inline]
    pub fn map<U: ?Sized, F>(s: Self, f: F) -> MappedMutexGuard<'a, R, U>
    where
        F: FnOnce(&mut T) -> &mut U,
    {
        let raw = &s.mutex.raw;
        let data = f(unsafe { &mut *s.mutex.data.get() });
        mem::forget(s);
        MappedMutexGuard {
            raw,
            data,
            marker: PhantomData,
        }
    }

    #[inline]
    pub fn try_map<U: ?Sized, F>(s: Self, f: F) -> Result<MappedMutexGuard<'a, R, U>, Self>
    where
        F: FnOnce(&mut T) -> Option<&mut U>,
    {
        let raw = &s.mutex.raw;
        let data = match f(unsafe { &mut *s.mutex.data.get() }) {
            Some(data) => data,
            None => return Err(s),
        };
        mem::forget(s);
        Ok(MappedMutexGuard {
            raw,
            data,
            marker: PhantomData,
        })
    }
}

impl<R: RawMutex, T: ?Sized> Deref for MutexGuard<'_, R, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        unsafe { &*self.mutex.data.get() }
    }
}

impl<R: RawMutex, T: ?Sized> DerefMut for MutexGuard<'_, R, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<R: RawMutex, T: ?Sized> Drop for MutexGuard<'_, R, T> {
    #[inline]
    fn drop(&mut self) {
        unsafe { self.mutex.raw.unlock() };
    }
}

impl<R: RawMutex, T: fmt::Debug + ?Sized> fmt::Debug for MutexGuard<'_, R, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<R: RawMutex, T: fmt::Display + ?Sized> fmt::Display for MutexGuard<'_, R, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

#[must_use = "if unused the Mutex will immediately unlock"]
pub struct MappedMutexGuard<'a, R: RawMutex, T: ?Sized> {
    raw: &'a R,
    data: *mut T,
    marker: PhantomData<&'a mut T>,
}

unsafe impl<R: RawMutex + Sync, T: ?Sized + Sync> Sync for MappedMutexGuard<'_, R, T> {}
unsafe impl<R: RawMutex, T: ?Sized + Send> Send for MappedMutexGuard<'_, R, T> where
    R::GuardMarker: Send
{
}

impl<'a, R: RawMutex, T: ?Sized> MappedMutexGuard<'a, R, T> {
    #[inline]
    pub fn map<U: ?Sized, F>(s: Self, f: F) -> MappedMutexGuard<'a, R, U>
    where
        F: FnOnce(&mut T) -> &mut U,
    {
        let raw = s.raw;
        let data = f(unsafe { &mut *s.data });
        mem::forget(s);
        MappedMutexGuard {
            raw,
            data,
            marker: PhantomData,
        }
    }
}

impl<R: RawMutex, T: ?Sized> Deref for MappedMutexGuard<'_, R, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        unsafe { &*self.data }
    }
}

impl<R: RawMutex, T: ?Sized> DerefMut for MappedMutexGuard<'_, R, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.data }
    }
}

impl<R: RawMutex, T: ?Sized> Drop for MappedMutexGuard<'_, R, T> {
    #[inline]
    fn drop(&mut self) {
        unsafe { self.raw.unlock() };
    }
}

impl<R: RawMutex, T: fmt::Debug + ?Sized> fmt::Debug for MappedMutexGuard<'_, R, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<R: RawMutex, T: fmt::Display + ?Sized> fmt::Display for MappedMutexGuard<'_, R, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

pub struct RwLock<R, T: ?Sized> {
    raw: R,
    data: UnsafeCell<T>,
}

unsafe impl<R: RawRwLock + Send, T: ?Sized + Send> Send for RwLock<R, T> {}
unsafe impl<R: RawRwLock + Sync, T: ?Sized + Send + Sync> Sync for RwLock<R, T> {}

impl<R: RawRwLock, T> RwLock<R, T> {
    #[inline]
    pub const fn new(value: T) -> Self {
        Self {
            raw: R::INIT,
            data: UnsafeCell::new(value),
        }
    }

    #[inline]
    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }
}

impl<R: RawRwLock, T: ?Sized> RwLock<R, T> {
    #[inline]
    unsafe fn make_read_guard_unchecked(&self) -> RwLockReadGuard<'_, R, T> {
        RwLockReadGuard {
            rwlock: self,
            marker: PhantomData,
        }
    }

    #[inline]
    unsafe fn make_write_guard_unchecked(&self) -> RwLockWriteGuard<'_, R, T> {
        RwLockWriteGuard {
            rwlock: self,
            marker: PhantomData,
        }
    }

    #[inline]
    pub fn read(&self) -> RwLockReadGuard<'_, R, T> {
        self.raw.lock_shared();
        unsafe { self.make_read_guard_unchecked() }
    }

    #[inline]
    pub fn try_read(&self) -> Option<RwLockReadGuard<'_, R, T>> {
        if self.raw.try_lock_shared() {
            Some(unsafe { self.make_read_guard_unchecked() })
        } else {
            None
        }
    }

    #[inline]
    pub fn write(&self) -> RwLockWriteGuard<'_, R, T> {
        self.raw.lock_exclusive();
        unsafe { self.make_write_guard_unchecked() }
    }

    #[inline]
    pub fn try_write(&self) -> Option<RwLockWriteGuard<'_, R, T>> {
        if self.raw.try_lock_exclusive() {
            Some(unsafe { self.make_write_guard_unchecked() })
        } else {
            None
        }
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.raw.is_locked()
    }

    #[inline]
    pub fn is_locked_exclusive(&self) -> bool {
        self.raw.is_locked_exclusive()
    }

    #[inline]
    pub fn get_mut(&mut self) -> &mut T {
        self.data.get_mut()
    }

    #[inline]
    pub unsafe fn raw(&self) -> &R {
        &self.raw
    }

    #[inline]
    pub fn data_ptr(&self) -> *mut T {
        self.data.get()
    }
}

impl<R: RawRwLockRecursive, T: ?Sized> RwLock<R, T> {
    #[inline]
    pub fn read_recursive(&self) -> RwLockReadGuard<'_, R, T> {
        self.raw.lock_shared_recursive();
        unsafe { self.make_read_guard_unchecked() }
    }

    #[inline]
    pub fn try_read_recursive(&self) -> Option<RwLockReadGuard<'_, R, T>> {
        if self.raw.try_lock_shared_recursive() {
            Some(unsafe { self.make_read_guard_unchecked() })
        } else {
            None
        }
    }
}

impl<R: RawRwLockUpgrade, T: ?Sized> RwLock<R, T> {
    #[inline]
    pub fn upgradable_read(&self) -> RwLockUpgradableReadGuard<'_, R, T> {
        self.raw.lock_upgradable();
        RwLockUpgradableReadGuard {
            rwlock: self,
            marker: PhantomData,
        }
    }
}

impl<R: RawRwLock, T: Default> Default for RwLock<R, T> {
    #[inline]
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<R: RawRwLock, T> From<T> for RwLock<R, T> {
    #[inline]
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

impl<R: RawRwLock, T: ?Sized + fmt::Debug> fmt::Debug for RwLock<R, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.try_read() {
            Some(guard) => f.debug_struct("RwLock").field("data", &&*guard).finish(),
            None => f
                .debug_struct("RwLock")
                .field("data", &LockedPlaceholder)
                .finish(),
        }
    }
}

#[must_use = "if unused the RwLock will immediately unlock"]
pub struct RwLockReadGuard<'a, R: RawRwLock, T: ?Sized> {
    rwlock: &'a RwLock<R, T>,
    marker: PhantomData<(&'a T, R::GuardMarker)>,
}

unsafe impl<R: RawRwLock + Sync, T: Sync + ?Sized> Sync for RwLockReadGuard<'_, R, T> {}

impl<'a, R: RawRwLock, T: ?Sized> RwLockReadGuard<'a, R, T> {
    #[inline]
    pub const fn rwlock(s: &Self) -> &'a RwLock<R, T> {
        s.rwlock
    }

    #[inline]
    pub fn map<U: ?Sized, F>(s: Self, f: F) -> MappedRwLockReadGuard<'a, R, U>
    where
        F: FnOnce(&T) -> &U,
    {
        let raw = &s.rwlock.raw;
        let data = f(unsafe { &*s.rwlock.data.get() });
        mem::forget(s);
        MappedRwLockReadGuard {
            raw,
            data: NonNull::from(data),
            marker: PhantomData,
        }
    }

    #[inline]
    pub fn try_map<U: ?Sized, F>(s: Self, f: F) -> Result<MappedRwLockReadGuard<'a, R, U>, Self>
    where
        F: FnOnce(&T) -> Option<&U>,
    {
        let raw = &s.rwlock.raw;
        let data = match f(unsafe { &*s.rwlock.data.get() }) {
            Some(data) => NonNull::from(data),
            None => return Err(s),
        };
        mem::forget(s);
        Ok(MappedRwLockReadGuard {
            raw,
            data,
            marker: PhantomData,
        })
    }
}

impl<R: RawRwLock, T: ?Sized> Deref for RwLockReadGuard<'_, R, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        unsafe { &*self.rwlock.data.get() }
    }
}

impl<R: RawRwLock, T: ?Sized> Drop for RwLockReadGuard<'_, R, T> {
    #[inline]
    fn drop(&mut self) {
        unsafe { self.rwlock.raw.unlock_shared() };
    }
}

impl<R: RawRwLock, T: fmt::Debug + ?Sized> fmt::Debug for RwLockReadGuard<'_, R, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<R: RawRwLock, T: fmt::Display + ?Sized> fmt::Display for RwLockReadGuard<'_, R, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

#[must_use = "if unused the RwLock will immediately unlock"]
pub struct RwLockWriteGuard<'a, R: RawRwLock, T: ?Sized> {
    rwlock: &'a RwLock<R, T>,
    marker: PhantomData<(&'a mut T, R::GuardMarker)>,
}

unsafe impl<R: RawRwLock + Sync, T: Sync + ?Sized> Sync for RwLockWriteGuard<'_, R, T> {}

impl<'a, R: RawRwLock, T: ?Sized> RwLockWriteGuard<'a, R, T> {
    #[inline]
    pub const fn rwlock(s: &Self) -> &'a RwLock<R, T> {
        s.rwlock
    }

    #[inline]
    pub fn map<U: ?Sized, F>(s: Self, f: F) -> MappedRwLockWriteGuard<'a, R, U>
    where
        F: FnOnce(&mut T) -> &mut U,
    {
        let raw = &s.rwlock.raw;
        let data = f(unsafe { &mut *s.rwlock.data.get() });
        mem::forget(s);
        MappedRwLockWriteGuard {
            raw,
            data: NonNull::from(data),
            marker: PhantomData,
        }
    }

    #[inline]
    pub fn try_map<U: ?Sized, F>(s: Self, f: F) -> Result<MappedRwLockWriteGuard<'a, R, U>, Self>
    where
        F: FnOnce(&mut T) -> Option<&mut U>,
    {
        let raw = &s.rwlock.raw;
        let data = match f(unsafe { &mut *s.rwlock.data.get() }) {
            Some(data) => NonNull::from(data),
            None => return Err(s),
        };
        mem::forget(s);
        Ok(MappedRwLockWriteGuard {
            raw,
            data,
            marker: PhantomData,
        })
    }
}

impl<'a, R: RawRwLockDowngrade, T: ?Sized> RwLockWriteGuard<'a, R, T> {
    #[inline]
    pub fn downgrade(s: Self) -> RwLockReadGuard<'a, R, T> {
        unsafe { s.rwlock.raw.downgrade() };
        let rwlock = s.rwlock;
        mem::forget(s);
        RwLockReadGuard {
            rwlock,
            marker: PhantomData,
        }
    }
}

impl<R: RawRwLock, T: ?Sized> Deref for RwLockWriteGuard<'_, R, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        unsafe { &*self.rwlock.data.get() }
    }
}

impl<R: RawRwLock, T: ?Sized> DerefMut for RwLockWriteGuard<'_, R, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.rwlock.data.get() }
    }
}

impl<R: RawRwLock, T: ?Sized> Drop for RwLockWriteGuard<'_, R, T> {
    #[inline]
    fn drop(&mut self) {
        unsafe { self.rwlock.raw.unlock_exclusive() };
    }
}

impl<R: RawRwLock, T: fmt::Debug + ?Sized> fmt::Debug for RwLockWriteGuard<'_, R, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<R: RawRwLock, T: fmt::Display + ?Sized> fmt::Display for RwLockWriteGuard<'_, R, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

#[must_use = "if unused the RwLock will immediately unlock"]
pub struct RwLockUpgradableReadGuard<'a, R: RawRwLockUpgrade, T: ?Sized> {
    rwlock: &'a RwLock<R, T>,
    marker: PhantomData<(&'a T, R::GuardMarker)>,
}

unsafe impl<R: RawRwLockUpgrade + Sync, T: Sync + ?Sized> Sync
    for RwLockUpgradableReadGuard<'_, R, T>
{
}

impl<'a, R: RawRwLockUpgrade, T: ?Sized> RwLockUpgradableReadGuard<'a, R, T> {
    #[inline]
    pub fn upgrade(s: Self) -> RwLockWriteGuard<'a, R, T> {
        unsafe { s.rwlock.raw.upgrade() };
        let rwlock = s.rwlock;
        mem::forget(s);
        RwLockWriteGuard {
            rwlock,
            marker: PhantomData,
        }
    }
}

impl<R: RawRwLockUpgrade, T: ?Sized> Deref for RwLockUpgradableReadGuard<'_, R, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        unsafe { &*self.rwlock.data.get() }
    }
}

impl<R: RawRwLockUpgrade, T: ?Sized> Drop for RwLockUpgradableReadGuard<'_, R, T> {
    #[inline]
    fn drop(&mut self) {
        unsafe { self.rwlock.raw.unlock_upgradable() };
    }
}

impl<R: RawRwLockUpgrade, T: fmt::Debug + ?Sized> fmt::Debug
    for RwLockUpgradableReadGuard<'_, R, T>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

#[must_use = "if unused the RwLock will immediately unlock"]
pub struct MappedRwLockReadGuard<'a, R: RawRwLock, T: ?Sized> {
    raw: &'a R,
    data: NonNull<T>,
    marker: PhantomData<&'a T>,
}

unsafe impl<R: RawRwLock + Sync, T: ?Sized + Sync> Sync for MappedRwLockReadGuard<'_, R, T> {}
unsafe impl<R: RawRwLock, T: ?Sized + Sync> Send for MappedRwLockReadGuard<'_, R, T> where
    R::GuardMarker: Send
{
}

impl<'a, R: RawRwLock, T: ?Sized> MappedRwLockReadGuard<'a, R, T> {
    #[inline]
    pub fn map<U: ?Sized, F>(s: Self, f: F) -> MappedRwLockReadGuard<'a, R, U>
    where
        F: FnOnce(&T) -> &U,
    {
        let raw = s.raw;
        let data = f(unsafe { s.data.as_ref() });
        mem::forget(s);
        MappedRwLockReadGuard {
            raw,
            data: NonNull::from(data),
            marker: PhantomData,
        }
    }
}

impl<R: RawRwLock, T: ?Sized> Deref for MappedRwLockReadGuard<'_, R, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        unsafe { self.data.as_ref() }
    }
}

impl<R: RawRwLock, T: ?Sized> Drop for MappedRwLockReadGuard<'_, R, T> {
    #[inline]
    fn drop(&mut self) {
        unsafe { self.raw.unlock_shared() };
    }
}

impl<R: RawRwLock, T: fmt::Debug + ?Sized> fmt::Debug for MappedRwLockReadGuard<'_, R, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<R: RawRwLock, T: fmt::Display + ?Sized> fmt::Display for MappedRwLockReadGuard<'_, R, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

#[must_use = "if unused the RwLock will immediately unlock"]
pub struct MappedRwLockWriteGuard<'a, R: RawRwLock, T: ?Sized> {
    raw: &'a R,
    data: NonNull<T>,
    marker: PhantomData<&'a mut T>,
}

unsafe impl<R: RawRwLock + Sync, T: ?Sized + Sync> Sync for MappedRwLockWriteGuard<'_, R, T> {}
unsafe impl<R: RawRwLock, T: ?Sized + Send> Send for MappedRwLockWriteGuard<'_, R, T> where
    R::GuardMarker: Send
{
}

impl<'a, R: RawRwLock, T: ?Sized> MappedRwLockWriteGuard<'a, R, T> {
    #[inline]
    pub fn map<U: ?Sized, F>(mut s: Self, f: F) -> MappedRwLockWriteGuard<'a, R, U>
    where
        F: FnOnce(&mut T) -> &mut U,
    {
        let raw = s.raw;
        let data = f(unsafe { s.data.as_mut() });
        mem::forget(s);
        MappedRwLockWriteGuard {
            raw,
            data: NonNull::from(data),
            marker: PhantomData,
        }
    }
}

impl<R: RawRwLock, T: ?Sized> Deref for MappedRwLockWriteGuard<'_, R, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        unsafe { self.data.as_ref() }
    }
}

impl<R: RawRwLock, T: ?Sized> DerefMut for MappedRwLockWriteGuard<'_, R, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        unsafe { self.data.as_mut() }
    }
}

impl<R: RawRwLock, T: ?Sized> Drop for MappedRwLockWriteGuard<'_, R, T> {
    #[inline]
    fn drop(&mut self) {
        unsafe { self.raw.unlock_exclusive() };
    }
}

impl<R: RawRwLock, T: fmt::Debug + ?Sized> fmt::Debug for MappedRwLockWriteGuard<'_, R, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<R: RawRwLock, T: fmt::Display + ?Sized> fmt::Display for MappedRwLockWriteGuard<'_, R, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}
