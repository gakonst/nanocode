use std::{path::Path, thread};

use rusqlite::{Connection, TransactionBehavior, types::ValueRef};
use tokio::sync::{mpsc, oneshot};

use nanocode_durability_core::{Sql, SqlRow, SqlStatement, StoreError, StoreFuture};

const JOB_CAPACITY: usize = 1;

const STATEMENT_CACHE: usize = 64;

type Rows = Result<Vec<Vec<SqlRow>>, StoreError>;

struct Job {
    statements: Vec<(&'static str, Vec<String>)>,
    reply: oneshot::Sender<Rows>,
}

pub struct SqliteSql {
    jobs: mpsc::Sender<Job>,
    readers: Vec<mpsc::Sender<Job>>,
    next_reader: std::sync::atomic::AtomicUsize,
}

impl SqliteSql {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let connection = Connection::open(path).map_err(backend)?;
        Self::from_connection(connection)
    }

    pub fn from_connection(connection: Connection) -> Result<Self, StoreError> {
        configure(&connection)?;
        let checkpoints = checkpointer(&connection);
        let readers = readers(&connection);
        let (jobs, receiver) = mpsc::channel(JOB_CAPACITY);
        thread::Builder::new()
            .name("nanocode-sqlite-sql".into())
            .spawn(move || {
                serve(
                    connection,
                    receiver,
                    checkpoints,
                    TransactionBehavior::Immediate,
                )
            })
            .map_err(|error| StoreError::Backend(error.to_string()))?;
        Ok(Self {
            jobs,
            readers,
            next_reader: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    pub async fn open_async(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_owned();
        let (opened, ready) = oneshot::channel();
        let (jobs, receiver) = mpsc::channel(JOB_CAPACITY);
        thread::Builder::new()
            .name("nanocode-sqlite-sql".into())
            .spawn(move || {
                let connection = match Connection::open(&path)
                    .map_err(backend)
                    .and_then(|connection| configure(&connection).map(|()| connection))
                {
                    Ok(connection) => connection,
                    Err(error) => return drop(opened.send(Err(error))),
                };
                let checkpoints = checkpointer(&connection);
                drop(opened.send(Ok(readers(&connection))));
                serve(
                    connection,
                    receiver,
                    checkpoints,
                    TransactionBehavior::Immediate,
                );
            })
            .map_err(|error| StoreError::Backend(error.to_string()))?;
        let readers = ready.await.map_err(|_| stopped())??;
        Ok(Self {
            jobs,
            readers,
            next_reader: std::sync::atomic::AtomicUsize::new(0),
        })
    }
}

impl Sql for SqliteSql {
    fn transaction<'a>(
        &'a self,
        _operation: &'static str,
        statements: &'a [SqlStatement<'a>],
    ) -> StoreFuture<'a, Rows> {
        let statements: Vec<(&'static str, Vec<String>)> = statements
            .iter()
            .map(|statement| {
                (
                    statement.sql,
                    statement
                        .params
                        .iter()
                        .map(|param| (*param).to_owned())
                        .collect(),
                )
            })
            .collect();
        let reads =
            !self.readers.is_empty() && statements.iter().all(|(sql, _)| sql.starts_with("SELECT"));
        let worker = if reads {
            let next = self
                .next_reader
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            &self.readers[next % self.readers.len()]
        } else {
            &self.jobs
        };
        Box::pin(async move {
            let (reply, result) = oneshot::channel();
            worker
                .send(Job { statements, reply })
                .await
                .map_err(|_| stopped())?;
            result.await.map_err(|_| stopped())?
        })
    }

    fn concurrent(self: std::sync::Arc<Self>) -> Option<std::sync::Arc<dyn Sql>> {
        Some(self)
    }
}

fn configure(connection: &Connection) -> Result<(), StoreError> {
    connection.set_prepared_statement_cache_capacity(STATEMENT_CACHE);
    connection
        .execute_batch(
            "PRAGMA page_size = 16384; PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL; PRAGMA busy_timeout = 5000; PRAGMA cache_size = -262144; PRAGMA mmap_size = 4294967296; PRAGMA temp_store = MEMORY;",
        )
        .map_err(backend)
}

fn readers(connection: &Connection) -> Vec<mpsc::Sender<Job>> {
    let Some(path) = connection
        .path()
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
    else {
        return Vec::new();
    };
    let count = thread::available_parallelism().map_or(1, std::num::NonZero::get);
    (0..count)
        .map_while(|_| {
            let reader = Connection::open_with_flags(
                &path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                    | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .ok()?;
            reader.set_prepared_statement_cache_capacity(STATEMENT_CACHE);
            reader
                .execute_batch("PRAGMA busy_timeout = 5000; PRAGMA mmap_size = 4294967296;")
                .ok()?;
            let (jobs, receiver) = mpsc::channel(JOB_CAPACITY);
            thread::Builder::new()
                .name("nanocode-sqlite-read".into())
                .spawn(move || serve(reader, receiver, None, TransactionBehavior::Deferred))
                .ok()?;
            Some(jobs)
        })
        .collect()
}

struct Checkpoints {
    wake: std::sync::mpsc::SyncSender<()>,
    draining: std::sync::Arc<std::sync::atomic::AtomicBool>,
    log: std::path::PathBuf,
    database: std::path::PathBuf,
}

impl Checkpoints {
    fn committed(&self, connection: &Connection) {
        let oversized = || {
            matches!(
                (std::fs::metadata(&self.log), std::fs::metadata(&self.database)),
                (Ok(log), Ok(database)) if log.len() > database.len()
            )
        };
        if !self.draining.load(std::sync::atomic::Ordering::Acquire) && !oversized() {
            let _ = self.wake.try_send(());
            return;
        }
        self.draining
            .store(true, std::sync::atomic::Ordering::Release);
        let truncated = connection
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                row.get::<_, i64>(0)
            })
            .is_ok_and(|busy| busy == 0);
        if truncated {
            self.draining
                .store(false, std::sync::atomic::Ordering::Release);
        }
    }
}

fn checkpointer(connection: &Connection) -> Option<Checkpoints> {
    let path = connection
        .path()
        .filter(|path| !path.is_empty())?
        .to_owned();
    connection
        .execute_batch("PRAGMA wal_autocheckpoint = 0;")
        .ok()?;
    let (wake, woken) = std::sync::mpsc::sync_channel(1);
    let draining = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let drained = std::sync::Arc::clone(&draining);
    let database = std::path::PathBuf::from(&path);
    let log = std::path::PathBuf::from(format!("{path}-wal"));
    thread::Builder::new()
        .name("nanocode-sqlite-checkpoint".into())
        .spawn(move || {
            let Ok(checkpoints) = Connection::open(&path) else {
                return;
            };
            if checkpoints
                .execute_batch("PRAGMA busy_timeout = 5000;")
                .is_err()
            {
                return;
            }
            while woken.recv().is_ok() {
                if !drained.load(std::sync::atomic::Ordering::Acquire) {
                    let _ = checkpoints.execute_batch("PRAGMA wal_checkpoint(PASSIVE);");
                }
            }
        })
        .ok()?;
    Some(Checkpoints {
        wake,
        draining,
        log,
        database,
    })
}

fn serve(
    mut connection: Connection,
    mut jobs: mpsc::Receiver<Job>,
    checkpoints: Option<Checkpoints>,
    behavior: TransactionBehavior,
) {
    while let Some(job) = jobs.blocking_recv() {
        drop(
            job.reply
                .send(run(&mut connection, &job.statements, behavior)),
        );
        if let Some(checkpoints) = &checkpoints {
            checkpoints.committed(&connection);
        }
    }
    drop(connection.close());
}

fn run(
    connection: &mut Connection,
    statements: &[(&'static str, Vec<String>)],
    behavior: TransactionBehavior,
) -> Rows {
    let transaction = connection
        .transaction_with_behavior(behavior)
        .map_err(not_committed)?;
    let mut results = Vec::with_capacity(statements.len());
    for (sql, params) in statements {
        let mut prepared = transaction.prepare_cached(sql).map_err(not_committed)?;
        let columns = prepared.column_count();
        let mut rows = prepared
            .query(rusqlite::params_from_iter(params.iter()))
            .map_err(not_committed)?;
        let mut collected = Vec::new();
        while let Some(row) = rows.next().map_err(not_committed)? {
            let mut cells = Vec::with_capacity(columns);
            for column in 0..columns {
                cells.push(text(row.get_ref(column).map_err(not_committed)?)?);
            }
            collected.push(cells);
        }
        results.push(collected);
    }
    transaction.commit().map_err(backend)?;
    Ok(results)
}

fn text(value: ValueRef<'_>) -> Result<Option<String>, StoreError> {
    Ok(match value {
        ValueRef::Null => None,
        ValueRef::Integer(value) => Some(value.to_string()),
        ValueRef::Real(value) => Some(value.to_string()),
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => {
            Some(String::from_utf8(bytes.to_vec()).map_err(|error| {
                StoreError::NotCommitted(format!("SQLite returned non-UTF-8 text: {error}"))
            })?)
        }
    })
}

fn not_committed(error: rusqlite::Error) -> StoreError {
    StoreError::NotCommitted(error.to_string())
}

fn backend(error: rusqlite::Error) -> StoreError {
    StoreError::Backend(error.to_string())
}

fn stopped() -> StoreError {
    StoreError::Backend("SQLite SQL worker stopped".to_owned())
}
