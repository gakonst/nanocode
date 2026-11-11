#![cfg(not(target_family = "wasm"))]

mod inspect;
mod leadership;
mod lock;
mod notify;
mod pool;
mod postgres;
mod sqlx_postgres;
mod statements;
mod zstd;

pub use inspect::{Inspected, inspect};
pub use leadership::Leadership;
pub use lock::{begin, held};
pub use notify::{Forwarding, Trouble, forward, listen};
pub use pool::{Warmed, unavailable, warm};
pub use postgres::{Locked, PipelineError, Pipelined, PostgresSql, Unavailable, collected_wakes};
pub use sqlx_postgres::{
    PgConnections, SqlxSql, SqlxTransaction, collect, commit_staged, stage_records, unreachable,
};
pub use statements::{Ran, Slow, SlowPlans, generic_plan, timed};
pub use zstd::Zstd;
