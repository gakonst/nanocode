use core::num::NonZero;
use parking_lot::lock_api::{
    GetThreadId as _, RawMutex as _, RawRwLock as _, RawRwLockDowngrade as _,
    RawRwLockRecursive as _, RawRwLockUpgrade as _, RawRwLockUpgradeDowngrade as _,
};

use super::api;

pub struct RawMutex(parking_lot::RawMutex);

unsafe impl api::RawMutex for RawMutex {
    #[allow(clippy::declare_interior_mutable_const)]
    const INIT: Self = Self(parking_lot::RawMutex::INIT);
    type GuardMarker = api::GuardNoSend;

    #[inline]
    fn lock(&self) {
        self.0.lock();
    }

    #[inline]
    fn try_lock(&self) -> bool {
        self.0.try_lock()
    }

    #[inline]
    unsafe fn unlock(&self) {
        unsafe { self.0.unlock() }
    }

    #[inline]
    fn is_locked(&self) -> bool {
        self.0.is_locked()
    }
}

pub struct RawRwLock(parking_lot::RawRwLock);

unsafe impl api::RawRwLock for RawRwLock {
    #[allow(clippy::declare_interior_mutable_const)]
    const INIT: Self = Self(parking_lot::RawRwLock::INIT);
    type GuardMarker = api::GuardNoSend;

    #[inline]
    fn lock_shared(&self) {
        self.0.lock_shared();
    }

    #[inline]
    fn try_lock_shared(&self) -> bool {
        self.0.try_lock_shared()
    }

    #[inline]
    unsafe fn unlock_shared(&self) {
        unsafe { self.0.unlock_shared() }
    }

    #[inline]
    fn lock_exclusive(&self) {
        self.0.lock_exclusive();
    }

    #[inline]
    fn try_lock_exclusive(&self) -> bool {
        self.0.try_lock_exclusive()
    }

    #[inline]
    unsafe fn unlock_exclusive(&self) {
        unsafe { self.0.unlock_exclusive() }
    }

    #[inline]
    fn is_locked(&self) -> bool {
        self.0.is_locked()
    }

    #[inline]
    fn is_locked_exclusive(&self) -> bool {
        self.0.is_locked_exclusive()
    }
}

unsafe impl api::RawRwLockDowngrade for RawRwLock {
    #[inline]
    unsafe fn downgrade(&self) {
        unsafe { self.0.downgrade() }
    }
}

unsafe impl api::RawRwLockRecursive for RawRwLock {
    #[inline]
    fn lock_shared_recursive(&self) {
        self.0.lock_shared_recursive();
    }

    #[inline]
    fn try_lock_shared_recursive(&self) -> bool {
        self.0.try_lock_shared_recursive()
    }
}

unsafe impl api::RawRwLockUpgrade for RawRwLock {
    #[inline]
    fn lock_upgradable(&self) {
        self.0.lock_upgradable();
    }

    #[inline]
    fn try_lock_upgradable(&self) -> bool {
        self.0.try_lock_upgradable()
    }

    #[inline]
    unsafe fn unlock_upgradable(&self) {
        unsafe { self.0.unlock_upgradable() }
    }

    #[inline]
    unsafe fn upgrade(&self) {
        unsafe { self.0.upgrade() }
    }

    #[inline]
    unsafe fn try_upgrade(&self) -> bool {
        unsafe { self.0.try_upgrade() }
    }
}

unsafe impl api::RawRwLockUpgradeDowngrade for RawRwLock {
    #[inline]
    unsafe fn downgrade_upgradable(&self) {
        unsafe { self.0.downgrade_upgradable() }
    }

    #[inline]
    unsafe fn downgrade_to_upgradable(&self) {
        unsafe { self.0.downgrade_to_upgradable() }
    }
}

pub struct RawThreadId(parking_lot::RawThreadId);

unsafe impl api::GetThreadId for RawThreadId {
    const INIT: Self = Self(parking_lot::RawThreadId::INIT);

    #[inline]
    fn nonzero_thread_id(&self) -> NonZero<usize> {
        self.0.nonzero_thread_id()
    }
}
