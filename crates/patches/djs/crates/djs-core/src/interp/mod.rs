//! The interpreter: value stack, frames, run states, host calls (docs/compiler.md 7).
//!
//! All interpreter state is in the arena:
//!
//! - the value stack, a `ValueArray` at `Roots::STACK`, addressed by index;
//! - the `Interp` cell at `Roots::INTERP`: registers (`BP`, `HP`, `SP`, `PC`) and the run
//!   state, flushed whenever `run` returns;
//! - the realm (`Roots::REALM`, an internal reference to a `Realm` cell) with the global
//!   object and the intrinsics;
//! - pending host calls at `Roots::HOST_CALLS`.
//!
//! So a suspended VM can be copied byte for byte into a fresh heap and resumed there.

pub(crate) mod conv;
pub mod frame;
pub(crate) mod module;
pub mod realm;
pub(crate) mod run;
pub mod state;

pub use frame::Continuation;
pub use realm::{ErrorKind, Intrinsic, error_stack_slot, error_summary, intrinsic, new_error};
pub use run::{ExecOptions, STACK_OVERFLOW, Status};
pub use state::RunState;
