#![cfg(not(target_family = "wasm"))]

use std::ops::DerefMut;

use nanocode_durability_core::{
    Sql, SqlDialect, SqlRow, SqlStatement, SqlStore, StagedWrites, StoreError, StoreFuture,
};
use nanocode_sync::Owned;
use sqlx::{PgConnection, PgPool, Postgres, Row, pool::PoolConnection};

pub trait PgConnections: Send + Sync {
    type Connection: DerefMut<Target = PgConnection> + Send;

    fn acquire(&self) -> StoreFuture<'_, Result<Self::Connection, StoreError>>;
}

impl PgConnections for PgPool {
    type Connection = PoolConnection<Postgres>;

    fn acquire(&self) -> StoreFuture<'_, Result<Self::Connection, StoreError>> {
        Box::pin(async move { Self::acquire(self).await.map_err(backend) })
    }
}

pub struct SqlxSql<C>(C);

impl<C: PgConnections> SqlxSql<C> {
    #[must_use]
    pub const fn new(connections: C) -> Self {
        Self(connections)
    }
}

impl<C: PgConnections> Sql for SqlxSql<C> {
    fn transaction<'a>(
        &'a self,
        _operation: &'static str,
        statements: &'a [SqlStatement<'a>],
    ) -> StoreFuture<'a, Result<Vec<Vec<SqlRow>>, StoreError>> {
        Box::pin(async move {
            let mut connection = self.0.acquire().await?;
            let mut transaction = sqlx::Connection::begin(&mut *connection)
                .await
                .map_err(backend)?;
            let results = run(&mut transaction, statements).await?;
            transaction.commit().await.map_err(backend)?;
            Ok(results)
        })
    }

    fn dialect(&self) -> SqlDialect {
        SqlDialect::Postgres
    }
}

pub struct SqlxTransaction<'c>(Owned<&'c mut PgConnection>);

impl<'c> SqlxTransaction<'c> {
    #[must_use]
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

pub async fn run(
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

fn backend(error: sqlx::Error) -> StoreError {
    StoreError::Backend(error.to_string())
}
