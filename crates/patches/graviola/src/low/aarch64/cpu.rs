// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

pub(crate) fn enter_cpu_state() -> u32 {
    dit::maybe_enable()
}

pub(crate) fn leave_cpu_state(old: u32) {
    dit::maybe_disable(old);

    // SAFETY: this crate requires the `neon` cpu feature
    unsafe { zero_neon_registers() }
}

#[target_feature(enable = "neon")]
unsafe fn zero_neon_registers() {
    // SAFETY: inline assembly. all written registers are listed as clobbers.
    unsafe {
        core::arch::asm!(
            "       eor v0.16b, v0.16b, v0.16b",
            "       eor v1.16b, v1.16b, v1.16b",
            "       eor v2.16b, v2.16b, v2.16b",
            "       eor v3.16b, v3.16b, v3.16b",
            "       eor v4.16b, v4.16b, v4.16b",
            "       eor v5.16b, v5.16b, v5.16b",
            "       eor v6.16b, v6.16b, v6.16b",
            "       eor v7.16b, v7.16b, v7.16b",
            "       eor v8.16b, v8.16b, v8.16b",
            "       eor v9.16b, v9.16b, v9.16b",
            "       eor v10.16b, v10.16b, v10.16b",
            "       eor v11.16b, v11.16b, v11.16b",
            "       eor v12.16b, v12.16b, v12.16b",
            "       eor v13.16b, v13.16b, v13.16b",
            "       eor v14.16b, v14.16b, v14.16b",
            "       eor v15.16b, v15.16b, v15.16b",
            "       eor v16.16b, v16.16b, v16.16b",
            "       eor v17.16b, v17.16b, v17.16b",
            "       eor v18.16b, v18.16b, v18.16b",
            "       eor v19.16b, v19.16b, v19.16b",
            "       eor v20.16b, v20.16b, v20.16b",
            "       eor v21.16b, v21.16b, v21.16b",
            "       eor v22.16b, v22.16b, v22.16b",
            "       eor v23.16b, v23.16b, v23.16b",
            "       eor v24.16b, v24.16b, v24.16b",
            "       eor v25.16b, v25.16b, v25.16b",
            "       eor v26.16b, v26.16b, v26.16b",
            "       eor v27.16b, v27.16b, v27.16b",
            "       eor v28.16b, v28.16b, v28.16b",
            "       eor v29.16b, v29.16b, v29.16b",
            "       eor v30.16b, v30.16b, v30.16b",
            "       eor v31.16b, v31.16b, v31.16b",

            // clobbers
            out("v0") _,
            out("v1") _,
            out("v2") _,
            out("v3") _,
            out("v4") _,
            out("v5") _,
            out("v6") _,
            out("v7") _,
            out("v8") _,
            out("v9") _,
            out("v10") _,
            out("v11") _,
            out("v12") _,
            out("v13") _,
            out("v14") _,
            out("v15") _,
            out("v16") _,
            out("v17") _,
            out("v18") _,
            out("v19") _,
            out("v20") _,
            out("v21") _,
            out("v22") _,
            out("v23") _,
            out("v24") _,
            out("v25") _,
            out("v26") _,
            out("v27") _,
            out("v28") _,
            out("v29") _,
            out("v30") _,
            out("v31") _,
        )
    }
}

/// Effectively memset(ptr, 0, len), but not visible to optimiser
///
/// # Safety:
/// There must be `len` writable bytes at `ptr`; with no alignment
/// requirement.
pub(in crate::low) fn zero_bytes(ptr: *mut u8, len: usize) {
    // SAFETY: inline assembly.
    unsafe {
        core::arch::asm!(
            "       eor {zero}.16b, {zero}.16b, {zero}.16b",
            // by-16 loop
            "   2:  cmp {len}, #16",
            "       blt 3f",
            "       st1 {{{zero}.16b}}, [{ptr}]",
            "       add {ptr}, {ptr}, #16",
            "       sub {len}, {len}, #16",
            "       b 2b",
            // by-1 loop
            "   3:  subs {len}, {len}, #1",
            "       blt 4f",
            "       strb wzr, [{ptr}], #1",
            "       b 3b",
            "   4:  ",

            ptr = inout(reg) ptr => _,
            len = inout(reg) len => _,

            // clobbers
            zero = out(vreg) _,
        )
    }
}

/// Effectively memcmp(a, b, len), but guaranteed to visit every element
/// of a and b.
///
/// The return value does not follow memcmp semantics: it is zero if
/// `a == b`, otherwise it is non-zero.
///
/// # Safety
/// The caller must ensure that there are `len` bytes readable at `a` and `b`,
pub(in crate::low) unsafe fn ct_compare_bytes(a: *const u8, b: *const u8, len: usize) -> u8 {
    // SAFETY: inline assembly.
    unsafe {
        let mut acc = 0u8;
        core::arch::asm!(
            "   2: cmp  {len}, 0",
            "      beq  3f",
            "      ldrb {tmpa:w}, [{a}]",
            "      ldrb {tmpb:w}, [{b}]",
            "      eor  {tmpa:w}, {tmpa:w}, {tmpb:w}",
            "      orr  {acc:w}, {acc:w}, {tmpa:w}",
            "      add  {a}, {a}, 1",
            "      add  {b}, {b}, 1",
            "      sub  {len}, {len}, 1",
            "      b    2b",
            "   3:  ",
            a = inout(reg) a => _,
            b = inout(reg) b => _,
            len = inout(reg) len => _,
            tmpa = inout(reg) 0u8 => _,
            tmpb = inout(reg) 0u8 => _,
            acc = inout(reg) acc,
        );
        acc
    }
}

/// This macro interdicts is_aarch64_feature_detected to allow testability.
macro_rules! have_cpu_feature {
    ("neon") => {
        crate::low::aarch64::cpu::test_toggle(
            "neon",
            std::arch::is_aarch64_feature_detected!("neon"),
        )
    };
    ("aes") => {
        crate::low::aarch64::cpu::test_toggle("aes", std::arch::is_aarch64_feature_detected!("aes"))
    };
    ("pmull") => {
        crate::low::aarch64::cpu::test_toggle(
            "pmull",
            std::arch::is_aarch64_feature_detected!("pmull"),
        )
    };
    ("sha2") => {
        crate::low::aarch64::cpu::test_toggle(
            "sha2",
            std::arch::is_aarch64_feature_detected!("sha2"),
        )
    };
    ("sha3") => {
        crate::low::aarch64::cpu::test_toggle(
            "sha3",
            std::arch::is_aarch64_feature_detected!("sha3"),
        )
    };
    ("dit") => {
        crate::low::aarch64::cpu::test_toggle("dit", std::arch::is_aarch64_feature_detected!("dit"))
    };
}

/// Token type reflecting the check for the SHA3 CPU feature
///
/// A value of this type is proof that the CPU dynamic feature check has happened.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HaveSha3(());

impl HaveSha3 {
    pub(crate) fn check() -> Option<Self> {
        match have_cpu_feature!("sha3") {
            true => Some(Self(())),
            false => None,
        }
    }
}

pub(crate) fn verify_cpu_features() {
    assert!(
        have_cpu_feature!("neon"),
        "graviola requires neon CPU support"
    );
    assert!(
        have_cpu_feature!("aes"),
        "graviola requires aes CPU support"
    );
    assert!(
        have_cpu_feature!("pmull"),
        "graviola requires pmull CPU support"
    );
    assert!(
        have_cpu_feature!("sha2"),
        "graviola requires sha2 CPU support"
    );
}

mod dit {
    pub(super) fn maybe_enable() -> u32 {
        if have_cpu_feature!("dit") {
            // SAFETY: in this branch, we verified `dit` cpu feature is supported
            match unsafe { read() } {
                0 => {
                    // SAFETY: in this branch, we verified `dit` cpu feature is supported
                    unsafe {
                        write(1);
                    };
                    1
                }
                _ => 0,
            }
        } else {
            0
        }
    }

    pub(super) fn maybe_disable(we_enabled: u32) {
        if we_enabled > 0 {
            // SAFETY: `we_enabled > 0` implies `dit` cpu feature was supported earlier
            unsafe { write(0) }
        }
    }

    #[target_feature(enable = "dit")]
    fn read() -> u32 {
        let mut out: u64;
        // SAFETY: `mrs _, DIT` is defined only if `dit` cpu feature is supported
        unsafe {
            core::arch::asm!(
                "mrs {r}, DIT",
                r = out(reg) out,
            );
        }

        const DIT: u64 = 0x01000000;
        (out & DIT == DIT) as u32
    }

    #[target_feature(enable = "dit")]
    fn write(on: u32) {
        if on > 0 {
            // SAFETY: `msr DIT, _` is defined only if `dit` cpu feature is supported
            unsafe { core::arch::asm!("msr DIT, #1") }
        } else {
            // SAFETY: `msr DIT, _` is defined only if `dit` cpu feature is supported
            unsafe { core::arch::asm!("msr DIT, #0") }
        }
    }
}

#[cfg(not(debug_assertions))]
pub(crate) fn test_toggle(_id: &str, detected: bool) -> bool {
    detected
}

#[cfg(debug_assertions)]
pub(crate) fn test_toggle(id: &str, detected: bool) -> bool {
    if std::env::var(format!("GRAVIOLA_CPU_DISABLE_{id}")).is_ok() {
        println!("DEBUG: denying cpuid {id:?}");
        false
    } else {
        detected
    }
}

/// Read-only prefetch hint.
pub(in crate::low) fn prefetch_ro<T>(ptr: *const T) {
    // SAFETY: inline assembly
    unsafe {
        core::arch::asm!(
            "prfm pldl1strm, [{ptr}]",
            ptr = in(reg) ptr,
            options(readonly, nostack)
        );
    }
}

/// Read-write prefetch hint.
pub(in crate::low) fn prefetch_rw<T>(ptr: *const T) {
    // SAFETY: inline assembly
    unsafe {
        core::arch::asm!(
            "prfm pstl1keep, [{ptr}]",
            ptr = in(reg) ptr,
            options(readonly, nostack)
        );
    }
}
