use std::{path::Path, thread};

use rusqlite::{Connection, TransactionBehavior, types::ValueRef};
use tokio::sync::{mpsc, oneshot};

use nanocode_durability_core::{Sql, SqlRow, SqlStatement, StoreError, StoreFuture};

const JOB_CAPACITY: usize = 1;

type Rows = Result<Vec<Vec<SqlRow>>, StoreError>;

struct Job {
    statements: Vec<(&'static str, Vec<String>)>,
    reply: oneshot::Sender<Rows>,
}

pub struct SqliteSql {
    jobs: mpsc::Sender<Job>,
}

impl SqliteSql {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let connection = Connection::open(path).map_err(backend)?;
        Self::from_connection(connection)
    }

    pub fn from_connection(connection: Connection) -> Result<Self, StoreError> {
        configure(&connection)?;
        let (jobs, receiver) = mpsc::channel(JOB_CAPACITY);
        thread::Builder::new()
            .name("nanocode-sqlite-sql".into())
            .spawn(move || serve(connection, receiver))
            .map_err(|error| StoreError::Backend(error.to_string()))?;
        Ok(Self { jobs })
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
                drop(opened.send(Ok(())));
                serve(connection, receiver);
            })
            .map_err(|error| StoreError::Backend(error.to_string()))?;
        ready.await.map_err(|_| stopped())??;
        Ok(Self { jobs })
    }
}

impl Sql for SqliteSql {
    fn transaction<'a>(
        &'a self,
        _operation: &'static str,
        statements: &'a [SqlStatement<'a>],
    ) -> StoreFuture<'a, Rows> {
        let statements = statements
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
        Box::pin(async move {
            let (reply, result) = oneshot::channel();
            self.jobs
                .send(Job { statements, reply })
                .await
                .map_err(|_| stopped())?;
            result.await.map_err(|_| stopped())?
        })
    }
}

fn configure(connection: &Connection) -> Result<(), StoreError> {
    connection
        .execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL; PRAGMA busy_timeout = 5000;",
        )
        .map_err(backend)
}

fn serve(mut connection: Connection, mut jobs: mpsc::Receiver<Job>) {
    while let Some(job) = jobs.blocking_recv() {
        drop(job.reply.send(run(&mut connection, &job.statements)));
    }
    drop(connection.close());
}

fn run(connection: &mut Connection, statements: &[(&'static str, Vec<String>)]) -> Rows {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
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
