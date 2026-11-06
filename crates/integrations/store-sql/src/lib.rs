#![cfg(not(target_family = "wasm"))]

mod lock;
mod notify;
mod pipeline;
mod postgres;
mod sqlite;
mod sqlx_postgres;

pub use lock::{begin, begin_exclusive, held};
pub use nanocode_durability_core::{Collected, Unreachable};
pub use notify::{Forwarding, Trouble, forward, listen};
pub use pipeline::{
    Alive, Locked, PipelineError, Pipelined, Pipelines, Unavailable, collected_wakes,
};
pub use postgres::PostgresSql;
pub use sqlite::{SqliteOptions, SqliteSql};
pub use sqlx_postgres::{
    PgConnections, SqlxSql, SqlxTransaction, collect, commit_staged, stage_records, unreachable,
};
