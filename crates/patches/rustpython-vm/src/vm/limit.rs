//! nanocode's addition: a budget the embedder puts on the interpreter.
//!
//! RustPython 0.6.0 gives an embedder no way to stop a script: its signal check is
//! compiled out without `host_env` and never runs on wasm. A [`Limiter`] is asked
//! whenever [`VirtualMachine::fuel`] runs out, and one unit of fuel is spent by
//!
//! - every bytecode instruction (the dispatch loop in `frame.rs`),
//! - every step of an iterator driven from Rust ([`crate::protocol::PyIter::next`]:
//!   `list(it)`, `sum(it)`, `"".join(it)` and the like never reach the dispatch loop),
//! - every move of the regex matcher, charged [`MOVES_PER_TICK`] at a time.
//!
//! The limiter answers the next slice of fuel, or the exception that ends the run. Once
//! it has refused, [`VirtualMachine::stopped`] holds that exception: every later step
//! raises it again and no `except`, `finally` or `with` catches it, so the frames unwind
//! to the embedder, which calls [`VirtualMachine::set_limiter`] before the next run.
//!
//! [`VirtualMachine::reserve`] is asked before the allocations Python can size with a
//! number or a product in one instruction (sequence repetition, `bytes(n)`, padding,
//! integer powers and shifts, format widths and precisions, `expandtabs`, `replace`,
//! `join`, `translate`) and before a concatenation, so that `[0] * 10**9` is refused
//! before the memory is taken rather than after. Everything else is counted by
//! [`Metered`] as it is allocated, and crossing [`VirtualMachine::with_memory_ceiling`]
//! spends the fuel, so the limiter is asked at the next step.
//!
//! What stays unbounded: one native operation that takes no step (a sort, a big-integer
//! multiplication or factorial, a codec over a large input) runs to its end, and one
//! that makes many objects out of one (`split`, `list(text)`, the JSON scanner) or
//! lives in `rustpython-stdlib` (`array` repetition) has its memory before the ceiling
//! is looked at: a constant factor of what the script already holds.

use core::{cell::Cell, ptr::NonNull};

use rustpython_sre_engine::engine::MOVES_PER_TICK;

use crate::{PyResult, VirtualMachine, common::rc::PyRc};

/// The embedder's budget.
pub trait Limiter {
    /// The fuel is spent: the next slice, or the exception that ends the run.
    fn refuel(&self, vm: &VirtualMachine) -> PyResult<u64>;

    /// An instruction is about to allocate `bytes` at once.
    fn reserve(&self, vm: &VirtualMachine, bytes: usize) -> PyResult<()>;
}

impl VirtualMachine {
    /// Spends one unit of fuel; `true` when none is left and [`Self::refuel`] must run.
    #[inline(always)]
    pub fn spend(&self) -> bool {
        let left = self.fuel.get();
        if left == 0 {
            return true;
        }
        self.fuel.set(left - 1);
        false
    }

    /// Asks the limiter for more fuel. Fails with what it refused with, now and on
    /// every later call until [`Self::set_limiter`].
    #[cold]
    #[inline(never)]
    pub fn refuel(&self) -> PyResult<()> {
        if let Some(stop) = self.stopped.borrow().as_ref() {
            return Err(stop.clone());
        }
        let limiter = self.limiter.borrow().clone();
        let Some(limiter) = limiter else {
            self.fuel.set(u64::MAX);
            return Ok(());
        };
        match limiter.refuel(self) {
            Ok(slice) => {
                self.fuel.set(slice);
                Ok(())
            }
            Err(stop) => {
                *self.stopped.borrow_mut() = Some(stop.clone());
                Err(stop)
            }
        }
    }

    /// Asks the limiter before `bytes` are allocated at once. Nothing is asked while
    /// the bytes fit under [`Self::with_memory_ceiling`], so a call costs two loads
    /// and can stand in front of every concatenation.
    #[inline]
    pub fn reserve(&self, bytes: usize) -> PyResult<()> {
        if bytes <= room() {
            return Ok(());
        }
        self.refuse_or_reserve(bytes)
    }

    #[cold]
    #[inline(never)]
    fn refuse_or_reserve(&self, bytes: usize) -> PyResult<()> {
        if let Some(stop) = self.stopped.borrow().as_ref() {
            return Err(stop.clone());
        }
        let limiter = self.limiter.borrow().clone();
        let Some(limiter) = limiter else {
            return Ok(());
        };
        limiter.reserve(self, bytes).inspect_err(|stop| {
            *self.stopped.borrow_mut() = Some(stop.clone());
            self.fuel.set(0);
        })
    }

    /// Sets or removes the budget and forgets an earlier refusal. With a limiter the
    /// first step asks it for fuel.
    pub fn set_limiter(&self, limiter: Option<PyRc<dyn Limiter>>) {
        self.stopped.borrow_mut().take();
        self.fuel
            .set(if limiter.is_some() { 0 } else { u64::MAX });
        *self.limiter.borrow_mut() = limiter;
    }
}

/// The regex matcher's tick ([`rustpython_sre_engine::Request::tick`]), one for every
/// [`MOVES_PER_TICK`] moves: `false` ends the match.
pub(crate) fn tick() -> bool {
    super::thread::with_current_vm(|vm| {
        let left = vm.fuel.get();
        let moves = u64::from(MOVES_PER_TICK);
        if left > moves {
            vm.fuel.set(left - moves);
            return true;
        }
        vm.fuel.set(0);
        vm.refuel().is_ok()
    })
}

/// A global allocator that counts the bytes each thread holds, for a [`Limiter`] that
/// bounds a script's memory on a native host: the interpreter allocates through the
/// program's allocator like everything else, so only the program can count. A host
/// binary installs it around its own allocator,
/// `#[global_allocator] static A: Metered<System> = Metered(System);`, and
/// [`held`] then answers on every thread. (On wasm the module's linear memory is the
/// count and nothing needs installing.)
///
/// The count is per thread and signed: memory freed on another thread than the one
/// that allocated it moves from one thread's count to the other's.
pub struct Metered<A>(pub A);

struct Meter {
    held: Cell<isize>,
    ceiling: Cell<isize>,
    /// The fuel of the machine that set the ceiling, spent when the count crosses it.
    watch: Cell<Option<NonNull<Cell<u64>>>>,
}

thread_local! {
    static METER: Meter = const {
        Meter {
            held: Cell::new(0),
            ceiling: Cell::new(isize::MAX),
            watch: Cell::new(None),
        }
    };
}

static INSTALLED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

fn count(delta: isize) {
    if !INSTALLED.load(core::sync::atomic::Ordering::Relaxed) {
        INSTALLED.store(true, core::sync::atomic::Ordering::Relaxed);
    }
    // A thread that is ending has no count left to keep.
    let _ = METER.try_with(|meter| {
        let held = meter.held.get().wrapping_add(delta);
        meter.held.set(held);
        if held > meter.ceiling.get()
            && let Some(fuel) = meter.watch.get()
        {
            // SAFETY: `watch` is set only inside `with_memory_ceiling`, which borrows
            // the machine whose `fuel` this is for as long as it is set, on this thread.
            unsafe { fuel.as_ref() }.set(0);
        }
    });
}

/// The bytes this thread may still take before it crosses the ceiling of
/// [`VirtualMachine::with_memory_ceiling`]: `usize::MAX` without one.
#[inline]
fn room() -> usize {
    METER
        .try_with(|meter| {
            let ceiling = meter.ceiling.get();
            if ceiling == isize::MAX {
                usize::MAX
            } else {
                ceiling.saturating_sub(meter.held.get()).max(0).cast_unsigned()
            }
        })
        .unwrap_or(usize::MAX)
}

/// The bytes this thread holds, when the program's allocator is [`Metered`].
pub fn held() -> Option<isize> {
    INSTALLED
        .load(core::sync::atomic::Ordering::Relaxed)
        .then(|| METER.with(|meter| meter.held.get()))
}

impl VirtualMachine {
    /// Runs `run` with this thread allowed to hold `more` bytes beyond what it holds
    /// now ([`held`]): an allocation that crosses the ceiling spends the fuel, so the
    /// limiter is asked at the next step and can refuse. `None` sets no ceiling.
    pub fn with_memory_ceiling<R>(&self, more: Option<usize>, run: impl FnOnce() -> R) -> R {
        struct Lift;
        impl Drop for Lift {
            fn drop(&mut self) {
                lift_memory_ceiling();
            }
        }
        let _lift = Lift;
        if let Some(more) = more {
            METER.with(|meter| {
                let ceiling = meter.held.get().saturating_add_unsigned(more);
                meter.ceiling.set(ceiling);
                meter.watch.set(Some(NonNull::from(&self.fuel)));
            });
        }
        run()
    }
}

/// Ends [`VirtualMachine::with_memory_ceiling`] early, for what the embedder runs
/// after the script inside the same call.
pub fn lift_memory_ceiling() {
    let _ = METER.try_with(|meter| {
        meter.ceiling.set(isize::MAX);
        meter.watch.set(None);
    });
}

// SAFETY: every call is forwarded unchanged to the wrapped allocator, whose contract
// this one therefore keeps; the count touches only a thread-local cell.
unsafe impl<A: core::alloc::GlobalAlloc> core::alloc::GlobalAlloc for Metered<A> {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        // SAFETY: the caller's contract, forwarded.
        let ptr = unsafe { self.0.alloc(layout) };
        if !ptr.is_null() {
            count(layout.size().cast_signed());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: core::alloc::Layout) {
        // SAFETY: the caller's contract, forwarded.
        unsafe { self.0.dealloc(ptr, layout) };
        count(-layout.size().cast_signed());
    }

    unsafe fn alloc_zeroed(&self, layout: core::alloc::Layout) -> *mut u8 {
        // SAFETY: the caller's contract, forwarded.
        let ptr = unsafe { self.0.alloc_zeroed(layout) };
        if !ptr.is_null() {
            count(layout.size().cast_signed());
        }
        ptr
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: core::alloc::Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller's contract, forwarded.
        let new = unsafe { self.0.realloc(ptr, layout, new_size) };
        if !new.is_null() {
            count(new_size.cast_signed() - layout.size().cast_signed());
        }
        new
    }
}
