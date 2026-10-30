mod audio;
pub mod bytes;
mod cell;
mod journal;
#[cfg(not(target_family = "wasm"))]
mod native;
#[cfg(target_family = "wasm")]
mod wasm;

pub use cell::RuntimeEvent;
pub use journal::{CellJournaling, JOURNAL_LIMIT_BYTES, Journal, JournalEntry, SharedJournal};
#[cfg(not(target_family = "wasm"))]
pub use native::EmbeddedHost;
#[cfg(target_family = "wasm")]
pub use wasm::{DEFAULT_INTERRUPT_CHECKS, EmbeddedHost, set_interrupt_checks};
