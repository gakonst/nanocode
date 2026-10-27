use std::{collections::HashSet, sync::Arc};

use serde::ser::SerializeSeq;

use crate::{
    Committed, EncodedPayload, OwnedCommit, OwnedState, OwnerId, OwnerToken, StateStore,
    StoreError, StoreFuture, StoreRecord, StoredState,
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
const SHARED: [&[&str]; 2] = [
    &[
        "ALTER TABLE records RENAME TO legacy_records",
        "CREATE TABLE records (id INTEGER PRIMARY KEY, key TEXT NOT NULL UNIQUE, value TEXT NOT NULL)",
        "CREATE TABLE state_records (record INTEGER NOT NULL, state_id TEXT NOT NULL, PRIMARY KEY (record, state_id)) WITHOUT ROWID",
        "CREATE INDEX state_records_state ON state_records (state_id)",
        "INSERT INTO records (key, value) SELECT key, value FROM blobs WHERE true ON CONFLICT (key) DO NOTHING",
        "INSERT INTO records (key, value) SELECT key, value FROM legacy_records WHERE true ON CONFLICT (key) DO NOTHING",
        "INSERT INTO state_records (state_id, record) SELECT blobs.state_id, records.id FROM blobs JOIN records ON records.key = blobs.key WHERE true ON CONFLICT DO NOTHING",
        "INSERT INTO state_records (state_id, record) SELECT legacy_records.state_id, records.id FROM legacy_records JOIN records ON records.key = legacy_records.key WHERE true ON CONFLICT DO NOTHING",
        "DROP TABLE legacy_records",
        "DROP TABLE blobs",
    ],
    &[
        "CREATE TABLE IF NOT EXISTS records (id BIGINT GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY, key TEXT NOT NULL UNIQUE, value TEXT NOT NULL)",
        "CREATE TABLE IF NOT EXISTS state_records (record BIGINT NOT NULL, state_id TEXT NOT NULL, PRIMARY KEY (record, state_id))",
        "CREATE INDEX IF NOT EXISTS state_records_state ON state_records (state_id)",
        "INSERT INTO records (key, value) SELECT DISTINCT ON (key) key, value FROM blobs ORDER BY key ON CONFLICT (key) DO NOTHING",
        "INSERT INTO state_records (state_id, record) SELECT DISTINCT blobs.state_id, records.id FROM blobs JOIN records ON records.key = blobs.key ON CONFLICT DO NOTHING",
        "DROP TABLE blobs",
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
    "SELECT records.value FROM json_each(?1) AS keys LEFT JOIN records ON records.key = keys.value ORDER BY keys.key",
    "SELECT records.value FROM jsonb_array_elements_text($1::text::jsonb) WITH ORDINALITY AS keys (key, ord) LEFT JOIN records ON records.key = keys.key ORDER BY keys.ord",
];
const STATES: Dialects = [
    "SELECT id FROM states ORDER BY rowid DESC LIMIT CAST(?1 AS INTEGER)",
    "SELECT id FROM states ORDER BY ord DESC LIMIT CAST($1::text AS BIGINT)",
];
const RECORD_KEYS: Dialects = [
    "SELECT records.key FROM state_records JOIN records ON records.id = state_records.record WHERE state_records.state_id = ?1",
    "SELECT records.key FROM state_records JOIN records ON records.id = state_records.record WHERE state_records.state_id = $1::text",
];
const ACQUIRE: Dialects = [
    "INSERT INTO owners VALUES (?1, ?2, '1') ON CONFLICT (id) DO UPDATE SET owner = excluded.owner, fence = CAST(CAST(fence AS INTEGER) + 1 AS TEXT) WHERE CAST(fence AS INTEGER) < 9223372036854775807 RETURNING fence",
    "INSERT INTO owners (id, owner, fence) VALUES ($1::text, $2::text, '1') ON CONFLICT (id) DO UPDATE SET owner = excluded.owner, fence = CAST(CAST(owners.fence AS BIGINT) + 1 AS TEXT) WHERE CAST(owners.fence AS BIGINT) < 9223372036854775807 RETURNING fence",
];
const INSERT_RECORD: &str = "INSERT INTO records (key, value) SELECT ?2, ?3 WHERE EXISTS (SELECT 1 FROM owners WHERE id = ?1 AND owner = ?4 AND fence = ?5) AND COALESCE((SELECT revision FROM states WHERE id = ?1), '0') = ?6 ON CONFLICT (key) DO NOTHING";
const INSERT_RECORDS: &str = "WITH stored AS (INSERT INTO records (key, value) SELECT staged.value ->> 0, staged.value ->> 1 FROM json_array_elements($5::text::json) AS staged (value) WHERE EXISTS (SELECT 1 FROM owners WHERE id = $1::text AND owner = $2::text AND fence = $3::text FOR SHARE) AND COALESCE((SELECT revision FROM states WHERE id = $1::text), '0') = $4::text ORDER BY 1 ON CONFLICT (key) DO UPDATE SET key = excluded.key RETURNING id) INSERT INTO state_records (record, state_id) SELECT id, $1::text FROM stored ON CONFLICT DO NOTHING";
const HOLD: &str = "INSERT INTO state_records (record, state_id) SELECT records.id, ?1 FROM json_each(?5) AS staged JOIN records ON records.key = staged.value WHERE EXISTS (SELECT 1 FROM owners WHERE id = ?1 AND owner = ?2 AND fence = ?3) AND COALESCE((SELECT revision FROM states WHERE id = ?1), '0') = ?4 ON CONFLICT DO NOTHING";
const PUT_RECORD: &str =
    "INSERT INTO records (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO NOTHING";
const PUT_RECORDS: &str = "WITH stored AS (INSERT INTO records (key, value) SELECT staged.value ->> 0, staged.value ->> 1 FROM json_array_elements($2::text::json) AS staged (value) ORDER BY 1 ON CONFLICT (key) DO UPDATE SET key = excluded.key RETURNING id) INSERT INTO state_records (record, state_id) SELECT id, $1::text FROM stored ON CONFLICT DO NOTHING";
const PUT_HOLDS: &str = "INSERT INTO state_records (record, state_id) SELECT records.id, ?1 FROM json_each(?2) AS staged JOIN records ON records.key = staged.value WHERE true ON CONFLICT DO NOTHING";
const REPLACE: Dialects = [
    "INSERT INTO states SELECT ?1, ?2, ?3 WHERE EXISTS (SELECT 1 FROM owners WHERE id = ?1 AND owner = ?4 AND fence = ?5) AND COALESCE((SELECT revision FROM states WHERE id = ?1), '0') = ?6 AND NOT EXISTS (SELECT 1 FROM json_each(?7) AS staged LEFT JOIN records ON records.key = staged.value LEFT JOIN state_records ON state_records.state_id = ?1 AND state_records.record = records.id WHERE state_records.record IS NULL) ON CONFLICT (id) DO UPDATE SET revision = excluded.revision, payload = excluded.payload RETURNING revision",
    "INSERT INTO states (id, revision, payload) SELECT $1::text, $2::text, $3::text WHERE EXISTS (SELECT 1 FROM owners WHERE id = $1::text AND owner = $4::text AND fence = $5::text FOR SHARE) AND COALESCE((SELECT revision FROM states WHERE id = $1::text), '0') = $6::text AND NOT EXISTS (SELECT 1 FROM jsonb_array_elements_text($7::text::jsonb) AS staged (key) LEFT JOIN records ON records.key = staged.key LEFT JOIN state_records ON state_records.state_id = $1::text AND state_records.record = records.id WHERE state_records.record IS NULL) ON CONFLICT (id) DO UPDATE SET revision = excluded.revision, payload = excluded.payload WHERE states.revision = $6::text RETURNING revision",
];
const HOLD_OWNER: Dialects = [
    "INSERT INTO owners (id, owner, fence) VALUES (?1, '', '0') ON CONFLICT (id) DO UPDATE SET fence = owners.fence",
    "INSERT INTO owners (id, owner, fence) VALUES ($1::text, '', '0') ON CONFLICT (id) DO UPDATE SET fence = owners.fence",
];
const LOCK_RECORDS: &str = "SELECT CAST(COUNT(*) AS TEXT) FROM (SELECT 1 FROM jsonb_array_elements_text($1::text::jsonb) AS dead (key) JOIN records ON records.key = dead.key ORDER BY records.key FOR UPDATE OF records) AS locked";
const RELEASE: Dialects = [
    "DELETE FROM state_records WHERE state_id = ?1 AND record IN (SELECT records.id FROM json_each(?3) AS dead JOIN records ON records.key = dead.value) AND COALESCE((SELECT revision FROM states WHERE id = ?1), '0') = ?2 AND (SELECT CAST(fence AS INTEGER) FROM owners WHERE id = ?1) < 9223372036854775807 RETURNING record",
    "DELETE FROM state_records USING jsonb_array_elements_text($3::text::jsonb) AS dead (key), records WHERE records.key = dead.key AND state_records.state_id = $1::text AND state_records.record = records.id AND COALESCE((SELECT revision FROM states WHERE id = $1::text), '0') = $2::text AND (SELECT CAST(fence AS BIGINT) FROM owners WHERE id = $1::text) < 9223372036854775807 RETURNING CAST(state_records.record AS TEXT)",
];
const FENCE: Dialects = [
    "UPDATE owners SET fence = CAST(CAST(fence AS INTEGER) + 1 AS TEXT) WHERE id = ?1 AND COALESCE((SELECT revision FROM states WHERE id = ?1), '0') = ?2 AND CAST(fence AS INTEGER) < 9223372036854775807 RETURNING fence",
    "UPDATE owners SET fence = CAST(CAST(fence AS BIGINT) + 1 AS TEXT) WHERE id = $1::text AND COALESCE((SELECT revision FROM states WHERE id = $1::text), '0') = $2::text AND CAST(fence AS BIGINT) < 9223372036854775807 RETURNING fence",
];
const RELEASE_OWNER: Dialects = [
    "DELETE FROM owners WHERE id = ?1 AND owner = '' AND fence = '0'",
    "DELETE FROM owners WHERE id = $1::text AND owner = '' AND fence = '0'",
];
const DROP_RECORDS: Dialects = [
    "DELETE FROM records WHERE key IN (SELECT value FROM json_each(?1)) AND NOT EXISTS (SELECT 1 FROM state_records WHERE state_records.record = records.id) RETURNING id",
    "DELETE FROM records USING jsonb_array_elements_text($1::text::jsonb) AS dead (key) WHERE records.key = dead.key AND NOT EXISTS (SELECT 1 FROM state_records WHERE state_records.record = records.id) RETURNING CAST(records.id AS TEXT)",
];

fn keys_json(keys: impl Iterator<Item = impl AsRef<str>>) -> Result<String, StoreError> {
    serde_json::to_string(&keys.map(|key| key.as_ref().to_owned()).collect::<Vec<_>>())
        .map_err(|error| StoreError::NotCommitted(format!("record keys: {error}")))
}

fn values_json(records: &[StoreRecord]) -> Result<String, StoreError> {
    serde_json::to_string(
        &records
            .iter()
            .map(|record| (record.key.as_str(), record.value.as_str()))
            .collect::<std::collections::BTreeMap<_, _>>()
            .into_iter()
            .map(|(key, value)| [key, value])
            .collect::<Vec<_>>(),
    )
    .map_err(|error| StoreError::NotCommitted(format!("records: {error}")))
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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Unreachable {
    pub revision: u64,
    pub keys: Vec<String>,
    pub reachable: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Collected {
    pub released: u64,
    pub deleted: u64,
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
        let values = (self.dialect == SqlDialect::Postgres && !self.records.is_empty())
            .then(|| values_json(self.records))
            .transpose()?;
        Ok(Counters {
            fence: self.owner.fence().to_string(),
            expected: self.expected_revision.to_string(),
            revision: self.revision.to_string(),
            staged: keys_json(self.records.iter().map(|record| &record.key))?,
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
            statements.extend(self.records.iter().map(|record| SqlStatement {
                sql: INSERT_RECORD,
                params: vec![
                    self.state_id,
                    &record.key,
                    &record.value,
                    owner,
                    &counters.fence,
                    &counters.expected,
                ],
            }));
            if !self.records.is_empty() {
                statements.push(SqlStatement {
                    sql: HOLD,
                    params: vec![
                        self.state_id,
                        owner,
                        &counters.fence,
                        &counters.expected,
                        &counters.staged,
                    ],
                });
            }
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

const MIGRATIONS: [Migration; 2] = [
    Migration {
        statements: SCHEMA,
        adopt: None,
    },
    Migration {
        statements: SHARED,
        adopt: None,
    },
];

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

    async fn record(&mut self, key: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .records(std::slice::from_ref(&key.to_owned()))
            .await?
            .pop()
            .flatten())
    }

    async fn records(&mut self, keys: &[String]) -> Result<Vec<Option<String>>, StoreError> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let encoded = keys_json(keys.iter())?;
        let rows = self.query("read_records", &RECORDS, vec![&encoded]).await?;
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
        let limit = u64::try_from(limit)
            .unwrap_or(u64::MAX)
            .min(i64::MAX as u64)
            .to_string();
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
        let mut statements = Vec::with_capacity(replacement.records.len() + 2);
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
        let keys = keys_json(records.iter().map(|record| &record.key))?;
        let values = (dialect == SqlDialect::Postgres)
            .then(|| values_json(records))
            .transpose()?;
        let statements = match &values {
            Some(values) => vec![SqlStatement {
                sql: PUT_RECORDS,
                params: vec![state_id, values],
            }],
            None => records
                .iter()
                .map(|record| SqlStatement {
                    sql: PUT_RECORD,
                    params: vec![&record.key, &record.value],
                })
                .chain([SqlStatement {
                    sql: PUT_HOLDS,
                    params: vec![state_id, &keys],
                }])
                .collect(),
        };
        self.transaction("put_records", statements).await?;
        Ok(true)
    }

    pub async fn unreachable(&mut self, state_id: &str) -> Result<Unreachable, StoreError> {
        let head = self.state(state_id).await?;
        let held = self.record_keys(state_id).await?;
        let mut unreachable = Unreachable {
            revision: head.revision,
            keys: Vec::new(),
            reachable: held.len(),
        };
        if !held.iter().all(|key| EncodedPayload::is_layout_key(key)) {
            return Ok(unreachable);
        }
        let holds: HashSet<&str> = held.iter().map(String::as_str).collect();
        let mut reached = HashSet::new();
        let mut level = Vec::new();
        let follow = |json: &str, reached: &mut HashSet<String>, level: &mut Vec<_>| {
            for key in EncodedPayload::references(json) {
                if holds.contains(key) && reached.insert(key.to_owned()) {
                    level.push(EncodedPayload::by_key(key));
                }
            }
        };
        follow(
            head.payload.as_deref().unwrap_or_default(),
            &mut reached,
            &mut level,
        );
        while !level.is_empty() {
            let loaded =
                EncodedPayload::load_many_noting(&level, &mut *self, state_id, &mut reached)
                    .await
                    .map_err(|error| {
                        StoreError::Backend(format!("state {state_id} is not whole: {error}"))
                    })?;
            level.clear();
            for payload in &loaded {
                let json = payload
                    .json()
                    .map_err(|error| StoreError::Backend(error.to_string()))?;
                follow(json, &mut reached, &mut level);
            }
        }
        unreachable.keys = held
            .iter()
            .filter(|key| !reached.contains(*key))
            .cloned()
            .collect();
        unreachable.reachable = held.len() - unreachable.keys.len();
        Ok(unreachable)
    }

    pub async fn collect(
        &mut self,
        state_id: &str,
        revision: u64,
        keys: &[String],
    ) -> Result<Option<Collected>, StoreError> {
        if keys.is_empty() {
            return Ok(Some(Collected::default()));
        }
        let dialect = self.host.dialect();
        let revision = revision.to_string();
        let dead = keys_json(keys.iter())?;
        let mut statements = vec![statement(dialect, &HOLD_OWNER, vec![state_id])];
        if dialect == SqlDialect::Postgres {
            statements.push(SqlStatement {
                sql: LOCK_RECORDS,
                params: vec![&dead],
            });
        }
        statements.extend([
            statement(dialect, &RELEASE, vec![state_id, &revision, &dead]),
            statement(dialect, &FENCE, vec![state_id, &revision]),
            statement(dialect, &RELEASE_OWNER, vec![state_id]),
            statement(dialect, &DROP_RECORDS, vec![&dead]),
        ]);
        let released = statements.len() - 4;
        let results = self.transaction("collect", statements).await?;
        if results[released + 1].is_empty() {
            return Ok(None);
        }
        Ok(Some(Collected {
            released: results[released].len() as u64,
            deleted: results[released + 3].len() as u64,
        }))
    }

    pub async fn sweep(
        &mut self,
        state_id: &str,
        batch: usize,
    ) -> Result<Option<Collected>, StoreError> {
        let unreachable = self.unreachable(state_id).await?;
        let mut collected = Collected::default();
        for keys in unreachable.keys.chunks(batch.max(1)) {
            let Some(swept) = self.collect(state_id, unreachable.revision, keys).await? else {
                return Ok(None);
            };
            collected.released += swept.released;
            collected.deleted += swept.deleted;
        }
        Ok(Some(collected))
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
        _state_id: &'a str,
        key: &'a str,
    ) -> StoreFuture<'a, Result<Option<String>, StoreError>> {
        Box::pin(self.record(key))
    }

    fn read_records<'a>(
        &'a mut self,
        _state_id: &'a str,
        keys: &'a [String],
    ) -> StoreFuture<'a, Result<Vec<Option<String>>, StoreError>> {
        Box::pin(self.records(keys))
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
