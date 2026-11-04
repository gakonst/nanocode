#![cfg(not(target_family = "wasm"))]

use tokio_postgres::{
    Client,
    types::{ToSql, Type},
};

use nanocode_durability_core::{
    OwnedState, OwnerId, OwnerToken, Sql, SqlDialect, SqlRow, SqlStatement, StateStore, StoreError,
    StoreFuture, StoredState,
};

const POSTGRES_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS nanocode_durable_records (
       state_id TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL,
       PRIMARY KEY (state_id, key)
     );
     CREATE TABLE IF NOT EXISTS nanocode_durable_owners (
       state_id TEXT PRIMARY KEY,
       owner_id TEXT NOT NULL,
       fence NUMERIC(20, 0) NOT NULL
         CHECK (fence >= 1 AND fence <= 18446744073709551615)
     );
     CREATE TABLE IF NOT EXISTS nanocode_durable_states (
       state_id TEXT PRIMARY KEY,
       revision NUMERIC(20, 0) NOT NULL
         CHECK (revision >= 1 AND revision <= 18446744073709551615),
       payload TEXT NOT NULL
     );";

pub struct PostgresStore {
    client: Client,
}

impl PostgresStore {
    pub async fn new(client: Client) -> Result<Self, StoreError> {
        let mut store = Self { client };
        let transaction = store.client.transaction().await.map_err(backend)?;
        transaction
            .query(
                "SELECT pg_advisory_xact_lock(hashtextextended(
                   current_database() || ':' || current_schema() || ':nanocode-durability-v2', 0
                 ))",
                &[],
            )
            .await
            .map_err(backend)?;
        transaction
            .batch_execute(POSTGRES_SCHEMA)
            .await
            .map_err(backend)?;
        validate_schema(&transaction).await?;
        transaction.commit().await.map_err(backend)?;
        Ok(store)
    }
}

impl StateStore for PostgresStore {
    fn read_record<'a>(
        &'a mut self,
        state_id: &'a str,
        key: &'a str,
    ) -> StoreFuture<'a, Result<Option<String>, StoreError>> {
        Box::pin(async move {
            self.client
                .query_opt(
                    "SELECT value FROM nanocode_durable_records WHERE state_id = $1 AND key = $2",
                    &[&state_id, &key],
                )
                .await
                .map(|row| row.map(|row| row.get(0)))
                .map_err(backend)
        })
    }

    fn acquire<'a>(
        &'a mut self,
        state_id: &'a str,
        owner_id: OwnerId,
    ) -> StoreFuture<'a, Result<OwnedState, StoreError>> {
        Box::pin(async move {
            let transaction = self.client.transaction().await.map_err(backend)?;
            let row = transaction
                .query_opt(
                    "INSERT INTO nanocode_durable_owners (state_id, owner_id, fence)
                     VALUES ($1, $2, 1)
                     ON CONFLICT (state_id) DO UPDATE
                     SET owner_id = excluded.owner_id,
                         fence = nanocode_durable_owners.fence + 1
                     WHERE nanocode_durable_owners.fence < 18446744073709551615
                     RETURNING fence::text",
                    &[&state_id, &owner_id.as_str()],
                )
                .await
                .map_err(backend)?
                .ok_or_else(|| {
                    StoreError::NotCommitted("Postgres durability owner fence overflow".to_owned())
                })?;
            let fence = parse_u64(&row.get::<_, String>(0), "Postgres owner fence")?;
            let state = transaction
                .query_opt(
                    "SELECT revision::text, payload
                     FROM nanocode_durable_states WHERE state_id = $1",
                    &[&state_id],
                )
                .await
                .map_err(backend)?
                .map(|row| {
                    Ok(StoredState {
                        revision: parse_u64(&row.get::<_, String>(0), "Postgres state revision")?,
                        payload: Some(row.get(1)),
                    })
                })
                .transpose()?
                .unwrap_or_default();
            let owner = OwnerToken::new(owner_id, fence);
            transaction.commit().await.map_err(backend)?;
            Ok(OwnedState { owner, state })
        })
    }

    fn replace<'a>(
        &'a mut self,
        state_id: &'a str,
        owner: &'a OwnerToken,
        expected_revision: u64,
        payload: &'a str,
        records: &'a [nanocode_durability_core::StoreRecord],
    ) -> StoreFuture<'a, Result<u64, StoreError>> {
        Box::pin(async move {
            let transaction = self.client.transaction().await.map_err(backend)?;
            let retained_owner = transaction
                .query_opt(
                    "SELECT owner_id, fence::text FROM nanocode_durable_owners
                     WHERE state_id = $1 FOR UPDATE",
                    &[&state_id],
                )
                .await
                .map_err(backend)?;
            let owns_state = match retained_owner {
                Some(row) => {
                    row.get::<_, String>(0) == owner.owner_id().as_str()
                        && parse_u64(&row.get::<_, String>(1), "Postgres owner fence")?
                            == owner.fence()
                }
                None => false,
            };
            if !owns_state {
                return Err(StoreError::Fenced);
            }
            let actual = transaction
                .query_opt(
                    "SELECT revision::text FROM nanocode_durable_states
                     WHERE state_id = $1 FOR UPDATE",
                    &[&state_id],
                )
                .await
                .map_err(backend)?
                .map_or(Ok(0), |row| {
                    parse_u64(&row.get::<_, String>(0), "Postgres state revision")
                })?;
            if actual != expected_revision {
                return Err(StoreError::Conflict {
                    expected: expected_revision,
                    actual,
                });
            }
            for record in records {
                transaction.execute(
                    "INSERT INTO nanocode_durable_records (state_id, key, value) VALUES ($1, $2, $3)
                     ON CONFLICT (state_id, key) DO NOTHING", &[&state_id, &record.key, &record.value],
                ).await.map_err(backend)?;
                let retained: String = transaction.query_one(
                    "SELECT value FROM nanocode_durable_records WHERE state_id = $1 AND key = $2",
                    &[&state_id, &record.key],
                ).await.map_err(backend)?.get(0);
                if retained != record.value {
                    return Err(StoreError::NotCommitted(format!(
                        "durability record `{}` has conflicting content",
                        record.key
                    )));
                }
            }
            let revision = actual.checked_add(1).ok_or_else(|| {
                StoreError::NotCommitted("Postgres durability revision overflow".to_owned())
            })?;
            let revision_text = revision.to_string();
            transaction
                .execute(
                    "INSERT INTO nanocode_durable_states (state_id, revision, payload)
                     VALUES ($1, $2::numeric, $3)
                     ON CONFLICT (state_id) DO UPDATE
                     SET revision = excluded.revision, payload = excluded.payload",
                    &[&state_id, &revision_text, &payload],
                )
                .await
                .map_err(backend)?;
            transaction.commit().await.map_err(backend)?;
            Ok(revision)
        })
    }

    fn peek<'a>(
        &'a mut self,
        state_id: &'a str,
    ) -> StoreFuture<'a, Result<StoredState, StoreError>> {
        Box::pin(async move {
            self.client
                .query_opt(
                    "SELECT revision::text, payload
                     FROM nanocode_durable_states WHERE state_id = $1",
                    &[&state_id],
                )
                .await
                .map_err(backend)?
                .map(|row| {
                    Ok(StoredState {
                        revision: parse_u64(&row.get::<_, String>(0), "Postgres state revision")?,
                        payload: Some(row.get(1)),
                    })
                })
                .transpose()
                .map(Option::unwrap_or_default)
        })
    }

    fn owner<'a>(
        &'a mut self,
        state_id: &'a str,
    ) -> StoreFuture<'a, Result<Option<OwnerToken>, StoreError>> {
        Box::pin(async move {
            self.client
                .query_opt(
                    "SELECT owner_id, fence::text FROM nanocode_durable_owners WHERE state_id = $1",
                    &[&state_id],
                )
                .await
                .map_err(backend)?
                .map(|row| {
                    Ok(OwnerToken::new(
                        OwnerId::from(row.get::<_, String>(0)),
                        parse_u64(&row.get::<_, String>(1), "Postgres owner fence")?,
                    ))
                })
                .transpose()
        })
    }

    fn list_states<'a>(
        &'a mut self,
        limit: usize,
    ) -> StoreFuture<'a, Result<Vec<String>, StoreError>> {
        Box::pin(async move {
            let limit = i64::try_from(limit).unwrap_or(i64::MAX);
            Ok(self
                .client
                .query(
                    "SELECT state_id FROM nanocode_durable_states ORDER BY state_id DESC LIMIT $1",
                    &[&limit],
                )
                .await
                .map_err(backend)?
                .into_iter()
                .map(|row| row.get(0))
                .collect())
        })
    }

    fn list_records<'a>(
        &'a mut self,
        state_id: &'a str,
    ) -> StoreFuture<'a, Result<Vec<String>, StoreError>> {
        Box::pin(async move {
            Ok(self
                .client
                .query(
                    "SELECT key FROM nanocode_durable_records WHERE state_id = $1",
                    &[&state_id],
                )
                .await
                .map_err(backend)?
                .into_iter()
                .map(|row| row.get(0))
                .collect())
        })
    }
}

pub struct PostgresSql {
    clients: Vec<nanocode_sync::Owned<Connection>>,
    next: std::sync::atomic::AtomicUsize,
}

struct Connection {
    client: Client,
    prepared: std::collections::HashMap<&'static str, tokio_postgres::Statement>,
}

fn reusable(sql: &str) -> bool {
    ["SELECT", "INSERT", "UPDATE", "DELETE", "WITH"]
        .iter()
        .any(|verb| sql.starts_with(verb))
}

impl PostgresSql {
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self::pool(vec![client])
    }

    #[must_use]
    pub fn pool(clients: Vec<Client>) -> Self {
        assert!(!clients.is_empty(), "a Postgres pool needs a connection");
        Self {
            clients: clients
                .into_iter()
                .map(|client| {
                    nanocode_sync::Owned::new(Connection {
                        client,
                        prepared: std::collections::HashMap::new(),
                    })
                })
                .collect(),
            next: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub async fn connect(config: &str, connections: usize) -> Result<Self, StoreError> {
        if connections == 0 {
            return Err(StoreError::Backend(
                "a Postgres pool needs a connection".to_owned(),
            ));
        }
        static RUNTIME: std::sync::OnceLock<Option<tokio::runtime::Handle>> =
            std::sync::OnceLock::new();
        let runtime = RUNTIME
            .get_or_init(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .ok()?;
                let handle = runtime.handle().clone();
                std::thread::Builder::new()
                    .name("nanocode-postgres".into())
                    .spawn(move || runtime.block_on(std::future::pending::<()>()))
                    .ok()?;
                Some(handle)
            })
            .as_ref()
            .ok_or_else(|| {
                StoreError::Backend("could not start the Postgres connection thread".to_owned())
            })?;
        let config = config.to_owned();
        let clients = runtime
            .spawn(async move {
                let mut clients = Vec::with_capacity(connections);
                for _ in 0..connections {
                    let (client, connection) =
                        tokio_postgres::connect(&config, tokio_postgres::NoTls)
                            .await
                            .map_err(backend)?;
                    drop(tokio::spawn(connection));
                    clients.push(client);
                }
                Ok::<_, StoreError>(clients)
            })
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))??;
        Ok(Self::pool(clients))
    }

    async fn client(&self) -> nanocode_sync::Held<'_, Connection> {
        let start = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let count = self.clients.len();
        for offset in 0..count {
            if let Some(client) = self.clients[(start + offset) % count].try_acquire() {
                return client;
            }
        }
        self.clients[start % count].acquire().await
    }
}

impl Sql for PostgresSql {
    fn transaction<'a>(
        &'a self,
        _operation: &'static str,
        statements: &'a [SqlStatement<'a>],
    ) -> StoreFuture<'a, Result<Vec<Vec<SqlRow>>, StoreError>> {
        Box::pin(async move {
            let reads = statements
                .iter()
                .all(|statement| statement.sql.starts_with("SELECT"));
            let mut connection = self.client().await;
            let prepare = statements.iter().all(|statement| reusable(statement.sql));
            if prepare {
                if connection.prepared.is_empty() {
                    connection
                        .client
                        .batch_execute("SET plan_cache_mode = force_custom_plan")
                        .await
                        .map_err(backend)?;
                }
                let missing = statements
                    .iter()
                    .filter(|statement| !connection.prepared.contains_key(statement.sql))
                    .map(|statement| (statement.sql, statement.params.len()))
                    .collect::<std::collections::HashMap<_, _>>();
                let prepared =
                    futures_util::future::try_join_all(missing.iter().map(|(sql, count)| {
                        let types = vec![Type::TEXT; *count];
                        let client = &connection.client;
                        async move {
                            client
                                .prepare_typed(sql, &types)
                                .await
                                .map(|statement| (*sql, statement))
                        }
                    }))
                    .await
                    .map_err(backend)?;
                connection.prepared.extend(prepared);
            }
            let Connection { client, prepared } = &mut *connection;
            let prepared = prepare.then_some(&*prepared);
            if reads {
                let replies = run_statements(&*client, statements, prepared)
                    .await
                    .map_err(backend)?;
                return text_rows(&replies);
            }
            let transaction = client.transaction().await.map_err(backend)?;
            let replies = run_statements(&transaction, statements, prepared)
                .await
                .map_err(backend)?;
            let results = text_rows(&replies)?;
            transaction.commit().await.map_err(backend)?;
            Ok(results)
        })
    }

    fn dialect(&self) -> SqlDialect {
        SqlDialect::Postgres
    }

    fn concurrent(self: std::sync::Arc<Self>) -> Option<std::sync::Arc<dyn Sql>> {
        Some(self)
    }

    fn writers(&self) -> usize {
        self.clients.len()
    }

    fn readers(&self) -> usize {
        self.clients.len()
    }
}

async fn run_statements<C: tokio_postgres::GenericClient>(
    client: &C,
    statements: &[SqlStatement<'_>],
    prepared: Option<&std::collections::HashMap<&'static str, tokio_postgres::Statement>>,
) -> Result<Vec<Vec<tokio_postgres::Row>>, tokio_postgres::Error> {
    let params = statements
        .iter()
        .map(|statement| {
            statement
                .params
                .iter()
                .map(|param| (param as &(dyn ToSql + Sync), Type::TEXT))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    futures_util::future::try_join_all(statements.iter().zip(&params).map(|(statement, params)| {
        let prepared = prepared.and_then(|prepared| prepared.get(statement.sql));
        async move {
            match prepared {
                Some(prepared) => {
                    let values = params.iter().map(|(value, _)| *value).collect::<Vec<_>>();
                    client.query(prepared, &values).await
                }
                None => client.query_typed(statement.sql, params).await,
            }
        }
    }))
    .await
}

fn text_rows(replies: &[Vec<tokio_postgres::Row>]) -> Result<Vec<Vec<SqlRow>>, StoreError> {
    replies
        .iter()
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    (0..row.len())
                        .map(|column| row.try_get::<_, Option<String>>(column))
                        .collect::<Result<SqlRow, _>>()
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(backend)
        })
        .collect()
}

async fn validate_schema(transaction: &tokio_postgres::Transaction<'_>) -> Result<(), StoreError> {
    let rows = transaction
        .query(
            "SELECT table_name, column_name, data_type, is_nullable,
                    numeric_precision, numeric_scale
             FROM information_schema.columns
             WHERE table_schema = current_schema()
               AND table_name IN ('nanocode_durable_owners', 'nanocode_durable_states')
             ORDER BY table_name, ordinal_position",
            &[],
        )
        .await
        .map_err(backend)?;
    let actual = rows
        .into_iter()
        .map(|row| {
            (
                row.get::<_, String>(0),
                row.get::<_, String>(1),
                row.get::<_, String>(2),
                row.get::<_, String>(3),
                row.get::<_, Option<i32>>(4),
                row.get::<_, Option<i32>>(5),
            )
        })
        .collect::<Vec<_>>();
    let expected = vec![
        (
            "nanocode_durable_owners",
            "state_id",
            "text",
            "NO",
            None,
            None,
        ),
        (
            "nanocode_durable_owners",
            "owner_id",
            "text",
            "NO",
            None,
            None,
        ),
        (
            "nanocode_durable_owners",
            "fence",
            "numeric",
            "NO",
            Some(20),
            Some(0),
        ),
        (
            "nanocode_durable_states",
            "state_id",
            "text",
            "NO",
            None,
            None,
        ),
        (
            "nanocode_durable_states",
            "revision",
            "numeric",
            "NO",
            Some(20),
            Some(0),
        ),
        (
            "nanocode_durable_states",
            "payload",
            "text",
            "NO",
            None,
            None,
        ),
    ];
    if actual.len() != expected.len()
        || actual.iter().zip(expected).any(|(actual, expected)| {
            actual.0 != expected.0
                || actual.1 != expected.1
                || actual.2 != expected.2
                || actual.3 != expected.3
                || actual.4 != expected.4
                || actual.5 != expected.5
        })
    {
        return Err(StoreError::Backend(
            "incompatible Postgres durability schema; recreate the two nanocode_durable_* tables"
                .to_owned(),
        ));
    }
    let primary_keys = transaction
        .query(
            "SELECT tc.table_name, kcu.column_name, kcu.ordinal_position
             FROM information_schema.table_constraints AS tc
             JOIN information_schema.key_column_usage AS kcu
               ON tc.constraint_catalog = kcu.constraint_catalog
              AND tc.constraint_schema = kcu.constraint_schema
              AND tc.constraint_name = kcu.constraint_name
             WHERE tc.table_schema = current_schema()
               AND tc.constraint_type = 'PRIMARY KEY'
               AND tc.table_name IN ('nanocode_durable_owners', 'nanocode_durable_states')
             ORDER BY tc.table_name, kcu.ordinal_position",
            &[],
        )
        .await
        .map_err(backend)?;
    let primary_keys = primary_keys
        .into_iter()
        .map(|row| {
            (
                row.get::<_, String>(0),
                row.get::<_, String>(1),
                row.get::<_, i32>(2),
            )
        })
        .collect::<Vec<_>>();
    let expected_primary_keys = [
        ("nanocode_durable_owners", "state_id", 1),
        ("nanocode_durable_states", "state_id", 1),
    ];
    if primary_keys.len() != expected_primary_keys.len()
        || primary_keys
            .iter()
            .zip(expected_primary_keys)
            .any(|(actual, expected)| {
                actual.0 != expected.0 || actual.1 != expected.1 || actual.2 != expected.2
            })
    {
        return Err(StoreError::Backend(
            "incompatible Postgres durability primary keys; each state_id must be the sole primary key"
                .to_owned(),
        ));
    }
    Ok(())
}

fn parse_u64(value: &str, label: &str) -> Result<u64, StoreError> {
    value
        .parse()
        .map_err(|error| StoreError::Backend(format!("invalid {label}: {error}")))
}

fn backend(error: tokio_postgres::Error) -> StoreError {
    StoreError::Backend(error.to_string())
}
