use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering},
};

use nanocode_sync::{Atom, Owned};

use crate::{Settle, Sql, SqlAccess, SqlDialect, SqlRow, SqlStatement, StoreError, StoreFuture};

const GUARD: &str =
    "SELECT json(CASE WHEN changes() = CAST(?1 AS INTEGER) THEN 'true' ELSE ?2 END)";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Deferred {
    pub sql: &'static str,
    pub params: Vec<String>,
    pub changes: Option<u64>,
}

impl Deferred {
    #[must_use]
    pub fn statement(&self) -> SqlStatement<'_> {
        SqlStatement {
            sql: self.sql,
            params: self.params.iter().map(String::as_str).collect(),
        }
    }
}

pub type After = Box<dyn FnOnce() -> StoreFuture<'static, ()> + Send>;

pub trait Prepare: Send + Sync {
    fn prepare<'a>(&'a self, ledger: &'a dyn Sql) -> StoreFuture<'a, Result<(), StoreError>>;
}

pub(crate) fn returned(
    operation: &'static str,
    writes: &[Deferred],
    results: &[Vec<SqlRow>],
) -> Result<(), StoreError> {
    for (write, rows) in writes.iter().zip(results) {
        if let Some(changes) = write.changes
            && rows.len() as u64 != changes
        {
            return Err(StoreError::NotCommitted(format!(
                "{operation} expected {changes} changed rows and changed {}",
                rows.len()
            )));
        }
    }
    Ok(())
}

#[derive(Default)]
struct Queue {
    writes: Vec<(&'static str, Deferred)>,
    after: Vec<After>,
    failed: Option<String>,
}

pub struct Ledger<H: ?Sized> {
    host: Arc<H>,
    queue: Owned<Queue>,
    hooks: Owned<()>,
    prepare: Atom<Vec<Weak<dyn Prepare>>>,
    broken: AtomicBool,
}

impl<H: Sql + ?Sized + 'static> Ledger<H> {
    pub fn new(host: Arc<H>) -> Result<Self, StoreError> {
        if host.dialect() != SqlDialect::Sqlite {
            return Err(StoreError::Backend(
                "a ledger guards deferred writes with SQLite's changes()".to_owned(),
            ));
        }
        Ok(Self {
            host,
            queue: Owned::new(Queue::default()),
            hooks: Owned::new(()),
            prepare: Atom::new(Vec::new()),
            broken: AtomicBool::new(false),
        })
    }

    pub fn host(&self) -> &H {
        &self.host
    }

    pub fn broken(&self) -> bool {
        self.broken.load(Ordering::Acquire)
    }

    async fn prepared(&self) -> Result<(), StoreError>
    where
        H: 'static,
    {
        for prepare in self.prepare.get().iter() {
            if let Some(prepare) = prepare.upgrade() {
                prepare.prepare(self).await?;
            }
        }
        Ok(())
    }

    async fn commit(
        &self,
        access: SqlAccess,
        operation: &'static str,
        statements: &[SqlStatement<'_>],
    ) -> Result<Vec<Vec<SqlRow>>, StoreError> {
        if access != SqlAccess::Read {
            self.prepared().await?;
        }
        let mut queue = self.queue.acquire().await;
        if let Some(failed) = &queue.failed {
            return Err(StoreError::NotCommitted(failed.clone()));
        }
        if queue.writes.is_empty() {
            return self.host.run(access, operation, statements).await;
        }
        if access == SqlAccess::Schema {
            self.flush(&mut queue).await?;
            let hooks = self.hooks.acquire().await;
            let after = std::mem::take(&mut queue.after);
            drop(queue);
            run(after).await;
            drop(hooks);
            return self.host.run(access, operation, statements).await;
        }
        let guards: Vec<String> = queue
            .writes
            .iter()
            .map(|(operation, write)| {
                write.changes.map_or_else(String::new, |changes| {
                    format!("{operation} expected {changes} changed rows")
                })
            })
            .collect();
        let counts: Vec<String> = queue
            .writes
            .iter()
            .map(|(_, write)| write.changes.unwrap_or_default().to_string())
            .collect();
        let mut combined = Vec::with_capacity(queue.writes.len() * 2 + statements.len());
        for (at, (_, write)) in queue.writes.iter().enumerate() {
            combined.push(write.statement());
            if write.changes.is_some() {
                combined.push(SqlStatement {
                    sql: GUARD,
                    params: vec![counts[at].as_str(), guards[at].as_str()],
                });
            }
        }
        let own = combined.len();
        combined.extend(statements.iter().cloned());
        match self.host.run(SqlAccess::Write, operation, &combined).await {
            Ok(mut results) if results.len() == combined.len() => {
                queue.writes.clear();
                let after = std::mem::take(&mut queue.after);
                let hooks = self.hooks.acquire().await;
                drop(queue);
                run(after).await;
                drop(hooks);
                Ok(results.split_off(own))
            }
            Ok(results) => {
                let failure = format!(
                    "SQL host answered {} of {} ledger statements",
                    results.len(),
                    combined.len()
                );
                queue.failed = Some(failure.clone());
                self.broken.store(true, Ordering::Release);
                Err(StoreError::NotCommitted(failure))
            }
            Err(_) => {
                drop(combined);
                self.flush(&mut queue).await?;
                let hooks = self.hooks.acquire().await;
                let after = std::mem::take(&mut queue.after);
                drop(queue);
                run(after).await;
                drop(hooks);
                self.host.run(access, operation, statements).await
            }
        }
    }

    async fn flush(&self, queue: &mut Queue) -> Result<(), StoreError> {
        let counts: Vec<String> = queue
            .writes
            .iter()
            .map(|(_, write)| write.changes.unwrap_or_default().to_string())
            .collect();
        let guards: Vec<String> = queue
            .writes
            .iter()
            .map(|(operation, write)| {
                write.changes.map_or_else(String::new, |changes| {
                    format!("{operation} expected {changes} changed rows")
                })
            })
            .collect();
        let mut combined = Vec::with_capacity(queue.writes.len() * 2);
        for (at, (_, write)) in queue.writes.iter().enumerate() {
            combined.push(write.statement());
            if write.changes.is_some() {
                combined.push(SqlStatement {
                    sql: GUARD,
                    params: vec![counts[at].as_str(), guards[at].as_str()],
                });
            }
        }
        match self.host.run(SqlAccess::Write, "ledger", &combined).await {
            Ok(results) if results.len() == combined.len() => {
                drop(combined);
                queue.writes.clear();
                Ok(())
            }
            Ok(results) => {
                let failure = format!(
                    "SQL host answered {} of {} ledger statements",
                    results.len(),
                    combined.len()
                );
                drop(combined);
                queue.failed = Some(failure.clone());
                self.broken.store(true, Ordering::Release);
                queue.after.clear();
                Err(StoreError::NotCommitted(failure))
            }
            Err(error) => {
                drop(combined);
                let failure = format!("deferred writes could not commit: {error}");
                queue.failed = Some(failure.clone());
                self.broken.store(true, Ordering::Release);
                queue.after.clear();
                Err(StoreError::NotCommitted(failure))
            }
        }
    }

    async fn barrier_now(&self) -> Result<(), StoreError> {
        self.prepared().await?;
        let mut queue = self.queue.acquire().await;
        if let Some(failed) = &queue.failed {
            return Err(StoreError::NotCommitted(failed.clone()));
        }
        if !queue.writes.is_empty() {
            self.flush(&mut queue).await?;
        }
        let hooks = self.hooks.acquire().await;
        let after = std::mem::take(&mut queue.after);
        drop(queue);
        run(after).await;
        drop(hooks);
        Ok(())
    }

    async fn enqueue(
        &self,
        operation: &'static str,
        writes: Vec<Deferred>,
        after: Option<After>,
    ) -> Result<(), StoreError> {
        let mut queue = self.queue.acquire().await;
        if let Some(failed) = &queue.failed {
            return Err(StoreError::NotCommitted(failed.clone()));
        }
        queue
            .writes
            .extend(writes.into_iter().map(|write| (operation, write)));
        queue.after.extend(after);
        Ok(())
    }
}

async fn run(after: Vec<After>) {
    for after in after {
        after().await;
    }
}

impl<H: Sql + ?Sized + 'static> Sql for Ledger<H> {
    fn transaction<'a>(
        &'a self,
        operation: &'static str,
        statements: &'a [SqlStatement<'a>],
    ) -> StoreFuture<'a, Result<Vec<Vec<SqlRow>>, StoreError>> {
        Box::pin(self.commit(SqlAccess::Write, operation, statements))
    }

    fn run<'a>(
        &'a self,
        access: SqlAccess,
        operation: &'static str,
        statements: &'a [SqlStatement<'a>],
    ) -> StoreFuture<'a, Result<Vec<Vec<SqlRow>>, StoreError>> {
        Box::pin(self.commit(access, operation, statements))
    }

    fn write<'a>(
        &'a self,
        _settle: Settle,
        operation: &'static str,
        statements: &'a [SqlStatement<'a>],
    ) -> StoreFuture<'a, Result<Vec<Vec<SqlRow>>, StoreError>> {
        Box::pin(self.commit(SqlAccess::Write, operation, statements))
    }

    fn dialect(&self) -> SqlDialect {
        self.host.dialect()
    }

    fn defers(&self) -> bool {
        true
    }

    fn defer<'a>(
        &'a self,
        operation: &'static str,
        writes: Vec<Deferred>,
        after: Option<After>,
    ) -> StoreFuture<'a, Result<(), StoreError>> {
        Box::pin(self.enqueue(operation, writes, after))
    }

    fn barrier(&self) -> StoreFuture<'_, Result<(), StoreError>> {
        Box::pin(self.barrier_now())
    }

    fn prepare_with(&self, prepare: Weak<dyn Prepare>) {
        self.prepare.update(|all| {
            all.retain(|known| known.strong_count() > 0);
            all.push(Weak::clone(&prepare));
        });
    }
}
