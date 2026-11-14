use std::{ops::Range, sync::Arc};

use nanocode_durability_core::{Sql, SqlStatement, StoreError, StoreFuture};
use nanocode_sync::Atom;

pub struct Compaction<'a> {
    pub archived: &'a [(u64, String)],
    pub dropped: Range<u64>,
}

pub trait LogHost: Send + Sync {
    fn scan<'a>(
        &'a self,
        scope: &'a str,
        from: u64,
        to: u64,
    ) -> StoreFuture<'a, Result<Vec<(u64, String)>, StoreError>>;

    fn load<'a>(
        &'a self,
        scope: &'a str,
    ) -> StoreFuture<'a, Result<Vec<(u64, String)>, StoreError>>;

    fn append<'a>(
        &'a self,
        scope: &'a str,
        rows: &'a [(u64, String)],
    ) -> StoreFuture<'a, Result<(), StoreError>>;

    fn append_compacting<'a>(
        &'a self,
        scope: &'a str,
        rows: &'a [(u64, String)],
        _compaction: Compaction<'a>,
    ) -> StoreFuture<'a, Result<(), StoreError>> {
        self.append(scope, rows)
    }
}

type Dialects = [&'static str; 2];

const CREATE: Dialects = [
    "CREATE TABLE IF NOT EXISTS nanocode_log (scope TEXT NOT NULL, seq INTEGER NOT NULL, body TEXT NOT NULL, PRIMARY KEY (scope, seq)) WITHOUT ROWID",
    "CREATE TABLE IF NOT EXISTS nanocode_log (scope TEXT NOT NULL, seq BIGINT NOT NULL, body TEXT NOT NULL, PRIMARY KEY (scope, seq))",
];
const INDEX: Dialects = [
    "CREATE INDEX IF NOT EXISTS nanocode_log_snapshots ON nanocode_log (scope, seq) WHERE substr(body, 1, 1) = 's'",
    "CREATE INDEX IF NOT EXISTS nanocode_log_snapshots ON nanocode_log (scope, seq) WHERE substr(body, 1, 1) = 's'",
];
const LOAD: Dialects = [
    "SELECT CAST(seq AS TEXT), body FROM nanocode_log WHERE scope = ?1 AND seq >= COALESCE((SELECT seq FROM nanocode_log INDEXED BY nanocode_log_snapshots WHERE scope = ?1 AND substr(body, 1, 1) = 's' ORDER BY seq DESC LIMIT 1), 0) ORDER BY seq",
    "SELECT CAST(seq AS TEXT), body FROM nanocode_log WHERE scope = $1::text AND seq >= COALESCE((SELECT seq FROM nanocode_log WHERE scope = $1::text AND substr(body, 1, 1) = 's' ORDER BY seq DESC LIMIT 1), 0) ORDER BY seq",
];
const SCAN: Dialects = [
    "SELECT CAST(seq AS TEXT), body FROM nanocode_log WHERE scope = ?1 AND seq >= CAST(?2 AS INTEGER) AND seq < CAST(?3 AS INTEGER) ORDER BY seq",
    "SELECT CAST(seq AS TEXT), body FROM nanocode_log WHERE scope = $1::text AND seq >= CAST($2::text AS BIGINT) AND seq < CAST($3::text AS BIGINT) ORDER BY seq",
];
const APPEND: Dialects = [
    "INSERT INTO nanocode_log (scope, seq, body) VALUES (?1, CAST(?2 AS INTEGER), ?3)",
    "INSERT INTO nanocode_log (scope, seq, body) VALUES ($1::text, CAST($2::text AS BIGINT), $3::text)",
];
const ARCHIVE: Dialects = [
    "UPDATE nanocode_log SET body = ?3 WHERE scope = ?1 AND seq = CAST(?2 AS INTEGER)",
    "UPDATE nanocode_log SET body = $3::text WHERE scope = $1::text AND seq = CAST($2::text AS BIGINT)",
];
const DROP: Dialects = [
    "DELETE FROM nanocode_log WHERE scope = ?1 AND seq >= CAST(?2 AS INTEGER) AND seq < CAST(?3 AS INTEGER)",
    "DELETE FROM nanocode_log WHERE scope = $1::text AND seq >= CAST($2::text AS BIGINT) AND seq < CAST($3::text AS BIGINT)",
];
pub struct SqlLog<H: ?Sized> {
    host: Arc<H>,
}

impl<H: Sql + ?Sized> SqlLog<H> {
    pub const fn new(host: Arc<H>) -> Self {
        Self { host }
    }

    fn pick(&self, statements: &Dialects) -> &'static str {
        self.host.dialect().pick(statements)
    }
}

fn invalid(detail: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(format!(
        "SQL host answered an invalid turn log row: {detail}"
    ))
}

fn row(cells: &[Option<String>]) -> Result<(u64, String), StoreError> {
    match cells {
        [Some(seq), Some(body)] => Ok((
            seq.parse()
                .map_err(|error| invalid(format!("seq {seq}: {error}")))?,
            body.clone(),
        )),
        _ => Err(invalid(format!("{} cells", cells.len()))),
    }
}

impl<H: Sql + Send + Sync + ?Sized> LogHost for SqlLog<H> {
    fn scan<'a>(
        &'a self,
        scope: &'a str,
        from: u64,
        to: u64,
    ) -> StoreFuture<'a, Result<Vec<(u64, String)>, StoreError>> {
        Box::pin(async move {
            let (from, to) = (from.to_string(), to.to_string());
            let statements = [SqlStatement {
                sql: self.pick(&SCAN),
                params: vec![scope, &from, &to],
            }];
            let rows = self.host.transaction("turn_log_scan", &statements).await?;
            rows.first()
                .ok_or_else(|| invalid("scan returned no result"))?
                .iter()
                .map(|cells| row(cells))
                .collect()
        })
    }

    fn load<'a>(
        &'a self,
        scope: &'a str,
    ) -> StoreFuture<'a, Result<Vec<(u64, String)>, StoreError>> {
        Box::pin(async move {
            let statements = [
                SqlStatement {
                    sql: self.pick(&CREATE),
                    params: Vec::new(),
                },
                SqlStatement {
                    sql: self.pick(&INDEX),
                    params: Vec::new(),
                },
                SqlStatement {
                    sql: self.pick(&LOAD),
                    params: vec![scope],
                },
            ];
            let rows = self.host.transaction("turn_log_load", &statements).await?;
            rows.get(2)
                .ok_or_else(|| invalid("load returned no result"))?
                .iter()
                .map(|cells| row(cells))
                .collect()
        })
    }

    fn append<'a>(
        &'a self,
        scope: &'a str,
        rows: &'a [(u64, String)],
    ) -> StoreFuture<'a, Result<(), StoreError>> {
        self.append_compacting(
            scope,
            rows,
            Compaction {
                archived: &[],
                dropped: 0..0,
            },
        )
    }

    fn append_compacting<'a>(
        &'a self,
        scope: &'a str,
        rows: &'a [(u64, String)],
        compaction: Compaction<'a>,
    ) -> StoreFuture<'a, Result<(), StoreError>> {
        Box::pin(async move {
            let seqs = rows
                .iter()
                .chain(compaction.archived)
                .map(|(seq, _)| seq.to_string())
                .collect::<Vec<_>>();
            let (from, to) = (
                compaction.dropped.start.to_string(),
                compaction.dropped.end.to_string(),
            );
            let mut statements = rows
                .iter()
                .chain(compaction.archived)
                .zip(&seqs)
                .enumerate()
                .map(|(index, ((_, body), seq))| SqlStatement {
                    sql: self.pick(if index < rows.len() {
                        &APPEND
                    } else {
                        &ARCHIVE
                    }),
                    params: vec![scope, seq, body],
                })
                .collect::<Vec<_>>();
            if !compaction.dropped.is_empty() {
                statements.push(SqlStatement {
                    sql: self.pick(&DROP),
                    params: vec![scope, &from, &to],
                });
            }
            self.host
                .transaction("turn_log_append", &statements)
                .await
                .map(drop)
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MemoryWrites {
    pub bytes: u64,
    pub rows: u64,
}

#[derive(Clone, Default)]
struct Memory {
    rows: nanocode_sync::OrdMap<(Arc<str>, u64), Arc<str>>,
    writes: MemoryWrites,
}

#[derive(Default)]
pub struct MemoryLog {
    memory: Atom<Memory>,
}

impl MemoryLog {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn writes(&self) -> MemoryWrites {
        self.memory.read(|memory| memory.writes)
    }

    pub fn truncate(&self, seq: u64) {
        self.memory.update(|memory| {
            memory.rows = memory
                .rows
                .iter()
                .filter(|((_, row), _)| *row <= seq)
                .map(|(key, body)| (key.clone(), Arc::clone(body)))
                .collect();
        });
    }
}

impl LogHost for MemoryLog {
    fn scan<'a>(
        &'a self,
        scope: &'a str,
        from: u64,
        to: u64,
    ) -> StoreFuture<'a, Result<Vec<(u64, String)>, StoreError>> {
        let rows = self.memory.read(|memory| {
            memory
                .rows
                .iter()
                .filter(|((owner, seq), _)| **owner == *scope && (from..to).contains(seq))
                .map(|((_, seq), body)| (*seq, body.to_string()))
                .collect()
        });
        Box::pin(async move { Ok(rows) })
    }

    fn load<'a>(
        &'a self,
        scope: &'a str,
    ) -> StoreFuture<'a, Result<Vec<(u64, String)>, StoreError>> {
        let rows = self.memory.read(|memory| {
            let owned = memory
                .rows
                .iter()
                .filter(|((owner, _), _)| **owner == *scope)
                .collect::<Vec<_>>();
            let start = owned
                .iter()
                .rposition(|(_, body)| body.starts_with('s'))
                .unwrap_or(0);
            owned[start..]
                .iter()
                .map(|((_, seq), body)| (*seq, body.to_string()))
                .collect()
        });
        Box::pin(async move { Ok(rows) })
    }

    fn append<'a>(
        &'a self,
        scope: &'a str,
        rows: &'a [(u64, String)],
    ) -> StoreFuture<'a, Result<(), StoreError>> {
        self.append_compacting(
            scope,
            rows,
            Compaction {
                archived: &[],
                dropped: 0..0,
            },
        )
    }

    fn append_compacting<'a>(
        &'a self,
        scope: &'a str,
        rows: &'a [(u64, String)],
        compaction: Compaction<'a>,
    ) -> StoreFuture<'a, Result<(), StoreError>> {
        let scope = Arc::<str>::from(scope);
        let result = self.memory.update(|memory| {
            if rows
                .iter()
                .any(|(seq, _)| memory.rows.contains_key(&(Arc::clone(&scope), *seq)))
            {
                return Err(StoreError::Fenced);
            }
            for (seq, body) in rows.iter().chain(compaction.archived) {
                memory
                    .rows
                    .insert((Arc::clone(&scope), *seq), Arc::from(body.as_str()));
                memory.writes.rows += 1;
                memory.writes.bytes += body.len() as u64;
            }
            for seq in compaction.dropped.clone() {
                memory.rows.remove(&(Arc::clone(&scope), seq));
            }
            Ok(())
        });
        Box::pin(async move { result })
    }
}
