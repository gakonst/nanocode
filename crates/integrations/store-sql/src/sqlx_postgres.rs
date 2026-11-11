use std::ops::DerefMut;

use futures_util::TryStreamExt as _;
use nanocode_durability_core::{
    Collected, Sql, SqlAccess, SqlDialect, SqlRow, SqlStatement, SqlStore, StagedWrites,
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

async fn transact<C: PgConnections>(
    connections: &C,
    statements: &[SqlStatement<'_>],
) -> Result<Vec<Vec<SqlRow>>, StoreError> {
    let mut connection = connections.acquire().await?;
    if let Some(prelude) = C::SIMPLE {
        return simple(&mut connection, prelude, statements).await;
    }
    let mut transaction = sqlx::Connection::begin(&mut *connection)
        .await
        .map_err(backend)?;
    let results = run(&mut transaction, statements).await?;
    transaction.commit().await.map_err(backend)?;
    Ok(results)
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
        _operation: &'static str,
        statements: &'a [SqlStatement<'a>],
    ) -> StoreFuture<'a, Result<Vec<Vec<SqlRow>>, StoreError>> {
        Box::pin(transact(&self.writes, statements))
    }

    fn run<'a>(
        &'a self,
        access: SqlAccess,
        _operation: &'static str,
        statements: &'a [SqlStatement<'a>],
    ) -> StoreFuture<'a, Result<Vec<Vec<SqlRow>>, StoreError>> {
        match access {
            SqlAccess::Read => Box::pin(transact(&self.reads, statements)),
            _ => Box::pin(transact(&self.writes, statements)),
        }
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
