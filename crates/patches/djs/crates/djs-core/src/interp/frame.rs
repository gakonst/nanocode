//! Frame records on the value stack (docs/compiler.md 7.1, D1086, D1150).
//!
//! ```text
//! bp      callee            the function object
//! bp+1    this
//! bp+2    arg0 .. arg(n-1)  n = max(argc, PARAMS), missing args padded with undefined
//! hp      prev_bp           int32
//! hp+1    prev_hp           int32
//! hp+2    ret_pc            int32: the caller's pc to resume (after the call instruction)
//! hp+3    argc              int32: actual argument count
//! hp+4    flags             int32: continuation (low 4 bits) | construct bit
//! hp+5    new_target        Value
//! lp      locals            lp = hp + HEADER, filled with undefined
//! sb      operand stack     sb = lp + LOCALS
//! ```
//!
//! The header needs `prev_hp` next to `prev_bp`: a frame's header position depends on its
//! padded argument count, which is only recorded in that header, so the caller's `hp`
//! cannot be recomputed from its `bp`. All header fields are plain Values, so the GC
//! traces the stack without knowing about frames.

/// Header slots.
pub const HEADER: u32 = 6;
pub const PREV_BP: u32 = 0;
pub const PREV_HP: u32 = 1;
pub const RET_PC: u32 = 2;
pub const ARGC: u32 = 3;
pub const FLAGS: u32 = 4;
pub const NEW_TARGET: u32 = 5;

/// `flags` bit: the frame was entered by `construct`.
pub const FLAG_CONSTRUCT: i32 = 1 << 4;

/// `flags` bit: a native frame (a `@native` builtin's generated state machine, 0063):
/// `pc` is the state, the locals are the generated slots. Set when the frame is pushed,
/// so a frame stays native (or bytecode) for its whole activation whatever the options
/// say later.
pub const FLAG_NATIVE: i32 = 1 << 5;

/// What happens with a frame's return value (docs/compiler.md 7.3, D1085, D1280).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Continuation {
    /// Push it on the caller's operand stack (calls, getters, slow paths).
    PushResult,
    /// Drop it (setters).
    DiscardResult,
    /// If it is not an object, use the frame's `this` (constructors).
    ConstructResult,
    /// The bottom frame of a `Vm::call`: `run` returns `Done(value)`.
    Entry,
    /// The bottom frame of a job (D1370): drop the value and run the next job.
    Job,
    /// Store it in the caller's stack slot (an absolute index) and resume at `ret_pc`,
    /// after the calling instruction: fills a placeholder (`get_method` through a getter).
    Replace(u32),
    /// `Replace`, resuming at the calling instruction itself: slow paths replace an
    /// operand with its primitive value and re-execute the instruction. The caller's
    /// `try` regions and line are those of `ret_pc` itself, not of `ret_pc - 1` (0086:
    /// `inc_loc` can be the first instruction of a `try`).
    Retry(u32),
    /// `%EachDense`'s callback (0137): the result completes one step of the loop, whose
    /// state is in the intrinsic's arguments right below the frame, then the frame is
    /// entered again for the next element, or the loop's k goes to the caller.
    Each,
}

/// Continuation kind bits of `flags` (bits 0-3); the `Replace` slot is in bits 8-30.
const KIND_MASK: i32 = 0xF;
const SLOT_SHIFT: u32 = 8;

impl Continuation {
    pub const fn from_flags(flags: i32) -> Continuation {
        match flags & KIND_MASK {
            0 => Continuation::PushResult,
            1 => Continuation::DiscardResult,
            2 => Continuation::ConstructResult,
            4 => Continuation::Replace((flags as u32 & 0x7FFF_FFFF) >> SLOT_SHIFT),
            5 => Continuation::Job,
            6 => Continuation::Retry((flags as u32 & 0x7FFF_FFFF) >> SLOT_SHIFT),
            7 => Continuation::Each,
            _ => Continuation::Entry,
        }
    }

    /// The `flags` encoding (a non-negative int32). Inline: every frame push (D2255).
    #[inline]
    pub const fn to_flags(self) -> i32 {
        match self {
            Continuation::PushResult => 0,
            Continuation::DiscardResult => 1,
            Continuation::ConstructResult => 2,
            Continuation::Entry => 3,
            Continuation::Replace(slot) => 4 | ((slot << SLOT_SHIFT) & 0x7FFF_FFFF) as i32,
            Continuation::Job => 5,
            Continuation::Retry(slot) => 6 | ((slot << SLOT_SHIFT) & 0x7FFF_FFFF) as i32,
            Continuation::Each => 7,
        }
    }

    /// A frame with no caller: the entry call's or a job's.
    pub const fn is_bottom(self) -> bool {
        matches!(self, Continuation::Entry | Continuation::Job)
    }

    /// The caller re-executes the calling instruction (`Retry`).
    pub const fn is_retry(self) -> bool {
        matches!(self, Continuation::Retry(_))
    }
}
