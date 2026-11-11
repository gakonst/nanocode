// psh patch (see PATCHES.md): `std::time::Instant::now()` panics on
// wasm32-unknown-unknown, which has no clock. The printers only use the clock for the
// elapsed time in their statistics, so there it reads zero.

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub(crate) use std::time::Instant;

/// A clock that never advances.
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
#[derive(Clone, Copy, Debug)]
pub(crate) struct Instant;

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
impl Instant {
    pub(crate) fn now() -> Instant {
        Instant
    }

    pub(crate) fn elapsed(&self) -> std::time::Duration {
        std::time::Duration::ZERO
    }
}
