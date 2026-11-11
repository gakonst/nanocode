//! Architecture-specific support for aarch64 with neon.

use core::arch::asm;

pub fn fma(mut x: f64, y: f64, z: f64) -> f64 {
    // SAFETY: `fmadd` is available with neon and has no side effects.
    unsafe {
        asm!(
            "fmadd {x:d}, {x:d}, {y:d}, {z:d}",
            x = inout(vreg) x,
            y = in(vreg) y,
            z = in(vreg) z,
            options(nomem, nostack, pure)
        );
    }
    x
}

pub fn sqrt(mut x: f64) -> f64 {
    // SAFETY: `fsqrt` is available with neon and has no side effects.
    unsafe {
        asm!(
            "fsqrt {x:d}, {x:d}",
            x = inout(vreg) x,
            options(nomem, nostack, pure)
        );
    }
    x
}
