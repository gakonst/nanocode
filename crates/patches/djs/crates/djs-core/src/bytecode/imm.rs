//! Immediate operand kinds (docs/compiler.md 4.1).
//!
//! Every immediate in the table has a kind. The kind fixes the width and encoding
//! (little-endian, unaligned), the Rust type of the emitter argument, and how the
//! disassembler prints it.

use super::code::Label;
use crate::heap::Atom as AtomId;

/// The kind of an immediate operand.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[repr(u8)]
pub enum ImmKind {
    /// A small enum or flag set (`special_object` kind, `copy_data_props` mode).
    U8,
    /// A small signed integer constant.
    I8,
    /// A signed integer constant.
    I32,
    /// An element count (`array_from`, `concat`): popped values.
    Count,
    /// A local slot index.
    Loc,
    /// An argument slot index.
    Arg,
    /// A capture index.
    Cap,
    /// A call argument count.
    Argc,
    /// A scope id (phase 1).
    Scope,
    /// An intrinsic id (privileged code).
    Intrinsic,
    /// A slot of the realm's prelude function table (privileged code, D2440).
    Builtin,
    /// An inline cache slot of the function (D1620): emitted as 0, numbered in code
    /// order by `alloc_template`; [`NO_CACHE`](super::template::NO_CACHE) when the
    /// function has too many.
    Cache,
    /// A constant pool index.
    Const,
    /// A child template index.
    Child,
    /// A name-ref side table index (phase 1).
    NameRef,
    /// The id of a `label` pseudo-op (phase 1).
    LabelId,
    /// An atom id.
    Atom,
    /// A jump target: a label id in phase-1 code, an `i32` offset relative to the end of
    /// the instruction in final code. Always the last immediate.
    Target,
    // One-byte forms of the kinds above, in the short ops (D2443). The emitter picks
    // a short op when its operand fits; `wide` names the kind it narrows.
    /// A local slot index below 256.
    Loc8,
    /// An argument slot index below 256.
    Arg8,
    /// A constant pool index below 256.
    Const8,
    /// An intrinsic id below 256.
    Intrinsic8,
    /// A prelude function table slot below 256.
    Builtin8,
    /// A jump offset in `-128..=127` (final code only: `CodeBuf::finish` shortens
    /// jumps).
    Target8,
}

impl ImmKind {
    /// Encoded width in bytes.
    pub const fn width(self) -> u8 {
        match self {
            ImmKind::U8
            | ImmKind::I8
            | ImmKind::Loc8
            | ImmKind::Arg8
            | ImmKind::Const8
            | ImmKind::Intrinsic8
            | ImmKind::Builtin8
            | ImmKind::Target8 => 1,
            ImmKind::Count
            | ImmKind::Loc
            | ImmKind::Arg
            | ImmKind::Cap
            | ImmKind::Argc
            | ImmKind::Scope
            | ImmKind::Intrinsic
            | ImmKind::Builtin
            | ImmKind::Cache => 2,
            ImmKind::I32
            | ImmKind::Const
            | ImmKind::Child
            | ImmKind::NameRef
            | ImmKind::LabelId
            | ImmKind::Atom
            | ImmKind::Target => 4,
        }
    }

    /// Whether the raw value is sign-extended when decoded.
    pub const fn is_signed(self) -> bool {
        matches!(
            self,
            ImmKind::I8 | ImmKind::I32 | ImmKind::Target | ImmKind::Target8
        )
    }

    /// The kind a one-byte kind narrows (itself for the others): what the operand
    /// means.
    pub const fn wide(self) -> ImmKind {
        match self {
            ImmKind::Loc8 => ImmKind::Loc,
            ImmKind::Arg8 => ImmKind::Arg,
            ImmKind::Const8 => ImmKind::Const,
            ImmKind::Intrinsic8 => ImmKind::Intrinsic,
            ImmKind::Builtin8 => ImmKind::Builtin,
            ImmKind::Target8 => ImmKind::Target,
            k => k,
        }
    }

    /// A jump target (either width).
    pub const fn is_target(self) -> bool {
        matches!(self, ImmKind::Target | ImmKind::Target8)
    }
}

/// Maps an immediate kind to its emitter argument type and its raw `u32` (the bytes
/// [`CodeBuf`] writes are the low `width` bytes, little-endian; a `Target` is a label
/// id). Implemented by the marker types below, which the `ops!` table names.
pub trait Imm {
    const KIND: ImmKind;
    type Ty: Copy;
    fn raw(v: Self::Ty) -> u32;
}

macro_rules! plain_imm {
    ($($name:ident: $ty:ty => |$v:ident| $raw:expr;)*) => {$(
        #[doc = concat!("Marker for [`ImmKind::", stringify!($name), "`].")]
        pub struct $name;
        impl Imm for $name {
            const KIND: ImmKind = ImmKind::$name;
            type Ty = $ty;
            #[inline(always)]
            fn raw($v: $ty) -> u32 {
                $raw
            }
        }
    )*};
}

plain_imm! {
    U8: u8 => |v| v as u32;
    I8: i8 => |v| v as u8 as u32;
    I32: i32 => |v| v as u32;
    Count: u16 => |v| v as u32;
    Loc: u16 => |v| v as u32;
    Arg: u16 => |v| v as u32;
    Cap: u16 => |v| v as u32;
    Argc: u16 => |v| v as u32;
    Scope: u16 => |v| v as u32;
    Intrinsic: u16 => |v| v as u32;
    Builtin: u16 => |v| v as u32;
    Cache: u16 => |v| v as u32;
    Const: u32 => |v| v;
    Child: u32 => |v| v;
    NameRef: u32 => |v| v;
    LabelId: Label => |v| v.0;
    Atom: AtomId => |v| v.id();
    Target: Label => |v| v.0;
    Loc8: u8 => |v| v as u32;
    Arg8: u8 => |v| v as u32;
    Const8: u8 => |v| v as u32;
    Intrinsic8: u8 => |v| v as u32;
    Builtin8: u8 => |v| v as u32;
    Target8: Label => |v| v.0;
}

// The table's `ImmKind`s and the marker types must agree on the width.
const _: () = {
    assert!(<U8 as Imm>::KIND.width() == 1 && <I8 as Imm>::KIND.width() == 1);
    assert!(<Loc as Imm>::KIND.width() == 2 && <Atom as Imm>::KIND.width() == 4);
    assert!(<Target as Imm>::KIND.width() == 4 && <LabelId as Imm>::KIND.width() == 4);
};
