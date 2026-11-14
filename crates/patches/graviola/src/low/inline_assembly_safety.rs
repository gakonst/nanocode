// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0
//! # Safety of inline assembly
//!
//! There are a number of ways inline assembly can be unsound.  This
//! documentation enumerates and addresses those.
//!
//! ## General assembly bugs
//!
//! `s2n-bignum` formally verifies that the assembly correctly computes
//! the desired mathematical operation.  Theoretically this could be
//! achieved even while being memory-unsafe (for example, reading past
//! the end of an input buffer, but not using that data in a further
//! calculation).
//!
//! Proving memory safety of arbitrary assembly is an active research
//! topic.
//!
//! ## Invariants required by the assembly
//!
//! Some assembly places requirements and limitations on its inputs.
//! We transcribe input length limitations to these places (in order of
//! preference):
//!
//! - the Rust type of the function.  For example, `bignum_inv_p256`
//!   takes a pointer to four 64-bit quantities for its input: therefore
//!   the input argument type of this function is `&[u64; 4]`.
//! - a `debug_assert`, which checks the invariant (these are typically
//!   the length of a slice, or a length relation between different
//!   slices.  See `bignum_emontredc_8n` for a more complex example.)
//!
//! Rust itself ensures the correct alignment of slices
//! (eg. `&[u64; 4].as_ptr()` is always correctly aligned according
//! to the required alignment of `u64`.)  Note that `s2n-bignum`'s
//! verification does not require alignment, but whether unaligned
//! accesses are unsound is architecturally defined.
//!
//! ## Incorrect clobbered registers
//!
//! "Clobbered registers" are registers that the inline assembly uses
//! other than for inputs or outputs.  These need to be communicated
//! to rustc so it can perform accurate register allocation.  If the
//! set of actually clobbered registers is larger than the those
//! communicated to the compiler, unsoundness results because the
//! register value of unrelated code will be trashed by the assembly.
//!
//! (In contrast, if the set of actually clobbered registers is smaller
//! than those communicated to the compiler, this is only an efficiency
//! concern due to extra register spills or non-optimal register allocation.
//! Currently this is the case: clobbered registers are
//! automatically computed by _any_ mention of a register name in the
//! assembly.  This notably includes cases where the assembly itself
//! manually spills registers that it clobbers.)
//!
//! ## Using unsupported instructions
//!
//! Using an unsupported instruction is generically unsound, though
//! many CPUs have defined exception behaviour (that kills the
//! process with the `SIGILL` signal, on some OSes).
//!
//! `crate::low::Entry` is responsible for panicking if run on an
//! unsuitable processor.  This is run before any inline assembly
//! code, because: `mod low` is non-public, and `mod low` is the only
//! venue where `unsafe` code may exist in this crate.
//!
//! The determination of which CPU features are requirements for this
//! crate is currently done manually, based on `s2n-bignum`'s documentation.
//!
//! ## `ret`
//! We always leave the function entry and exit to rustc, so it can
//! take care of ABI portability for us, and to enable inlining.
//! For this reason, it is unsound if `ret` is called in the outer
//! frame.  However, our inline assembly can contain leaf internal
//! functions: these may `ret` back to the outer frame.
//!
//! # Safety of intrinsics
//!
//! The above sections "Using unsupported instructions" also apply
//! to intrinsics, and the same arrangements exist to avoid ever
//! issuing an unsupported instruction.
//!
//! In general, intrinsics are less hazardous to use than inline
//! assembly.  However, since they are intended to be drop-in
//! replacements for their counterparts in C/C++, they are less
//! Rust-friendly than they could otherwise be.  For example,
//! an analog of `_mm_loadu_si128` could take `&[u8; 16]` as its
//! argument, rather than a pointer.  That would externalise the
//! requirements on that function, and allow it to be safe
//! (though only if `target_feature` `sse2` was statically
//! guaranteed at compile-time, and would require safe-transmute
//! to be available for non-byte types).
