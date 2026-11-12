use crate::atomic::AtomicStatic;
use std::{
    cell::Cell,
    fmt,
    marker::PhantomData,
    mem::ManuallyDrop,
    ops::Deref,
    ptr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering},
    },
};

const GUARD_SLOTS: usize = 8;
const FREE: usize = 0;
const PENDING_TAG: usize = 0b01;
const PENDING_PAID_TAG: usize = 0b10;
const DEBT_PAID_TAG: usize = 0b11;
const TAG_MASK: usize = 0b11;

#[repr(align(128))]
struct Node {
    claimed: AtomicBool,
    next: AtomicStatic<Self>,
    guards: [AtomicUsize; GUARD_SLOTS],
    transient: AtomicUsize,
}

impl Node {
    fn slots(&self) -> impl Iterator<Item = &AtomicUsize> {
        self.guards.iter().chain(std::iter::once(&self.transient))
    }
}

static NODES: AtomicStatic<Node> = AtomicStatic::empty();

fn nodes() -> impl Iterator<Item = &'static Node> {
    std::iter::successors(NODES.load(), |node| node.next.load())
}

fn claim() -> &'static Node {
    if let Some(node) = nodes().find(|node| {
        node.claimed
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
            .is_ok()
    }) {
        return node;
    }
    let node: &'static Node = Box::leak(Box::new(Node {
        claimed: AtomicBool::new(true),
        next: AtomicStatic::empty(),
        guards: std::array::from_fn(|_| AtomicUsize::new(FREE)),
        transient: AtomicUsize::new(FREE),
    }));
    let mut head = NODES.load();
    loop {
        node.next.store(head);
        match NODES.compare_exchange(head, Some(node)) {
            Ok(_) => return node,
            Err(current) => head = current,
        }
    }
}

struct Local(Cell<Option<&'static Node>>);

impl Drop for Local {
    fn drop(&mut self) {
        if let Some(node) = self.0.get() {
            node.claimed.store(false, Ordering::SeqCst);
        }
    }
}

thread_local! {
    static LOCAL: Local = const { Local(Cell::new(None)) };
}

fn with_node<R>(work: impl FnOnce(&'static Node) -> R) -> R {
    let local = LOCAL.try_with(|local| {
        local.0.get().unwrap_or_else(|| {
            let node = claim();
            local.0.set(Some(node));
            node
        })
    });
    match local {
        Ok(node) => work(node),
        Err(_) => {
            let node = claim();
            let output = work(node);
            node.claimed.store(false, Ordering::SeqCst);
            output
        }
    }
}

mod sealed {
    pub trait Sealed {}

    pub trait Raw: Sized {
        type Base;

        fn into_raw(self) -> *mut Self::Base;

        fn as_raw(&self) -> *mut Self::Base;

        unsafe fn from_raw(raw: *mut Self::Base) -> Self;
    }
}

use sealed::Raw;

pub trait Pointer: Raw {}

impl<T> Pointer for Arc<T> {}

impl<T> Pointer for Option<Arc<T>> {}

impl<T> Raw for Arc<T> {
    type Base = T;

    fn into_raw(self) -> *mut T {
        Self::into_raw(self).cast_mut()
    }

    fn as_raw(&self) -> *mut T {
        Self::as_ptr(self).cast_mut()
    }

    unsafe fn from_raw(raw: *mut T) -> Self {
        assert!(!raw.is_null(), "a Swap of an Arc never holds null");
        unsafe { Self::from_raw(raw) }
    }
}

impl<T> Raw for Option<Arc<T>> {
    type Base = T;

    fn into_raw(self) -> *mut T {
        self.map_or(ptr::null_mut(), |value| Arc::into_raw(value).cast_mut())
    }

    fn as_raw(&self) -> *mut T {
        self.as_ref()
            .map_or(ptr::null_mut(), |value| Arc::as_ptr(value).cast_mut())
    }

    unsafe fn from_raw(raw: *mut T) -> Self {
        if raw.is_null() {
            return None;
        }
        Some(unsafe { Arc::from_raw(raw) })
    }
}

fn increment<T>(raw: *mut T) {
    assert!(!raw.is_null());
    unsafe { Arc::increment_strong_count(raw) };
}

fn release<T>(raw: *mut T) {
    if !raw.is_null() {
        drop(unsafe { Arc::from_raw(raw) });
    }
}

struct Release(fn(*mut ()));

fn release_erased<T>(raw: *mut ()) {
    release(raw.cast::<T>());
}

struct Releaser<T>(PhantomData<T>);

impl<T> Releaser<T> {
    const RELEASE: &'static Release = &Release(release_erased::<T>);
}

fn debt_paid<T>() -> usize {
    ptr::from_ref(Releaser::<T>::RELEASE).expose_provenance() | DEBT_PAID_TAG
}

fn pending_paid<T>(raw: *mut T) -> usize {
    raw.expose_provenance() | PENDING_PAID_TAG
}

const fn is_debt_paid(seen: usize) -> bool {
    seen & TAG_MASK == DEBT_PAID_TAG
}

fn release_paid_debt(seen: usize, raw: *mut ()) {
    assert!(is_debt_paid(seen), "only a writer's payment replaces a debt");
    let release: &'static Release =
        unsafe { &*ptr::with_exposed_provenance::<Release>(seen & !TAG_MASK) };
    (release.0)(raw);
}

enum Protected<T> {
    Debt(*mut T),
    Owned(*mut T),
}

struct Storage<T> {
    pointer: AtomicPtr<T>,
    owns: PhantomData<Option<Arc<T>>>,
}

impl<T> Storage<T> {
    fn new(raw: *mut T) -> Self {
        assert_eq!(raw.addr() & TAG_MASK, 0, "an Arc's value is aligned past the tag bits");
        Self {
            pointer: AtomicPtr::new(raw),
            owns: PhantomData,
        }
    }

    const fn empty() -> Self {
        Self {
            pointer: AtomicPtr::new(ptr::null_mut()),
            owns: PhantomData,
        }
    }

    fn pending(&self) -> usize {
        ptr::from_ref(&self.pointer).addr() | PENDING_TAG
    }

    fn protect(&self, slot: &AtomicUsize) -> Protected<T> {
        let current = self.pointer.load(Ordering::SeqCst);
        if current.is_null() {
            return Protected::Owned(current);
        }
        slot.store(current.addr(), Ordering::SeqCst);
        if self.pointer.load(Ordering::SeqCst) == current {
            return Protected::Debt(current);
        }
        if let Err(seen) =
            slot.compare_exchange(current.addr(), FREE, Ordering::SeqCst, Ordering::SeqCst)
        {
            slot.store(FREE, Ordering::SeqCst);
            release_paid_debt(seen, current.cast());
        }
        self.protect_pending(slot)
    }

    fn protect_pending(&self, slot: &AtomicUsize) -> Protected<T> {
        let pending = self.pending();
        slot.store(pending, Ordering::SeqCst);
        let current = self.pointer.load(Ordering::SeqCst);
        match slot.compare_exchange(pending, current.addr(), Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) if current.is_null() => Protected::Owned(current),
            Ok(_) => Protected::Debt(current),
            Err(seen) => {
                assert_eq!(seen & TAG_MASK, PENDING_PAID_TAG, "only a writer helps a pending load");
                slot.store(FREE, Ordering::SeqCst);
                Protected::Owned(ptr::with_exposed_provenance_mut(seen & !TAG_MASK))
            }
        }
    }

    fn load_owned(&self) -> *mut T {
        with_node(|node| match self.protect(&node.transient) {
            Protected::Owned(raw) => raw,
            Protected::Debt(raw) => {
                increment(raw);
                let seen = node.transient.swap(FREE, Ordering::SeqCst);
                if seen != raw.addr() {
                    release_paid_debt(seen, raw.cast());
                }
                raw
            }
        })
    }

    fn load_guard<P: Pointer<Base = T>>(&self) -> Guard<P> {
        with_node(|node| {
            let Some(slot) = node
                .guards
                .iter()
                .find(|slot| slot.load(Ordering::Relaxed) == FREE)
            else {
                return Guard::owned(self.load_owned());
            };
            match self.protect(slot) {
                Protected::Owned(raw) => Guard::owned(raw),
                Protected::Debt(raw) => Guard {
                    value: ManuallyDrop::new(unsafe { P::from_raw(raw) }),
                    debt: Some(slot),
                },
            }
        })
    }

    fn swap(&self, new: *mut T) -> *mut T {
        assert_eq!(new.addr() & TAG_MASK, 0, "an Arc's value is aligned past the tag bits");
        let old = self.pointer.swap(new, Ordering::SeqCst);
        self.settle(old);
        old
    }

    fn compare_exchange(&self, current: *mut T, new: *mut T) -> bool {
        assert_eq!(new.addr() & TAG_MASK, 0, "an Arc's value is aligned past the tag bits");
        let exchanged = self
            .pointer
            .compare_exchange(current, new, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok();
        if exchanged {
            self.settle(current);
        }
        exchanged
    }

    fn settle(&self, old: *mut T) {
        if old.is_null() {
            return;
        }
        let pending = self.pending();
        for slot in nodes().flat_map(Node::slots) {
            let mut seen = slot.load(Ordering::SeqCst);
            if seen == pending {
                let fresh = self.load_owned();
                match slot.compare_exchange(
                    pending,
                    pending_paid(fresh),
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                ) {
                    Ok(_) => continue,
                    Err(now) => {
                        release(fresh);
                        seen = now;
                    }
                }
            }
            pay(slot, seen, old);
        }
    }

    fn settle_debts(&mut self) -> *mut T {
        let old = *self.pointer.get_mut();
        if !old.is_null() {
            for slot in nodes().flat_map(Node::slots) {
                pay(slot, slot.load(Ordering::SeqCst), old);
            }
        }
        old
    }
}

fn pay<T>(slot: &AtomicUsize, seen: usize, old: *mut T) {
    if seen != old.addr() {
        return;
    }
    increment(old);
    if slot
        .compare_exchange(seen, debt_paid::<T>(), Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        release(old);
    }
}

pub struct Guard<P: Pointer> {
    value: ManuallyDrop<P>,
    debt: Option<&'static AtomicUsize>,
}

impl<P: Pointer> Guard<P> {
    fn owned(raw: *mut P::Base) -> Self {
        Self {
            value: ManuallyDrop::new(unsafe { P::from_raw(raw) }),
            debt: None,
        }
    }

    pub const fn from_inner(value: P) -> Self {
        Self {
            value: ManuallyDrop::new(value),
            debt: None,
        }
    }

    pub fn into_inner(guard: Self) -> P {
        let mut guard = ManuallyDrop::new(guard);
        let raw = guard.value.as_raw();
        if let Some(slot) = guard.debt.take() {
            increment(raw);
            let seen = slot.swap(FREE, Ordering::SeqCst);
            if seen != raw.addr() {
                release_paid_debt(seen, raw.cast());
            }
        }
        unsafe { ManuallyDrop::take(&mut guard.value) }
    }
}

impl<P: Pointer> Drop for Guard<P> {
    fn drop(&mut self) {
        if let Some(slot) = self.debt.take() {
            let raw = self.value.as_raw();
            let seen = slot.swap(FREE, Ordering::SeqCst);
            if seen == raw.addr() {
                return;
            }
            assert!(is_debt_paid(seen), "only a writer's payment replaces a debt");
        }
        unsafe { ManuallyDrop::drop(&mut self.value) };
    }
}

impl<P: Pointer> Deref for Guard<P> {
    type Target = P;

    fn deref(&self) -> &P {
        &self.value
    }
}

impl<P: Pointer> From<P> for Guard<P> {
    fn from(value: P) -> Self {
        Self::from_inner(value)
    }
}

impl<P: Pointer + Default> Default for Guard<P> {
    fn default() -> Self {
        Self::from_inner(P::default())
    }
}

impl<P: Pointer + fmt::Debug> fmt::Debug for Guard<P> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.value.fmt(formatter)
    }
}

impl<P: Pointer + fmt::Display> fmt::Display for Guard<P> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.value.fmt(formatter)
    }
}

pub trait AsRaw<T>: sealed::Sealed {
    fn as_raw(&self) -> *mut T;
}

impl<T> sealed::Sealed for &Arc<T> {}

impl<T> AsRaw<T> for &Arc<T> {
    fn as_raw(&self) -> *mut T {
        Arc::as_ptr(self).cast_mut()
    }
}

impl<T> sealed::Sealed for &Option<Arc<T>> {}

impl<T> AsRaw<T> for &Option<Arc<T>> {
    fn as_raw(&self) -> *mut T {
        Raw::as_raw(*self)
    }
}

impl<P: Pointer> sealed::Sealed for &Guard<P> {}

impl<P: Pointer> AsRaw<P::Base> for &Guard<P> {
    fn as_raw(&self) -> *mut P::Base {
        self.value.as_raw()
    }
}

impl<T> sealed::Sealed for *const T {}

impl<T> AsRaw<T> for *const T {
    fn as_raw(&self) -> *mut T {
        self.cast_mut()
    }
}

impl<T> sealed::Sealed for *mut T {}

impl<T> AsRaw<T> for *mut T {
    fn as_raw(&self) -> *mut T {
        *self
    }
}

pub struct Swapped<P: Pointer> {
    storage: Storage<P::Base>,
    holds: PhantomData<P>,
}

pub type Swap<T> = Swapped<Arc<T>>;

pub type SwapOption<T> = Swapped<Option<Arc<T>>>;

impl<P: Pointer> Swapped<P> {
    pub fn new(value: P) -> Self {
        Self {
            storage: Storage::new(value.into_raw()),
            holds: PhantomData,
        }
    }

    pub fn into_inner(self) -> P {
        let mut this = ManuallyDrop::new(self);
        let raw = this.storage.settle_debts();
        unsafe { P::from_raw(raw) }
    }

    pub fn load(&self) -> Guard<P> {
        self.storage.load_guard()
    }

    pub fn load_full(&self) -> P {
        let raw = self.storage.load_owned();
        unsafe { P::from_raw(raw) }
    }

    pub fn store(&self, value: P) {
        drop(self.swap(value));
    }

    pub fn swap(&self, value: P) -> P {
        let raw = self.storage.swap(value.into_raw());
        unsafe { P::from_raw(raw) }
    }

    pub fn compare_and_swap<C: AsRaw<P::Base>, N: Into<P>>(&self, current: C, new: N) -> Guard<P> {
        let expected = current.as_raw();
        let new = new.into().into_raw();
        loop {
            let previous = self.load();
            if previous.as_raw() != expected {
                release(new);
                return previous;
            }
            if self.storage.compare_exchange(expected, new) {
                drop(previous);
                return Guard::owned(expected);
            }
        }
    }

    pub fn rcu<R, F>(&self, mut change: F) -> P
    where
        F: FnMut(&P) -> R,
        R: Into<P>,
    {
        let mut current = self.load();
        loop {
            let new = change(&current);
            let previous = self.compare_and_swap(&current, new);
            if previous.as_raw() == current.as_raw() {
                return Guard::into_inner(previous);
            }
            current = previous;
        }
    }
}

impl<P: Pointer> Drop for Swapped<P> {
    fn drop(&mut self) {
        release(self.storage.settle_debts());
    }
}

impl<T> Swap<T> {
    pub fn from_pointee(value: T) -> Self {
        Self::new(Arc::new(value))
    }
}

impl<T> SwapOption<T> {
    #[must_use]
    pub const fn const_empty() -> Self {
        Self {
            storage: Storage::empty(),
            holds: PhantomData,
        }
    }

    #[must_use]
    pub const fn empty() -> Self {
        Self::const_empty()
    }

    pub fn from_pointee(value: impl Into<Option<T>>) -> Self {
        Self::new(value.into().map(Arc::new))
    }
}

impl<P: Pointer + Default> Default for Swapped<P> {
    fn default() -> Self {
        Self::new(P::default())
    }
}

impl<P: Pointer> From<P> for Swapped<P> {
    fn from(value: P) -> Self {
        Self::new(value)
    }
}

impl<P: Pointer + fmt::Debug> fmt::Debug for Swapped<P> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("ArcSwapAny").field(&*self.load()).finish()
    }
}

impl<P: Pointer + fmt::Display> fmt::Display for Swapped<P> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        (*self.load()).fmt(formatter)
    }
}

pub struct Enclosed<T> {
    storage: Storage<T>,
    holds: PhantomData<Arc<T>>,
}

impl<T> Enclosed<T> {
    pub fn from_pointee(value: T) -> Self {
        Self {
            storage: Storage::new(Arc::into_raw(Arc::new(value)).cast_mut()),
            holds: PhantomData,
        }
    }

    pub fn load_full(&self) -> Arc<T> {
        let raw = self.storage.load_owned();
        unsafe { <Arc<T> as Raw>::from_raw(raw) }
    }

    pub fn with<R>(&self, read: impl FnOnce(&T) -> R) -> R {
        read(&self.storage.load_guard::<Arc<T>>())
    }

    pub fn swap(&self, value: Arc<T>) -> Arc<T> {
        let raw = self.storage.swap(Raw::into_raw(value));
        unsafe { <Arc<T> as Raw>::from_raw(raw) }
    }

    pub fn compare_and_swap(&self, current: &Arc<T>, new: Arc<T>) -> Arc<T> {
        let expected = Arc::as_ptr(current).cast_mut();
        let new = Raw::into_raw(new);
        loop {
            let previous = self.storage.load_guard::<Arc<T>>();
            if Raw::as_raw(&*previous) != expected {
                release(new);
                return Guard::into_inner(previous);
            }
            if self.storage.compare_exchange(expected, new) {
                drop(previous);
                return unsafe { <Arc<T> as Raw>::from_raw(expected) };
            }
        }
    }
}

impl<T> Drop for Enclosed<T> {
    fn drop(&mut self) {
        release(*self.storage.pointer.get_mut());
    }
}
