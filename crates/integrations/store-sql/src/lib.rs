#![cfg(not(target_family = "wasm"))]

mod inspect;
mod lock;
mod migrate;
mod notify;
mod pool;
mod postgres;
mod sqlite;
mod sqlx_postgres;
mod statements;

pub use inspect::{Inspected, inspect};
pub use lock::{begin, begin_exclusive, held};
pub use migrate::{
    Leadership, after, allowed, applied, drop_replaced, ensure_index, index, locked, try_lock,
};
pub use nanocode_durability_core::{Collected, Unreachable};
pub use notify::{Forwarding, Trouble, forward, listen};
pub use pool::{
    Counts, Lending, Lent, LongHold, Owing, PoolSlot, Reach, Reaching, Stranded, Warmed, contended,
    unavailable, warm,
};
pub use postgres::{
    Alive, Locked, PipelineError, Pipelined, PostgresSql, Unavailable, collected_wakes,
};
pub use sqlite::{SqliteOptions, SqliteSql};
pub use sqlx_postgres::{
    PgConnections, SqlxSql, SqlxTransaction, collect, commit_staged, stage_records, unreachable,
};
pub use statements::{Ran, Slow, SlowPlans, generic_plan, timed};
