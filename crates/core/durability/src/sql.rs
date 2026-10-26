use std::sync::Arc;

use serde::ser::SerializeSeq;

use crate::{
    Committed, OwnedCommit, OwnedState, OwnerId, OwnerToken, StateStore, StoreError, StoreFuture,
    StoreRecord, StoredState,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqlStatement<'a> {
    pub sql: &'static str,
    pub params: Vec<&'a str>,
}

impl serde::Serialize for SqlStatement<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.params.len() + 1))?;
        sequence.serialize_element(self.sql)?;
        for param in &self.params {
            sequence.serialize_element(param)?;
        }
        sequence.end()
    }
}

pub type SqlRow = Vec<Option<String>>;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum SqlDialect {
    #[default]
    Sqlite,
    Postgres,
}

impl SqlDialect {
    #[must_use]
    pub const fn pick(self, statements: &[&'static str; 2]) -> &'static str {
        statements[self as usize]
    }
}

#[cfg(not(target_family = "wasm"))]
pub trait Sql: Send + Sync {
    fn transaction<'a>(
        &'a self,
        operation: &'static str,
        statements: &'a [SqlStatement<'a>],
    ) -> StoreFuture<'a, Result<Vec<Vec<SqlRow>>, StoreError>>;

    fn dialect(&self) -> SqlDialect {
        SqlDialect::Sqlite
    }
}

#[cfg(target_family = "wasm")]
pub trait Sql {
    fn transaction<'a>(
        &'a self,
        operation: &'static str,
        statements: &'a [SqlStatement<'a>],
    ) -> StoreFuture<'a, Result<Vec<Vec<SqlRow>>, StoreError>>;

    fn dialect(&self) -> SqlDialect {
        SqlDialect::Sqlite
    }
}

type Dialects = [&'static str; 2];

#[derive(Clone, Copy, Debug)]
pub struct MissingColumn {
    pub probe: Dialects,
    pub add: Dialects,
}

#[derive(Clone, Copy, Debug)]
pub struct Migration {
    pub statements: [&'static [&'static str]; 2],
    pub adopt: Option<MissingColumn>,
}

const CREATE_FORMATS: Dialects = [
    "CREATE TABLE IF NOT EXISTS nanocode_formats (component TEXT PRIMARY KEY, version INTEGER NOT NULL)",
    "CREATE TABLE IF NOT EXISTS nanocode_formats (component TEXT PRIMARY KEY, version BIGINT NOT NULL)",
];
const READ_FORMAT: Dialects = [
    "SELECT CAST(version AS TEXT) FROM nanocode_formats WHERE component = ?1",
    "SELECT CAST(version AS TEXT) FROM nanocode_formats WHERE component = $1::text",
];
const WRITE_FORMAT: Dialects = [
    "INSERT INTO nanocode_formats (component, version) VALUES (?1, CAST(?2 AS INTEGER)) ON CONFLICT (component) DO UPDATE SET version = excluded.version",
    "INSERT INTO nanocode_formats (component, version) VALUES ($1::text, CAST($2::text AS BIGINT)) ON CONFLICT (component) DO UPDATE SET version = excluded.version",
];

async fn run_checked<H: Sql + ?Sized>(
    host: &H,
    operation: &'static str,
    statements: &[SqlStatement<'_>],
) -> Result<Vec<Vec<SqlRow>>, StoreError> {
    let results = host.transaction(operation, statements).await?;
    if results.len() != statements.len() {
        return Err(StoreError::Backend(format!(
            "SQL host answered {} of {} statements",
            results.len(),
            statements.len()
        )));
    }
    Ok(results)
}

pub async fn migrate_format<H: Sql + ?Sized>(
    host: &H,
    component: &'static str,
    migrations: &[Migration],
) -> Result<(), StoreError> {
    let dialect = host.dialect();
    let results = run_checked(
        host,
        "format",
        &[
            statement(dialect, &CREATE_FORMATS, Vec::new()),
            statement(dialect, &READ_FORMAT, vec![component]),
        ],
    )
    .await?;
    let stored = match single(&results[1])? {
        Some(row) => counter(row, 0)?,
        None => 0,
    };
    let known = u32::try_from(migrations.len()).unwrap_or(u32::MAX);
    if stored > u64::from(known) {
        return Err(StoreError::UnsupportedFormat {
            component,
            stored,
            known,
        });
    }
    for (index, migration) in migrations.iter().enumerate().skip(stored as usize) {
        let version = (index + 1).to_string();
        let write = statement(dialect, &WRITE_FORMAT, vec![component, &version]);
        let schema = migration.statements[dialect as usize]
            .iter()
            .map(|sql| SqlStatement {
                sql,
                params: Vec::new(),
            });
        let Some(adopt) = migration.adopt else {
            let statements: Vec<_> = schema.chain([write]).collect();
            run_checked(host, "format", &statements).await?;
            continue;
        };
        let probe = || statement(dialect, &adopt.probe, Vec::new());
        let statements: Vec<_> = schema.chain([probe()]).collect();
        let results = run_checked(host, "format", &statements).await?;
        if !column_present(&results[statements.len() - 1])? {
            let add = [statement(dialect, &adopt.add, Vec::new())];
            if let Err(error) = run_checked(host, "format", &add).await
                && !column_present(&run_checked(host, "format", &[probe()]).await?[0])?
            {
                return Err(error);
            }
        }
        run_checked(host, "format", &[write]).await?;
    }
    Ok(())
}

fn column_present(rows: &[SqlRow]) -> Result<bool, StoreError> {
    match single(rows)? {
        Some(row) => Ok(counter(row, 0)? > 0),
        None => Ok(false),
    }
}

const SCHEMA: [&[&str]; 2] = [
    &[
        "CREATE TABLE IF NOT EXISTS states (id TEXT PRIMARY KEY, revision TEXT NOT NULL, payload TEXT NOT NULL)",
        "CREATE TABLE IF NOT EXISTS owners (id TEXT PRIMARY KEY, owner TEXT NOT NULL, fence TEXT NOT NULL)",
        "CREATE TABLE IF NOT EXISTS records (state_id TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY (state_id, key)) WITHOUT ROWID",
        "CREATE TABLE IF NOT EXISTS blobs (id INTEGER PRIMARY KEY, state_id TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL)",
    ],
    &[
        "CREATE TABLE IF NOT EXISTS states (id TEXT PRIMARY KEY, revision TEXT NOT NULL, payload TEXT NOT NULL, ord BIGINT GENERATED BY DEFAULT AS IDENTITY)",
        "CREATE TABLE IF NOT EXISTS owners (id TEXT PRIMARY KEY, owner TEXT NOT NULL, fence TEXT NOT NULL)",
        "CREATE TABLE IF NOT EXISTS blobs (id BIGINT PRIMARY KEY, state_id TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL)",
    ],
];
const PEEK: Dialects = [
    "SELECT revision, payload FROM states WHERE id = ?1",
    "SELECT revision, payload FROM states WHERE id = $1::text",
];
const OWNER: Dialects = [
    "SELECT owner, fence FROM owners WHERE id = ?1",
    "SELECT owner, fence FROM owners WHERE id = $1::text",
];
const HEAD: Dialects = [
    "SELECT owners.owner, owners.fence, COALESCE(states.revision, '0') FROM owners LEFT JOIN states ON states.id = owners.id WHERE owners.id = ?1",
    "SELECT owners.owner, owners.fence, COALESCE(states.revision, '0') FROM owners LEFT JOIN states ON states.id = owners.id WHERE owners.id = $1::text",
];
const RECORDS: Dialects = [
    "SELECT COALESCE(blobs.value, records.value) FROM json_each(?2) AS keys LEFT JOIN blobs ON blobs.id = CAST(json_extract(keys.value, '$[0]') AS INTEGER) AND blobs.state_id = ?1 AND blobs.key = json_extract(keys.value, '$[1]') LEFT JOIN records ON blobs.id IS NULL AND records.state_id = ?1 AND records.key = json_extract(keys.value, '$[1]') ORDER BY keys.key",
    "SELECT blobs.value FROM jsonb_array_elements($2::text::jsonb) WITH ORDINALITY AS keys (value, ord) LEFT JOIN blobs ON blobs.id = CAST(keys.value ->> 0 AS BIGINT) AND blobs.state_id = $1::text AND blobs.key = keys.value ->> 1 ORDER BY keys.ord",
];
const STATES: Dialects = [
    "SELECT id FROM states ORDER BY rowid DESC LIMIT CAST(?1 AS INTEGER)",
    "SELECT id FROM states ORDER BY ord DESC LIMIT CAST($1::text AS BIGINT)",
];
const RECORD_KEYS: Dialects = [
    "SELECT key FROM blobs WHERE state_id = ?1 UNION SELECT key FROM records WHERE state_id = ?1",
    "SELECT key FROM blobs WHERE state_id = $1::text",
];
const ACQUIRE: Dialects = [
    "INSERT INTO owners VALUES (?1, ?2, '1') ON CONFLICT (id) DO UPDATE SET owner = excluded.owner, fence = CAST(CAST(fence AS INTEGER) + 1 AS TEXT) WHERE CAST(fence AS INTEGER) < 9223372036854775807 RETURNING fence",
    "INSERT INTO owners (id, owner, fence) VALUES ($1::text, $2::text, '1') ON CONFLICT (id) DO UPDATE SET owner = excluded.owner, fence = CAST(CAST(owners.fence AS BIGINT) + 1 AS TEXT) WHERE CAST(owners.fence AS BIGINT) < 9223372036854775807 RETURNING fence",
];
const INSERT_RECORD: Dialects = [
    "INSERT INTO blobs SELECT CAST(?7 AS INTEGER), ?1, ?2, ?3 WHERE EXISTS (SELECT 1 FROM owners WHERE id = ?1 AND owner = ?4 AND fence = ?5) AND COALESCE((SELECT revision FROM states WHERE id = ?1), '0') = ?6 ON CONFLICT DO NOTHING",
    "INSERT INTO blobs (id, state_id, key, value) SELECT CAST($7::text AS BIGINT), $1::text, $2::text, $3::text WHERE EXISTS (SELECT 1 FROM owners WHERE id = $1::text AND owner = $4::text AND fence = $5::text FOR SHARE) AND COALESCE((SELECT revision FROM states WHERE id = $1::text), '0') = $6::text ON CONFLICT DO NOTHING",
];
const INSERT_RECORDS: &str = "INSERT INTO blobs (id, state_id, key, value) SELECT CAST(staged.value ->> 0 AS BIGINT), $1::text, staged.value ->> 1, staged.value ->> 2 FROM json_array_elements($5::text::json) AS staged (value) WHERE EXISTS (SELECT 1 FROM owners WHERE id = $1::text AND owner = $2::text AND fence = $3::text FOR SHARE) AND COALESCE((SELECT revision FROM states WHERE id = $1::text), '0') = $4::text ON CONFLICT DO NOTHING";
const PUT_RECORD: &str =
    "INSERT INTO blobs VALUES (CAST(?1 AS INTEGER), ?2, ?3, ?4) ON CONFLICT DO NOTHING";
const PUT_RECORDS: &str = "INSERT INTO blobs (id, state_id, key, value) SELECT CAST(staged.value ->> 0 AS BIGINT), $1::text, staged.value ->> 1, staged.value ->> 2 FROM json_array_elements($2::text::json) AS staged (value) ON CONFLICT DO NOTHING";
const MISPLACED: Dialects = [
    "SELECT CAST(COUNT(*) AS TEXT) FROM json_each(?2) AS staged LEFT JOIN blobs ON blobs.id = CAST(json_extract(staged.value, '$[0]') AS INTEGER) WHERE blobs.state_id IS NOT ?1 OR blobs.key IS NOT json_extract(staged.value, '$[1]')",
    "SELECT CAST(COUNT(*) AS TEXT) FROM jsonb_array_elements($2::text::jsonb) AS staged (value) LEFT JOIN blobs ON blobs.id = CAST(staged.value ->> 0 AS BIGINT) WHERE blobs.state_id IS DISTINCT FROM $1::text OR blobs.key IS DISTINCT FROM staged.value ->> 1",
];
const REPLACE: Dialects = [
    "INSERT INTO states SELECT ?1, ?2, ?3 WHERE EXISTS (SELECT 1 FROM owners WHERE id = ?1 AND owner = ?4 AND fence = ?5) AND COALESCE((SELECT revision FROM states WHERE id = ?1), '0') = ?6 AND NOT EXISTS (SELECT 1 FROM json_each(?7) AS staged LEFT JOIN blobs ON blobs.id = CAST(json_extract(staged.value, '$[0]') AS INTEGER) WHERE blobs.state_id IS NOT ?1 OR blobs.key IS NOT json_extract(staged.value, '$[1]')) ON CONFLICT (id) DO UPDATE SET revision = excluded.revision, payload = excluded.payload RETURNING revision",
    "INSERT INTO states (id, revision, payload) SELECT $1::text, $2::text, $3::text WHERE EXISTS (SELECT 1 FROM owners WHERE id = $1::text AND owner = $4::text AND fence = $5::text FOR SHARE) AND COALESCE((SELECT revision FROM states WHERE id = $1::text), '0') = $6::text AND NOT EXISTS (SELECT 1 FROM jsonb_array_elements($7::text::jsonb) AS staged (value) LEFT JOIN blobs ON blobs.id = CAST(staged.value ->> 0 AS BIGINT) WHERE blobs.state_id IS DISTINCT FROM $1::text OR blobs.key IS DISTINCT FROM staged.value ->> 1) ON CONFLICT (id) DO UPDATE SET revision = excluded.revision, payload = excluded.payload WHERE states.revision = $6::text RETURNING revision",
];

fn row_id(state_id: &str, key: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(state_id.as_bytes());
    hasher.update(&[0]);
    hasher.update(key.as_bytes());
    let mut id = [0; 8];
    hasher.finalize_xof().fill(&mut id);
    i64::from_be_bytes(id).to_string()
}

fn located(state_id: &str, keys: &[String]) -> Result<String, StoreError> {
    serde_json::to_string(
        &keys
            .iter()
            .map(|key| [row_id(state_id, key), key.clone()])
            .collect::<Vec<_>>(),
    )
    .map_err(|error| StoreError::Backend(format!("record keys: {error}")))
}

const fn statement<'a>(
    dialect: SqlDialect,
    sql: &Dialects,
    params: Vec<&'a str>,
) -> SqlStatement<'a> {
    SqlStatement {
        sql: dialect.pick(sql),
        params,
    }
}

fn invalid(detail: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(format!("SQL host answered an invalid row: {detail}"))
}

fn text(row: &SqlRow, column: usize) -> Result<&str, StoreError> {
    row.get(column)
        .and_then(Option::as_deref)
        .ok_or_else(|| invalid(format!("column {column} is missing")))
}

fn counter(row: &SqlRow, column: usize) -> Result<u64, StoreError> {
    let value = text(row, column)?;
    value
        .parse()
        .map_err(|error| invalid(format!("counter `{value}`: {error}")))
}

fn single(rows: &[SqlRow]) -> Result<Option<&SqlRow>, StoreError> {
    match rows {
        [] => Ok(None),
        [row] => Ok(Some(row)),
        _ => Err(invalid(format!(
            "{} rows for a single-row query",
            rows.len()
        ))),
    }
}

fn stored(rows: &[SqlRow]) -> Result<StoredState, StoreError> {
    match single(rows)? {
        None => Ok(StoredState::default()),
        Some(row) => Ok(StoredState {
            revision: counter(row, 0)?,
            payload: Some(text(row, 1)?.to_owned()),
        }),
    }
}

struct Replacement<'a> {
    dialect: SqlDialect,
    state_id: &'a str,
    owner: &'a OwnerToken,
    expected_revision: u64,
    revision: u64,
    payload: &'a str,
    records: &'a [StoreRecord],
}

struct Counters {
    fence: String,
    expected: String,
    revision: String,
    ids: Vec<String>,
    staged: String,
    values: Option<String>,
}

impl Replacement<'_> {
    fn counters(&self) -> Result<Counters, StoreError> {
        if self.revision <= self.expected_revision {
            return Err(StoreError::NotCommitted(format!(
                "a replacement of revision {} must advance it, not set {}",
                self.expected_revision, self.revision
            )));
        }
        let keys = self
            .records
            .iter()
            .map(|record| record.key.clone())
            .collect::<Vec<_>>();
        let ids = keys
            .iter()
            .map(|key| row_id(self.state_id, key))
            .collect::<Vec<_>>();
        let values = (self.dialect == SqlDialect::Postgres && !self.records.is_empty())
            .then(|| {
                serde_json::to_string(
                    &self
                        .records
                        .iter()
                        .zip(&ids)
                        .map(|(record, id)| [id.as_str(), &record.key, &record.value])
                        .collect::<Vec<_>>(),
                )
            })
            .transpose()
            .map_err(|error| StoreError::NotCommitted(format!("records: {error}")))?;
        Ok(Counters {
            fence: self.owner.fence().to_string(),
            expected: self.expected_revision.to_string(),
            revision: self.revision.to_string(),
            staged: located(self.state_id, &keys)
                .map_err(|error| StoreError::NotCommitted(error.to_string()))?,
            ids,
            values,
        })
    }

    fn statements<'a>(&'a self, counters: &'a Counters, statements: &mut Vec<SqlStatement<'a>>) {
        let owner = self.owner.owner_id().as_str();
        if let Some(values) = &counters.values {
            statements.push(SqlStatement {
                sql: INSERT_RECORDS,
                params: vec![
                    self.state_id,
                    owner,
                    &counters.fence,
                    &counters.expected,
                    values,
                ],
            });
        } else {
            statements.extend(self.records.iter().zip(&counters.ids).map(|(record, id)| {
                statement(
                    self.dialect,
                    &INSERT_RECORD,
                    vec![
                        self.state_id,
                        &record.key,
                        &record.value,
                        owner,
                        &counters.fence,
                        &counters.expected,
                        id,
                    ],
                )
            }));
        }
        statements.push(statement(
            self.dialect,
            &REPLACE,
            vec![
                self.state_id,
                &counters.revision,
                self.payload,
                owner,
                &counters.fence,
                &counters.expected,
                &counters.staged,
            ],
        ));
    }

    fn replaced(results: &[Vec<SqlRow>]) -> Result<Option<u64>, StoreError> {
        let [.., replaced] = results else {
            return Err(StoreError::Backend(
                "SQL host answered too few statements for a replacement".to_owned(),
            ));
        };
        single(replaced)?.map(|row| counter(row, 0)).transpose()
    }

    fn refusal(&self, head: &[SqlRow]) -> StoreError {
        self.diagnose(head).unwrap_or_else(|error| error)
    }

    fn diagnose(&self, head: &[SqlRow]) -> Result<StoreError, StoreError> {
        let Some(head) = single(head)? else {
            return Ok(StoreError::Fenced);
        };
        if text(head, 0)? != self.owner.owner_id().as_str()
            || counter(head, 1)? != self.owner.fence()
        {
            return Ok(StoreError::Fenced);
        }
        let actual = counter(head, 2)?;
        if actual != self.expected_revision {
            return Ok(StoreError::Conflict {
                expected: self.expected_revision,
                actual,
            });
        }
        Ok(StoreError::Backend(format!(
            "SQL host refused to replace revision {actual} of state {} while reporting it as current",
            self.state_id
        )))
    }
}

const MIGRATIONS: [Migration; 1] = [Migration {
    statements: SCHEMA,
    adopt: None,
}];

pub struct SqlStore<H: ?Sized> {
    host: Arc<H>,
    ready: bool,
}

impl<H: Sql + ?Sized> SqlStore<H> {
    pub fn new(host: H) -> Self
    where
        H: Sized,
    {
        Self::shared(Arc::new(host))
    }

    pub const fn shared(host: Arc<H>) -> Self {
        Self { host, ready: false }
    }

    pub const FORMAT: u32 = MIGRATIONS.len() as u32;

    #[must_use]
    pub const fn migrated(mut self) -> Self {
        self.ready = true;
        self
    }

    pub async fn migrate(&mut self) -> Result<(), StoreError> {
        migrate_format(&*self.host, "records", &MIGRATIONS).await?;
        self.ready = true;
        Ok(())
    }

    pub fn host(&self) -> &H {
        &self.host
    }

    async fn transaction(
        &mut self,
        operation: &'static str,
        statements: Vec<SqlStatement<'_>>,
    ) -> Result<Vec<Vec<SqlRow>>, StoreError> {
        if !self.ready {
            self.migrate().await?;
        }
        run_checked(&*self.host, operation, &statements).await
    }

    async fn query(
        &mut self,
        operation: &'static str,
        sql: &Dialects,
        params: Vec<&str>,
    ) -> Result<Vec<SqlRow>, StoreError> {
        let dialect = self.host.dialect();
        let mut results = self
            .transaction(operation, vec![statement(dialect, sql, params)])
            .await?;
        Ok(results.pop().unwrap_or_default())
    }

    async fn record(&mut self, state_id: &str, key: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .records(state_id, std::slice::from_ref(&key.to_owned()))
            .await?
            .pop()
            .flatten())
    }

    async fn records(
        &mut self,
        state_id: &str,
        keys: &[String],
    ) -> Result<Vec<Option<String>>, StoreError> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let encoded = located(state_id, keys)?;
        let rows = self
            .query("read_records", &RECORDS, vec![state_id, &encoded])
            .await?;
        if rows.len() != keys.len() {
            return Err(StoreError::Backend(format!(
                "SQL host answered {} of {} record reads",
                rows.len(),
                keys.len()
            )));
        }
        rows.into_iter()
            .map(|row| match <[Option<String>; 1]>::try_from(row) {
                Ok([value]) => Ok(value),
                Err(row) => Err(invalid(format!("{} columns for a record read", row.len()))),
            })
            .collect()
    }

    async fn state(&mut self, state_id: &str) -> Result<StoredState, StoreError> {
        stored(&self.query("peek", &PEEK, vec![state_id]).await?)
    }

    async fn state_ids(&mut self, limit: usize) -> Result<Vec<String>, StoreError> {
        let limit = limit.to_string();
        let rows = self.query("list_states", &STATES, vec![&limit]).await?;
        rows.iter()
            .map(|row| text(row, 0).map(str::to_owned))
            .collect()
    }

    async fn record_keys(&mut self, state_id: &str) -> Result<Vec<String>, StoreError> {
        let rows = self
            .query("list_records", &RECORD_KEYS, vec![state_id])
            .await?;
        rows.iter()
            .map(|row| text(row, 0).map(str::to_owned))
            .collect()
    }

    async fn current_owner(&mut self, state_id: &str) -> Result<Option<OwnerToken>, StoreError> {
        let rows = self.query("owner", &OWNER, vec![state_id]).await?;
        single(&rows)?
            .map(|row| {
                Ok(OwnerToken::new(
                    OwnerId::from(text(row, 0)?.to_owned()),
                    counter(row, 1)?,
                ))
            })
            .transpose()
    }

    async fn acquire_owner(
        &mut self,
        state_id: &str,
        owner_id: OwnerId,
    ) -> Result<OwnedState, StoreError> {
        let dialect = self.host.dialect();
        let results = self
            .transaction(
                "acquire",
                vec![
                    statement(dialect, &ACQUIRE, vec![state_id, owner_id.as_str()]),
                    statement(dialect, &PEEK, vec![state_id]),
                ],
            )
            .await?;
        let Some(acquired) = single(&results[0])? else {
            return Err(StoreError::NotCommitted(
                "SQL durability owner fence overflow".to_owned(),
            ));
        };
        let fence = counter(acquired, 0)?;
        Ok(OwnedState {
            owner: OwnerToken::new(owner_id, fence),
            state: stored(&results[1])?,
        })
    }

    async fn replace_state(&mut self, replacement: Replacement<'_>) -> Result<u64, StoreError> {
        let counters = replacement.counters()?;
        let mut statements = Vec::with_capacity(counters.ids.len() + 1);
        replacement.statements(&counters, &mut statements);
        let results = self.transaction("replace", statements).await?;
        match Replacement::replaced(&results)? {
            Some(revision) => Ok(revision),
            None => Err(self.refusal(&replacement).await),
        }
    }

    async fn put(&mut self, state_id: &str, records: &[StoreRecord]) -> Result<bool, StoreError> {
        if records.is_empty() {
            return Ok(true);
        }
        let dialect = self.host.dialect();
        let keys = records
            .iter()
            .map(|record| record.key.clone())
            .collect::<Vec<_>>();
        let ids = keys
            .iter()
            .map(|key| row_id(state_id, key))
            .collect::<Vec<_>>();
        let staged = located(state_id, &keys)?;
        let values = (dialect == SqlDialect::Postgres)
            .then(|| {
                serde_json::to_string(
                    &records
                        .iter()
                        .zip(&ids)
                        .map(|(record, id)| [id.as_str(), &record.key, &record.value])
                        .collect::<Vec<_>>(),
                )
            })
            .transpose()
            .map_err(|error| StoreError::NotCommitted(format!("records: {error}")))?;
        let mut statements = match &values {
            Some(values) => vec![SqlStatement {
                sql: PUT_RECORDS,
                params: vec![state_id, values],
            }],
            None => records
                .iter()
                .zip(&ids)
                .map(|(record, id)| SqlStatement {
                    sql: PUT_RECORD,
                    params: vec![id, state_id, &record.key, &record.value],
                })
                .collect(),
        };
        statements.push(statement(dialect, &MISPLACED, vec![state_id, &staged]));
        let results = self.transaction("put_records", statements).await?;
        let misplaced = results
            .last()
            .and_then(|rows| single(rows).transpose())
            .transpose()?
            .map(|row| counter(row, 0))
            .transpose()?
            .unwrap_or(0);
        if misplaced > 0 {
            return Err(StoreError::NotCommitted(format!(
                "{misplaced} of {} records of state {state_id} collide with another record's row id",
                records.len()
            )));
        }
        Ok(true)
    }

    async fn refusal(&mut self, replacement: &Replacement<'_>) -> StoreError {
        match self.query("head", &HEAD, vec![replacement.state_id]).await {
            Ok(head) => replacement.refusal(&head),
            Err(error) => error,
        }
    }

    async fn commit_all(
        &mut self,
        commits: Vec<OwnedCommit>,
    ) -> Vec<Result<Committed, StoreError>> {
        let dialect = self.host.dialect();
        let prepared: Vec<_> = commits
            .iter()
            .map(|commit| {
                if !commit.events.is_empty() {
                    return Err(StoreError::NotCommitted(
                        "this store does not record state events".to_owned(),
                    ));
                }
                let replacement = Replacement {
                    dialect,
                    state_id: &commit.state_id,
                    owner: &commit.owner,
                    expected_revision: commit.expected_revision,
                    revision: commit.expected_revision.checked_add(1).ok_or_else(|| {
                        StoreError::NotCommitted("SQL durability revision overflow".to_owned())
                    })?,
                    payload: &commit.payload,
                    records: &commit.records,
                };
                let counters = replacement.counters()?;
                Ok((replacement, counters))
            })
            .collect();
        let mut statements = Vec::new();
        let mut ranges = Vec::with_capacity(prepared.len());
        for entry in &prepared {
            let start = statements.len();
            if let Ok((replacement, counters)) = entry {
                replacement.statements(counters, &mut statements);
            }
            ranges.push(start..statements.len());
        }
        let results = if statements.is_empty() {
            Ok(Vec::new())
        } else {
            self.transaction("commit_many", statements).await
        };
        let mut outcomes = Vec::with_capacity(prepared.len());
        for (entry, range) in prepared.iter().zip(ranges) {
            let outcome = match (entry, &results) {
                (Err(error), _) | (Ok(_), Err(error)) => Err(error.clone()),
                (Ok((replacement, _)), Ok(results)) => match Replacement::replaced(&results[range])
                {
                    Ok(Some(revision)) => Ok(Committed {
                        revision,
                        first_event: None,
                    }),
                    Ok(None) => Err(self.refusal(replacement).await),
                    Err(error) => Err(error),
                },
            };
            outcomes.push(outcome);
        }
        outcomes
    }
}

impl<H: Sql + ?Sized> StateStore for SqlStore<H> {
    fn read_record<'a>(
        &'a mut self,
        state_id: &'a str,
        key: &'a str,
    ) -> StoreFuture<'a, Result<Option<String>, StoreError>> {
        Box::pin(self.record(state_id, key))
    }

    fn read_records<'a>(
        &'a mut self,
        state_id: &'a str,
        keys: &'a [String],
    ) -> StoreFuture<'a, Result<Vec<Option<String>>, StoreError>> {
        Box::pin(self.records(state_id, keys))
    }

    fn peek<'a>(
        &'a mut self,
        state_id: &'a str,
    ) -> StoreFuture<'a, Result<StoredState, StoreError>> {
        Box::pin(self.state(state_id))
    }

    fn list_states<'a>(
        &'a mut self,
        limit: usize,
    ) -> StoreFuture<'a, Result<Vec<String>, StoreError>> {
        Box::pin(self.state_ids(limit))
    }

    fn list_records<'a>(
        &'a mut self,
        state_id: &'a str,
    ) -> StoreFuture<'a, Result<Vec<String>, StoreError>> {
        Box::pin(self.record_keys(state_id))
    }

    fn owner<'a>(
        &'a mut self,
        state_id: &'a str,
    ) -> StoreFuture<'a, Result<Option<OwnerToken>, StoreError>> {
        Box::pin(self.current_owner(state_id))
    }

    fn acquire<'a>(
        &'a mut self,
        state_id: &'a str,
        owner_id: OwnerId,
    ) -> StoreFuture<'a, Result<OwnedState, StoreError>> {
        Box::pin(self.acquire_owner(state_id, owner_id))
    }

    fn replace<'a>(
        &'a mut self,
        state_id: &'a str,
        owner: &'a OwnerToken,
        expected_revision: u64,
        payload: &'a str,
        records: &'a [StoreRecord],
    ) -> StoreFuture<'a, Result<u64, StoreError>> {
        let dialect = self.host.dialect();
        Box::pin(async move {
            self.replace_state(Replacement {
                dialect,
                state_id,
                owner,
                expected_revision,
                revision: expected_revision.checked_add(1).ok_or_else(|| {
                    StoreError::NotCommitted("SQL durability revision overflow".to_owned())
                })?,
                payload,
                records,
            })
            .await
        })
    }

    fn put_records<'a>(
        &'a mut self,
        state_id: &'a str,
        records: &'a [StoreRecord],
    ) -> StoreFuture<'a, Result<bool, StoreError>> {
        Box::pin(self.put(state_id, records))
    }

    fn replace_to<'a>(
        &'a mut self,
        state_id: &'a str,
        owner: &'a OwnerToken,
        expected_revision: u64,
        revision: u64,
        payload: &'a str,
        records: &'a [StoreRecord],
    ) -> StoreFuture<'a, Result<Option<u64>, StoreError>> {
        let dialect = self.host.dialect();
        Box::pin(async move {
            self.replace_state(Replacement {
                dialect,
                state_id,
                owner,
                expected_revision,
                revision,
                payload,
                records,
            })
            .await
            .map(Some)
        })
    }

    fn commit_many(
        &mut self,
        commits: Vec<OwnedCommit>,
    ) -> StoreFuture<'_, Vec<Result<Committed, StoreError>>> {
        Box::pin(self.commit_all(commits))
    }
}
