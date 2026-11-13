use crate::postgres::{PgConnection, PgError, PgPool};
use nanocode_durability_core::{
    Settle, Sql, SqlAccess, SqlDialect, SqlRow, SqlStatement, StoreError, StoreFuture,
};
use nanocode_sync::Owned;
use std::sync::Arc;

pub struct PgSql {
    writes: PgPool,
    reads: PgPool,
}

impl PgSql {
    #[must_use]
    pub const fn reading(writes: PgPool, reads: PgPool) -> Self {
        Self { writes, reads }
    }
}

async fn transact(
    pool: &PgPool,
    settle: Settle,
    statements: &[SqlStatement<'_>],
) -> Result<Vec<Vec<SqlRow>>, StoreError> {
    let mut connection = pool.acquire().await.map_err(backend)?;
    connection
        .transaction(settle, statements)
        .await
        .map_err(backend)
}

impl Sql for PgSql {
    fn transaction<'a>(
        &'a self,
        _operation: &'static str,
        statements: &'a [SqlStatement<'a>],
    ) -> StoreFuture<'a, Result<Vec<Vec<SqlRow>>, StoreError>> {
        Box::pin(transact(&self.writes, Settle::Durable, statements))
    }

    fn run<'a>(
        &'a self,
        access: SqlAccess,
        _operation: &'static str,
        statements: &'a [SqlStatement<'a>],
    ) -> StoreFuture<'a, Result<Vec<Vec<SqlRow>>, StoreError>> {
        let pool = match access {
            SqlAccess::Read => &self.reads,
            SqlAccess::Write | SqlAccess::Schema => &self.writes,
        };
        Box::pin(transact(pool, Settle::Durable, statements))
    }

    fn write<'a>(
        &'a self,
        settle: Settle,
        _operation: &'static str,
        statements: &'a [SqlStatement<'a>],
    ) -> StoreFuture<'a, Result<Vec<Vec<SqlRow>>, StoreError>> {
        Box::pin(transact(&self.writes, settle, statements))
    }

    fn dialect(&self) -> SqlDialect {
        SqlDialect::Postgres
    }

    fn concurrent(self: Arc<Self>) -> Option<Arc<dyn Sql>> {
        (self.writers() > 1 || self.readers() > 1).then_some(self as Arc<dyn Sql>)
    }

    fn writers(&self) -> usize {
        self.writes.connections()
    }

    fn readers(&self) -> usize {
        self.reads.connections()
    }
}

pub struct PgTransaction<'c>(Owned<&'c mut PgConnection>);

impl<'c> PgTransaction<'c> {
    pub fn new(connection: &'c mut PgConnection) -> Self {
        Self(Owned::new(connection))
    }
}

impl Sql for PgTransaction<'_> {
    fn transaction<'a>(
        &'a self,
        _operation: &'static str,
        statements: &'a [SqlStatement<'a>],
    ) -> StoreFuture<'a, Result<Vec<Vec<SqlRow>>, StoreError>> {
        Box::pin(async move {
            self.0
                .acquire()
                .await
                .run(statements)
                .await
                .map_err(backend)
        })
    }

    fn dialect(&self) -> SqlDialect {
        SqlDialect::Postgres
    }
}

fn backend(error: PgError) -> StoreError {
    StoreError::Backend(error.to_string())
}
