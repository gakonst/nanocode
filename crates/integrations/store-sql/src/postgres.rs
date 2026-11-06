use tokio_postgres::{
    Client,
    types::{ToSql, Type},
};

use nanocode_durability_core::{Sql, SqlDialect, SqlRow, SqlStatement, StoreError, StoreFuture};

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

fn backend(error: tokio_postgres::Error) -> StoreError {
    StoreError::Backend(error.to_string())
}
