#![cfg(not(target_family = "wasm"))]

mod genai;
mod held;
mod lines;

pub use genai::{Agent, Exported, GenAi};
pub use held::{Ended, Held, Holding};
pub use lines::{Counters, Exports, Lines, line};

pub const METRICS_TARGET: &str = "nc_metrics";
