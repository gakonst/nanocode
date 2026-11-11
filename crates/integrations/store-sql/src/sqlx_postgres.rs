use std::{
    ops::DerefMut,
    time::{Duration, Instant},
};

use futures_util::TryStreamExt as _;
use nanocode_durability_core::{
    Collected, Settle, Sql, SqlAccess, SqlDialect, SqlRow, SqlStatement, SqlStore, StagedWrites,
    StoreError, StoreFuture, Unreachable,
};
use nanocode_sync::Owned;
use sqlx::{AssertSqlSafe, Either, PgConnection, Row};

pub trait PgConnections: Send + Sync {
    type Connection: DerefMut<Target = PgConnection> + Send;
    const SIMPLE: Option<&'static str> = None;

    fn acquire(&self) -> StoreFuture<'_, Result<Self::Connection, StoreError>>;

    fn lanes(&self) -> usize {
        1
    }

    fn ran(&self, _operation: &'static str, _took: Duration, _committed: bool) {}
}

pub struct SqlxSql<C, R> {
    writes: C,
    reads: R,
}

impl<C: PgConnections, R: PgConnections> SqlxSql<C, R> {
    #[must_use]
    pub const fn reading(writes: C, reads: R) -> Self {
        Self { writes, reads }
    }
}

const UNSYNCED: &str = "SET LOCAL synchronous_commit = off";
const BEGIN_UNSYNCED: &str = "BEGIN; SET LOCAL synchronous_commit = off";

async fn transact<C: PgConnections>(
    connections: &C,
    settle: Settle,
    statements: &[SqlStatement<'_>],
) -> Result<Vec<Vec<SqlRow>>, StoreError> {
    let mut connection = connections.acquire().await?;
    if let Some(prelude) = C::SIMPLE {
        return match settle {
            Settle::Durable => simple(&mut connection, prelude, statements).await,
            Settle::Ordered => {
                simple(
                    &mut connection,
                    &format!("{prelude};{UNSYNCED}"),
                    statements,
                )
                .await
            }
        };
    }
    let mut transaction = match settle {
        Settle::Durable => sqlx::Connection::begin(&mut *connection).await,
        Settle::Ordered => sqlx::Connection::begin_with(&mut *connection, BEGIN_UNSYNCED).await,
    }
    .map_err(backend)?;
    let results = run(&mut transaction, statements).await?;
    transaction.commit().await.map_err(backend)?;
    Ok(results)
}

async fn observed<C: PgConnections>(
    connections: &C,
    settle: Settle,
    operation: &'static str,
    statements: &[SqlStatement<'_>],
) -> Result<Vec<Vec<SqlRow>>, StoreError> {
    let started = Instant::now();
    let results = transact(connections, settle, statements).await;
    connections.ran(operation, started.elapsed(), results.is_ok());
    results
}

impl<C: PgConnections + 'static, R: PgConnections + 'static> Sql for SqlxSql<C, R> {
    fn concurrent(self: std::sync::Arc<Self>) -> Option<std::sync::Arc<dyn Sql>> {
        (self.writers() > 1 || self.readers() > 1).then_some(self as std::sync::Arc<dyn Sql>)
    }

    fn writers(&self) -> usize {
        self.writes.lanes()
    }

    fn readers(&self) -> usize {
        self.reads.lanes()
    }

    fn transaction<'a>(
        &'a self,
        operation: &'static str,
        statements: &'a [SqlStatement<'a>],
    ) -> StoreFuture<'a, Result<Vec<Vec<SqlRow>>, StoreError>> {
        Box::pin(observed(
            &self.writes,
            Settle::Durable,
            operation,
            statements,
        ))
    }

    fn run<'a>(
        &'a self,
        access: SqlAccess,
        operation: &'static str,
        statements: &'a [SqlStatement<'a>],
    ) -> StoreFuture<'a, Result<Vec<Vec<SqlRow>>, StoreError>> {
        match access {
            SqlAccess::Read => Box::pin(observed(
                &self.reads,
                Settle::Durable,
                operation,
                statements,
            )),
            _ => Box::pin(observed(
                &self.writes,
                Settle::Durable,
                operation,
                statements,
            )),
        }
    }

    fn write<'a>(
        &'a self,
        settle: Settle,
        operation: &'static str,
        statements: &'a [SqlStatement<'a>],
    ) -> StoreFuture<'a, Result<Vec<Vec<SqlRow>>, StoreError>> {
        Box::pin(observed(&self.writes, settle, operation, statements))
    }

    fn dialect(&self) -> SqlDialect {
        SqlDialect::Postgres
    }
}

pub struct SqlxTransaction<'c>(Owned<&'c mut PgConnection>);

impl<'c> SqlxTransaction<'c> {
    pub fn new(connection: &'c mut PgConnection) -> Self {
        Self(Owned::new(connection))
    }
}

impl Sql for SqlxTransaction<'_> {
    fn transaction<'a>(
        &'a self,
        _operation: &'static str,
        statements: &'a [SqlStatement<'a>],
    ) -> StoreFuture<'a, Result<Vec<Vec<SqlRow>>, StoreError>> {
        Box::pin(async move { run(&mut **self.0.acquire().await, statements).await })
    }

    fn dialect(&self) -> SqlDialect {
        SqlDialect::Postgres
    }
}

pub async fn commit_staged(
    writes: &StagedWrites,
    connection: &mut PgConnection,
) -> Result<(), StoreError> {
    writes
        .commit_to(&mut SqlStore::new(SqlxTransaction::new(connection)).migrated())
        .await
}

pub async fn stage_records(
    writes: &StagedWrites,
    connection: &mut PgConnection,
) -> Result<(), StoreError> {
    writes
        .stage_records(&mut SqlStore::new(SqlxTransaction::new(connection)).migrated())
        .await
}

pub async fn unreachable(
    state_id: &str,
    connection: &mut PgConnection,
) -> Result<Unreachable, StoreError> {
    SqlStore::new(SqlxTransaction::new(connection))
        .migrated()
        .unreachable(state_id)
        .await
}

pub async fn collect(
    state_id: &str,
    revision: u64,
    keys: &[String],
    connection: &mut PgConnection,
) -> Result<Option<Collected>, StoreError> {
    SqlStore::new(SqlxTransaction::new(connection))
        .migrated()
        .collect(state_id, revision, keys)
        .await
}

async fn run(
    connection: &mut PgConnection,
    statements: &[SqlStatement<'_>],
) -> Result<Vec<Vec<SqlRow>>, StoreError> {
    let mut results = Vec::with_capacity(statements.len());
    for statement in statements {
        let query = statement
            .params
            .iter()
            .fold(sqlx::query(statement.sql), |query, param| {
                query.bind(*param)
            });
        let rows = query.fetch_all(&mut *connection).await.map_err(backend)?;
        let rows = rows
            .iter()
            .map(|row| (0..row.len()).map(|column| row.try_get(column)).collect())
            .collect::<Result<_, _>>()
            .map_err(backend)?;
        results.push(rows);
    }
    Ok(results)
}

async fn simple(
    connection: &mut PgConnection,
    prelude: &str,
    statements: &[SqlStatement<'_>],
) -> Result<Vec<Vec<SqlRow>>, StoreError> {
    let mut sql = prelude.to_owned();
    for statement in statements {
        sql.push(';');
        inline(&mut sql, statement)?;
    }
    let mut ended = Vec::with_capacity(statements.len() + 1);
    let mut rows = Vec::new();
    let mut results = sqlx::raw_sql(AssertSqlSafe(sql)).fetch_many(connection);
    while let Some(result) = results.try_next().await.map_err(backend)? {
        match result {
            Either::Left(_) => ended.push(std::mem::take(&mut rows)),
            Either::Right(row) => rows.push(
                (0..row.len())
                    .map(|column| row.try_get(column))
                    .collect::<Result<_, _>>()
                    .map_err(backend)?,
            ),
        }
    }
    let Some(first) = ended.len().checked_sub(statements.len()) else {
        return Err(StoreError::Backend(format!(
            "Postgres ended {} of {} statements",
            ended.len(),
            statements.len()
        )));
    };
    Ok(ended.split_off(first))
}

fn inline(sql: &mut String, statement: &SqlStatement<'_>) -> Result<(), StoreError> {
    let refused = || {
        StoreError::Backend(format!(
            "a statement sent with its parameters inlined holds only $n placeholders outside quotes and comments: {}",
            statement.sql
        ))
    };
    let mut quote = None;
    let mut chars = statement.sql.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(_), '\\') => return Err(refused()),
            (Some(open), c) => {
                quote = (c != open).then_some(open);
                sql.push(c);
            }
            (None, '\'' | '"') => {
                quote = Some(c);
                sql.push(c);
            }
            (None, '-') if chars.peek() == Some(&'-') => return Err(refused()),
            (None, '/') if chars.peek() == Some(&'*') => return Err(refused()),
            (None, '$') => {
                let mut index = 0_usize;
                while let Some(digit) = chars.peek().and_then(|c| c.to_digit(10)) {
                    chars.next();
                    index = index
                        .checked_mul(10)
                        .and_then(|index| index.checked_add(digit as usize))
                        .ok_or_else(refused)?;
                }
                let value = index
                    .checked_sub(1)
                    .and_then(|at| statement.params.get(at))
                    .filter(|value| !value.contains('\0'))
                    .ok_or_else(refused)?;
                sql.push_str("E'");
                for c in value.chars() {
                    match c {
                        '\\' => sql.push_str("\\\\"),
                        '\'' => sql.push_str("''"),
                        c => sql.push(c),
                    }
                }
                sql.push('\'');
            }
            (None, c) => sql.push(c),
        }
    }
    match quote {
        Some(_) => Err(refused()),
        None => Ok(()),
    }
}

fn backend(error: sqlx::Error) -> StoreError {
    StoreError::Backend(error.to_string())
}
