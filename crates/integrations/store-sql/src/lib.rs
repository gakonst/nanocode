#![cfg(not(target_family = "wasm"))]

mod postgres;
mod sqlite;
mod sqlx_postgres;

pub use nanocode_durability_core::{Collected, Unreachable};
pub use postgres::PostgresSql;
pub use sqlite::{SqliteOptions, SqliteSql};
pub use sqlx_postgres::{
    PgConnections, SqlxSql, SqlxTransaction, collect, commit_staged, stage_records, unreachable,
};
