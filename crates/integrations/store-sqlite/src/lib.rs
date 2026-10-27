#![cfg(not(target_family = "wasm"))]

use std::{
    path::{Path, PathBuf},
    thread,
};

use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};
use tokio::sync::{mpsc, oneshot};

use nanocode_durability_core::{
    OwnedState, OwnerId, OwnerToken, StateStore, StoreError, StoreFuture, StoredState,
};

mod sql;

pub use sql::SqliteSql;

const MAX_SQL_REVISION: u64 = i64::MAX as u64;
const JOB_CAPACITY: usize = 1;

type Job = Box<dyn FnOnce(&mut Database) + Send>;

struct Database {
    connection: Connection,
}

pub struct SqliteStore {
    jobs: mpsc::Sender<Job>,
    closed: oneshot::Receiver<Result<(), StoreError>>,
}

impl SqliteStore {
    #[must_use]
    pub fn home_path(home: &Path) -> PathBuf {
        home.join("sessions.sqlite")
    }

    pub fn open_home(home: &Path) -> Result<Self, StoreError> {
        Self::open(Database::home(home)?)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let connection = Connection::open(path).map_err(backend)?;
        Self::from_connection(connection)
    }

    pub fn from_connection(connection: Connection) -> Result<Self, StoreError> {
        let database = Database::initialize(connection)?;
        Self::start(move || Ok(database)).map(|(store, _)| store)
    }

    pub async fn open_home_async(home: &Path) -> Result<Self, StoreError> {
        let home = home.to_owned();
        Self::opened(Self::start(move || Database::open(Database::home(&home)?))?).await
    }

    pub async fn open_async(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_owned();
        Self::opened(Self::start(move || Database::open(path))?).await
    }

    pub async fn from_connection_async(connection: Connection) -> Result<Self, StoreError> {
        Self::opened(Self::start(move || Database::initialize(connection))?).await
    }

    pub async fn close(self) -> Result<(), StoreError> {
        let Self { jobs, closed } = self;
        drop(jobs);
        closed.await.map_err(|_| stopped())?
    }

    fn start(
        open: impl FnOnce() -> Result<Database, StoreError> + Send + 'static,
    ) -> Result<(Self, oneshot::Receiver<Result<(), StoreError>>), StoreError> {
        let (jobs, mut receiver) = mpsc::channel::<Job>(JOB_CAPACITY);
        let (finished, closed) = oneshot::channel();
        let (ready, opened) = oneshot::channel();
        thread::Builder::new()
            .name("nanocode-sqlite".into())
            .spawn(move || {
                let mut database = match open() {
                    Ok(database) => database,
                    Err(error) => return drop(ready.send(Err(error))),
                };
                drop(ready.send(Ok(())));
                while let Some(job) = receiver.blocking_recv() {
                    job(&mut database);
                }
                let closed = database
                    .connection
                    .close()
                    .map_err(|(_, error)| backend(error));
                drop(finished.send(closed));
            })
            .map_err(|error| StoreError::Backend(error.to_string()))?;
        Ok((Self { jobs, closed }, opened))
    }

    async fn opened(
        (store, opened): (Self, oneshot::Receiver<Result<(), StoreError>>),
    ) -> Result<Self, StoreError> {
        opened.await.map_err(|_| stopped())??;
        Ok(store)
    }

    fn write<T: Send + 'static>(
        &mut self,
        action: impl FnOnce(&mut Database) -> Result<T, StoreError> + Send + 'static,
    ) -> StoreFuture<'_, Result<T, StoreError>> {
        self.dispatch(false, action)
    }

    fn read<T: Send + 'static>(
        &mut self,
        action: impl FnOnce(&mut Database) -> Result<T, StoreError> + Send + 'static,
    ) -> StoreFuture<'_, Result<T, StoreError>> {
        self.dispatch(true, action)
    }

    fn dispatch<T: Send + 'static>(
        &mut self,
        abandonable: bool,
        action: impl FnOnce(&mut Database) -> Result<T, StoreError> + Send + 'static,
    ) -> StoreFuture<'_, Result<T, StoreError>> {
        let (reply, result) = oneshot::channel();
        let job: Job = Box::new(move |database| {
            if abandonable && reply.is_closed() {
                return;
            }
            drop(reply.send(action(database)));
        });
        Box::pin(async move {
            self.jobs.send(job).await.map_err(|_| stopped())?;
            result.await.map_err(|_| stopped())?
        })
    }
}

impl Database {
    fn home(home: &Path) -> Result<PathBuf, StoreError> {
        std::fs::create_dir_all(home).map_err(|error| {
            StoreError::NotCommitted(format!("failed to create {}: {error}", home.display()))
        })?;
        Ok(SqliteStore::home_path(home))
    }

    fn open(path: PathBuf) -> Result<Self, StoreError> {
        Self::initialize(Connection::open(path).map_err(backend)?)
    }

    fn initialize(connection: Connection) -> Result<Self, StoreError> {
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE IF NOT EXISTS nanocode_durable_records (
                   state_id TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL,
                   PRIMARY KEY (state_id, key)
                 ) WITHOUT ROWID;
                 CREATE TABLE IF NOT EXISTS nanocode_durable_owners (
                   state_id TEXT PRIMARY KEY,
                   owner_id TEXT NOT NULL,
                   fence TEXT NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS nanocode_durable_states (
                   state_id TEXT PRIMARY KEY,
                   revision INTEGER NOT NULL CHECK (revision > 0),
                   payload TEXT NOT NULL
                 );",
            )
            .map_err(backend)?;
        validate_owner_schema(&connection)?;
        validate_state_schema(&connection)?;
        Ok(Self { connection })
    }

    fn acquire_transactional(
        &mut self,
        state_id: &str,
        owner_id: OwnerId,
        after_begin: impl FnOnce(),
    ) -> Result<OwnedState, StoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(backend)?;
        after_begin();
        let retained_fence = transaction
            .query_row(
                "SELECT fence FROM nanocode_durable_owners WHERE state_id = ?1",
                [state_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(backend)?;
        let fence = match retained_fence {
            Some(retained) => {
                let retained = parse_u64(&retained, "SQLite owner fence")?;
                retained.checked_add(1).ok_or_else(|| {
                    StoreError::NotCommitted("SQLite durability owner fence overflow".to_owned())
                })?
            }
            None => 1,
        };
        transaction
            .execute(
                "INSERT INTO nanocode_durable_owners (state_id, owner_id, fence)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT (state_id) DO UPDATE
                 SET owner_id = excluded.owner_id, fence = excluded.fence",
                params![state_id, owner_id.as_str(), fence.to_string()],
            )
            .map_err(backend)?;
        let state = load_state(&transaction, state_id)?;
        let owner = OwnerToken::new(owner_id, fence);
        transaction.commit().map_err(backend)?;
        Ok(OwnedState { owner, state })
    }

    fn replace_transactional(
        &mut self,
        state_id: &str,
        owner: &OwnerToken,
        expected_revision: u64,
        payload: &str,
        records: &[nanocode_durability_core::StoreRecord],
        after_begin: impl FnOnce(),
    ) -> Result<u64, StoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(backend)?;
        after_begin();
        let retained_owner = transaction
            .query_row(
                "SELECT owner_id, fence FROM nanocode_durable_owners WHERE state_id = ?1",
                [state_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(backend)?;
        let owns_state = match retained_owner {
            Some((owner_id, fence)) => {
                owner_id == owner.owner_id().as_str()
                    && parse_u64(&fence, "SQLite owner fence")? == owner.fence()
            }
            None => false,
        };
        if !owns_state {
            return Err(StoreError::Fenced);
        }
        let actual = transaction
            .query_row(
                "SELECT revision FROM nanocode_durable_states WHERE state_id = ?1",
                [state_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(backend)?
            .unwrap_or(0);
        let actual = to_u64(actual)?;
        if actual != expected_revision {
            return Err(StoreError::Conflict {
                expected: expected_revision,
                actual,
            });
        }
        let revision = actual.checked_add(1).ok_or_else(|| {
            StoreError::NotCommitted("SQLite durability revision overflow".to_owned())
        })?;
        for record in records {
            transaction.execute(
                "INSERT INTO nanocode_durable_records (state_id, key, value) VALUES (?1, ?2, ?3)
                 ON CONFLICT (state_id, key) DO NOTHING", params![state_id, record.key, record.value],
            ).map_err(backend)?;
            let retained: String = transaction
                .query_row(
                    "SELECT value FROM nanocode_durable_records WHERE state_id = ?1 AND key = ?2",
                    params![state_id, record.key],
                    |row| row.get(0),
                )
                .map_err(backend)?;
            if retained != record.value {
                return Err(StoreError::NotCommitted(format!(
                    "durability record `{}` has conflicting content",
                    record.key
                )));
            }
        }
        let sql_revision = sql_counter(revision, "SQLite durability revision overflow")?;
        transaction
            .execute(
                "INSERT INTO nanocode_durable_states (state_id, revision, payload)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT (state_id) DO UPDATE
                 SET revision = excluded.revision, payload = excluded.payload",
                params![state_id, sql_revision, payload],
            )
            .map_err(backend)?;
        transaction.commit().map_err(backend)?;
        Ok(revision)
    }

    fn read_record(&self, state_id: &str, key: &str) -> Result<Option<String>, StoreError> {
        self.connection
            .query_row(
                "SELECT value FROM nanocode_durable_records WHERE state_id = ?1 AND key = ?2",
                params![state_id, key],
                |row| row.get(0),
            )
            .optional()
            .map_err(backend)
    }

    fn peek(&self, state_id: &str) -> Result<StoredState, StoreError> {
        self.connection
            .query_row(
                "SELECT revision, payload FROM nanocode_durable_states WHERE state_id = ?1",
                [state_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(backend)
            .and_then(|row| match row {
                None => Ok(StoredState::default()),
                Some((revision, payload)) => Ok(StoredState {
                    revision: u64::try_from(revision).map_err(|_| {
                        StoreError::Backend("SQLite state revision is negative".to_owned())
                    })?,
                    payload: Some(payload),
                }),
            })
    }

    fn owner(&self, state_id: &str) -> Result<Option<OwnerToken>, StoreError> {
        self.connection
            .query_row(
                "SELECT owner_id, fence FROM nanocode_durable_owners WHERE state_id = ?1",
                [state_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(backend)?
            .map(|(owner_id, fence)| {
                Ok(OwnerToken::new(
                    OwnerId::from(owner_id),
                    parse_u64(&fence, "SQLite owner fence")?,
                ))
            })
            .transpose()
    }

    fn list_records(&self, state_id: &str) -> Result<Vec<String>, StoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT key FROM nanocode_durable_records WHERE state_id = ?1")
            .map_err(backend)?;
        statement
            .query_map([state_id], |row| row.get::<_, String>(0))
            .map_err(backend)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(backend)
    }

    fn list_states(&self, limit: usize) -> Result<Vec<String>, StoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT state_id FROM nanocode_durable_states ORDER BY rowid DESC LIMIT ?1")
            .map_err(backend)?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        statement
            .query_map([limit], |row| row.get::<_, String>(0))
            .map_err(backend)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(backend)
    }
}

impl StateStore for SqliteStore {
    fn read_record<'a>(
        &'a mut self,
        state_id: &'a str,
        key: &'a str,
    ) -> StoreFuture<'a, Result<Option<String>, StoreError>> {
        let (state_id, key) = (state_id.to_owned(), key.to_owned());
        self.read(move |database| database.read_record(&state_id, &key))
    }

    fn read_records<'a>(
        &'a mut self,
        state_id: &'a str,
        keys: &'a [String],
    ) -> StoreFuture<'a, Result<Vec<Option<String>>, StoreError>> {
        let (state_id, keys) = (state_id.to_owned(), keys.to_vec());
        self.read(move |database| {
            keys.iter()
                .map(|key| database.read_record(&state_id, key))
                .collect()
        })
    }

    fn acquire<'a>(
        &'a mut self,
        state_id: &'a str,
        owner_id: OwnerId,
    ) -> StoreFuture<'a, Result<OwnedState, StoreError>> {
        let state_id = state_id.to_owned();
        self.write(move |database| database.acquire_transactional(&state_id, owner_id, || {}))
    }

    fn replace<'a>(
        &'a mut self,
        state_id: &'a str,
        owner: &'a OwnerToken,
        expected_revision: u64,
        payload: &'a str,
        records: &'a [nanocode_durability_core::StoreRecord],
    ) -> StoreFuture<'a, Result<u64, StoreError>> {
        let (state_id, owner, payload, records) = (
            state_id.to_owned(),
            owner.clone(),
            payload.to_owned(),
            records.to_vec(),
        );
        self.write(move |database| {
            database.replace_transactional(
                &state_id,
                &owner,
                expected_revision,
                &payload,
                &records,
                || {},
            )
        })
    }

    fn peek<'a>(
        &'a mut self,
        state_id: &'a str,
    ) -> StoreFuture<'a, Result<StoredState, StoreError>> {
        let state_id = state_id.to_owned();
        self.read(move |database| database.peek(&state_id))
    }

    fn list_states<'a>(
        &'a mut self,
        limit: usize,
    ) -> StoreFuture<'a, Result<Vec<String>, StoreError>> {
        self.read(move |database| database.list_states(limit))
    }

    fn list_records<'a>(
        &'a mut self,
        state_id: &'a str,
    ) -> StoreFuture<'a, Result<Vec<String>, StoreError>> {
        let state_id = state_id.to_owned();
        self.read(move |database| database.list_records(&state_id))
    }

    fn owner<'a>(
        &'a mut self,
        state_id: &'a str,
    ) -> StoreFuture<'a, Result<Option<OwnerToken>, StoreError>> {
        let state_id = state_id.to_owned();
        self.read(move |database| database.owner(&state_id))
    }

    fn flush(&mut self) -> StoreFuture<'_, Result<(), StoreError>> {
        self.write(|_| Ok(()))
    }
}

fn validate_owner_schema(connection: &Connection) -> Result<(), StoreError> {
    let mut statement = connection
        .prepare("PRAGMA table_info('nanocode_durable_owners')")
        .map_err(backend)?;
    let columns = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, bool>(3)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .map_err(backend)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(backend)?;
    let expected = [
        ("state_id", "TEXT", false, 1),
        ("owner_id", "TEXT", true, 0),
        ("fence", "TEXT", true, 0),
    ];
    if columns.len() != expected.len() {
        return Err(incompatible_owner_schema(format!(
            "expected exactly {} columns, found {}",
            expected.len(),
            columns.len()
        )));
    }
    for (name, declared_type, not_null, primary_key) in expected {
        let Some((_, actual_type, actual_not_null, actual_primary_key)) =
            columns.iter().find(|(actual_name, ..)| actual_name == name)
        else {
            return Err(incompatible_owner_schema(format!(
                "missing required `{name}` column"
            )));
        };
        if !actual_type.eq_ignore_ascii_case(declared_type) {
            return Err(incompatible_owner_schema(format!(
                "`{name}` must be declared {declared_type}, found {actual_type}"
            )));
        }
        if not_null && !actual_not_null {
            return Err(incompatible_owner_schema(format!(
                "`{name}` has an incompatible NOT NULL constraint"
            )));
        }
        if *actual_primary_key != primary_key {
            return Err(incompatible_owner_schema(format!(
                "`{name}` has an incompatible PRIMARY KEY constraint"
            )));
        }
    }
    if columns
        .iter()
        .any(|(name, _, _, primary_key)| name != "state_id" && *primary_key != 0)
    {
        return Err(incompatible_owner_schema(
            "the PRIMARY KEY must contain only `state_id`".to_owned(),
        ));
    }
    Ok(())
}

fn validate_state_schema(connection: &Connection) -> Result<(), StoreError> {
    let mut statement = connection
        .prepare("PRAGMA table_info('nanocode_durable_states')")
        .map_err(backend)?;
    let columns = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, bool>(3)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .map_err(backend)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(backend)?;
    let expected = [
        ("state_id", "TEXT", false, 1),
        ("revision", "INTEGER", true, 0),
        ("payload", "TEXT", true, 0),
    ];
    if columns.len() != expected.len() {
        return Err(incompatible_state_schema(format!(
            "expected exactly {} columns, found {}",
            expected.len(),
            columns.len()
        )));
    }
    for (name, declared_type, not_null, primary_key) in expected {
        let Some((_, actual_type, actual_not_null, actual_primary_key)) =
            columns.iter().find(|(actual_name, ..)| actual_name == name)
        else {
            return Err(incompatible_state_schema(format!(
                "missing required `{name}` column"
            )));
        };
        if !actual_type.eq_ignore_ascii_case(declared_type)
            || (not_null && !actual_not_null)
            || *actual_primary_key != primary_key
        {
            return Err(incompatible_state_schema(format!(
                "`{name}` has an incompatible column shape"
            )));
        }
    }
    Ok(())
}

fn incompatible_owner_schema(detail: String) -> StoreError {
    StoreError::Backend(format!(
        "incompatible SQLite `nanocode_durable_owners` schema: {detail}; recreate the table with the current schema"
    ))
}

fn incompatible_state_schema(detail: String) -> StoreError {
    StoreError::Backend(format!(
        "incompatible SQLite `nanocode_durable_states` schema: {detail}; recreate the table with the current schema"
    ))
}

fn load_state(connection: &Connection, state_id: &str) -> Result<StoredState, StoreError> {
    let retained = connection
        .query_row(
            "SELECT revision, payload FROM nanocode_durable_states WHERE state_id = ?1",
            [state_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(backend)?;
    match retained {
        Some((revision, payload)) => Ok(StoredState {
            revision: to_u64(revision)?,
            payload: Some(payload),
        }),
        None => Ok(StoredState::default()),
    }
}

fn backend(error: rusqlite::Error) -> StoreError {
    StoreError::Backend(error.to_string())
}

fn stopped() -> StoreError {
    StoreError::Backend("SQLite durability worker stopped".to_owned())
}

fn to_u64(value: i64) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|error| StoreError::Backend(error.to_string()))
}

fn parse_u64(value: &str, label: &str) -> Result<u64, StoreError> {
    value
        .parse()
        .map_err(|error| StoreError::Backend(format!("invalid {label}: {error}")))
}

fn sql_counter(value: u64, overflow: &str) -> Result<i64, StoreError> {
    debug_assert_eq!(MAX_SQL_REVISION, i64::MAX as u64);
    i64::try_from(value).map_err(|_| StoreError::NotCommitted(overflow.to_owned()))
}
