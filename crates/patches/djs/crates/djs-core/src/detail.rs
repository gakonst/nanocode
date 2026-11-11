//! Debug details of engine-internal failures (D2336).
//!
//! An internal error (a compiler bug, a corrupt image) or an invariant panic shows its
//! cause's `Debug` form natively and in debug builds. The shipped module (wasm32 without
//! `debug_assertions`) records no panic messages anyway (D2332), and formatting the
//! details would keep every `Debug` impl and core::fmt's builders in it, so there the
//! text is only the description.

use alloc::string::String;

/// `"{what}: {detail:?}"`, or only `what` in the shipped module.
#[inline(always)]
pub(crate) fn detail(what: &str, detail: &dyn core::fmt::Debug) -> String {
    #[cfg(all(target_arch = "wasm32", not(debug_assertions)))]
    {
        let _ = detail;
        String::from(what)
    }
    #[cfg(not(all(target_arch = "wasm32", not(debug_assertions))))]
    {
        alloc::format!("{what}: {detail:?}")
    }
}

/// An opcode's name for an internal error message, or its byte in the shipped module,
/// which then needs no opcode names (D3092).
#[inline(always)]
pub(crate) fn op_name(op: crate::bytecode::Op) -> String {
    #[cfg(all(target_arch = "wasm32", not(debug_assertions)))]
    {
        crate::text::cat!("op ", op as u8)
    }
    #[cfg(not(all(target_arch = "wasm32", not(debug_assertions))))]
    {
        String::from(op.name())
    }
}
