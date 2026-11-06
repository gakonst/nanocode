use futures_util::future::{select_all, try_join, try_join_all, try_join3};
use nanocode_durability_core::{
    LockKey, Sql, SqlAccess, SqlDialect, SqlRow, SqlStatement, StoreError, StoreFuture,
};
use nanocode_sync::{Atom, Held, Owned};
use percent_encoding::percent_decode_str;
use rustls::{
    CertificateError, ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme,
    client::{
        VerifierBuilderError, WebPkiServerVerifier,
        danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    },
    crypto::{
        CryptoProvider, ring::default_provider, verify_tls12_signature, verify_tls13_signature,
    },
    pki_types::{
        CertificateDer, PrivateKeyDer, ServerName, UnixTime,
        pem::{self, PemObject},
    },
};
use sqlx::{
    ConnectOptions as _,
    postgres::{PgConnectOptions, PgSslMode},
};
use std::{
    collections::HashMap,
    fmt::{self, Display, Formatter},
    future::Future,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{fs::read, spawn, time::timeout};
use tokio_postgres::{
    Client, Config, NoTls, Row, Statement,
    config::SslMode,
    types::{ToSql, Type},
};
use tokio_postgres_rustls::MakeRustlsConnect;
use webpki_roots::TLS_SERVER_ROOTS;

const OPEN: &str = "BEGIN";
const LOCKS: &str = "SELECT pg_advisory_xact_lock_shared($1::bigint), (SELECT count(*) FROM (
    SELECT CASE WHEN s THEN pg_advisory_xact_lock_shared(c,k)::text ELSE pg_advisory_xact_lock(c,k)::text END
    FROM unnest($2::int[],$3::int[],$4::bool[]) WITH ORDINALITY AS u(c,k,s,n) ORDER BY n) taken)";
const CLOSE: &str = "COMMIT";
const CUSTOM_PLANS: &str = "SET plan_cache_mode = force_custom_plan";

#[derive(Debug)]
pub struct Unavailable(String);

impl Unavailable {
    #[must_use]
    pub fn cause(&self) -> &str {
        &self.0
    }
}

impl Display for Unavailable {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "pipelined connection unavailable: {}", self.0)
    }
}

impl std::error::Error for Unavailable {}

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error(transparent)]
    Unavailable(#[from] Unavailable),
    #[error(transparent)]
    Postgres(#[from] tokio_postgres::Error),
    #[error(transparent)]
    Tls(#[from] rustls::Error),
    #[error(transparent)]
    Pem(#[from] pem::Error),
    #[error(transparent)]
    Verifier(#[from] VerifierBuilderError),
    #[error(transparent)]
    Password(#[from] std::str::Utf8Error),
    #[error("reading {path}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("invalid certificate {root}")]
    Certificate { root: String, source: pem::Error },
    #[error("{0}")]
    Said(&'static str),
}

#[derive(Clone, Copy, Debug)]
pub struct Alive {
    pub statement: &'static str,
    pub standby: &'static str,
    pub lapsed: &'static str,
}

#[derive(Clone, Debug)]
pub struct Pipelined {
    pub application: String,
    pub settings: Vec<(&'static str, String)>,
    pub setup: &'static str,
    pub write_lock: i64,
    pub collected: &'static str,
    pub alive: Option<Alive>,
    pub deadline: Duration,
    pub backoff: Duration,
    pub ping_idle: Duration,
}

pub struct Locked {
    pub rows: Vec<Vec<Row>>,
    pub collected: String,
}

pub struct PostgresSql {
    dial: Option<Dial>,
    pipelined: Option<Pipelined>,
    lanes: Box<[Owned<Option<Lane>>]>,
    failed: Atom<Option<Instant>>,
    next: AtomicUsize,
    custom_plans: bool,
}

struct Dial {
    config: Config,
    tls: Tls,
}

struct Lane {
    client: Client,
    typed: HashMap<&'static str, Statement>,
    prepared: HashMap<String, Statement>,
    planned: bool,
    used: Instant,
}

impl Lane {
    fn new(client: Client) -> Self {
        Self {
            client,
            typed: HashMap::new(),
            prepared: HashMap::new(),
            planned: false,
            used: Instant::now(),
        }
    }

    async fn prepare(&mut self, sql: &str) -> Result<Statement, tokio_postgres::Error> {
        if let Some(statement) = self.prepared.get(sql) {
            return Ok(statement.clone());
        }
        let statement = self.client.prepare(sql).await?;
        self.prepared.insert(sql.to_owned(), statement.clone());
        Ok(statement)
    }
}

fn inferred(statements: &[SqlStatement<'_>]) -> SqlAccess {
    let starts = |verbs: &[&str]| {
        statements
            .iter()
            .all(|statement| verbs.iter().any(|verb| statement.sql.starts_with(verb)))
    };
    if starts(&["SELECT"]) {
        SqlAccess::Read
    } else if starts(&["SELECT", "INSERT", "UPDATE", "DELETE", "WITH"]) {
        SqlAccess::Write
    } else {
        SqlAccess::Schema
    }
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
            dial: None,
            pipelined: None,
            lanes: clients
                .into_iter()
                .map(|client| Owned::new(Some(Lane::new(client))))
                .collect(),
            failed: Atom::new(None),
            next: AtomicUsize::new(0),
            custom_plans: true,
        }
    }

    pub async fn connect(config: &str, connections: usize) -> Result<Self, StoreError> {
        if connections == 0 {
            return Err(StoreError::Backend(
                "a Postgres pool needs a connection".to_owned(),
            ));
        }
        static RUNTIME: OnceLock<Option<tokio::runtime::Handle>> = OnceLock::new();
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
                    let (client, connection) = tokio_postgres::connect(&config, NoTls)
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

    pub fn dialing(
        options: &PgConnectOptions,
        size: u32,
        pipelined: Pipelined,
    ) -> Result<Self, PipelineError> {
        let url = options.to_url_lossy();
        let query = |key: &str| {
            url.query_pairs()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.into_owned())
        };
        let mode = options.get_ssl_mode();
        let mut config = Config::new();
        match options.get_socket() {
            Some(socket) => config.host_path(socket),
            None => config.host(options.get_host()),
        };
        if let Some(password) = url.password() {
            config.password(percent_decode_str(password).decode_utf8()?.as_ref());
        }
        if let Some(database) = options.get_database() {
            config.dbname(database);
        }
        let settings: Vec<String> = pipelined
            .settings
            .iter()
            .map(|(name, value)| format!("-c {name}={value}"))
            .collect();
        let options_line: Vec<&str> = options
            .get_options()
            .into_iter()
            .chain(settings.iter().map(String::as_str))
            .collect();
        config
            .port(options.get_port())
            .user(options.get_username())
            .ssl_mode(match mode {
                PgSslMode::Disable | PgSslMode::Allow => SslMode::Disable,
                PgSslMode::Prefer => SslMode::Prefer,
                PgSslMode::Require | PgSslMode::VerifyCa | PgSslMode::VerifyFull => {
                    SslMode::Require
                }
            })
            .application_name(&pipelined.application)
            .options(options_line.join(" "));
        let tls = Tls {
            mode,
            root: query("sslrootcert"),
            cert: query("sslcert"),
            key: query("sslkey"),
        };
        if tls.cert.is_some() != tls.key.is_some() {
            return Err(PipelineError::Said(
                "sslcert and sslkey must be given together",
            ));
        }
        Ok(Self {
            dial: Some(Dial { config, tls }),
            pipelined: Some(pipelined),
            lanes: (0..size.max(1)).map(|_| Owned::new(None)).collect(),
            failed: Atom::new(None),
            next: AtomicUsize::new(0),
            custom_plans: false,
        })
    }

    pub async fn probe(&self) -> Result<(), PipelineError> {
        let opened = async {
            let client = self.open().await?;
            client.simple_query("").await?;
            Ok(())
        };
        self.bounded(opened, "connecting timed out").await
    }

    async fn bounded<T>(
        &self,
        work: impl Future<Output = Result<T, PipelineError>>,
        late: &'static str,
    ) -> Result<T, PipelineError> {
        match &self.pipelined {
            Some(pipelined) => timeout(pipelined.deadline, work)
                .await
                .unwrap_or(Err(PipelineError::Said(late))),
            None => work.await,
        }
    }

    async fn tls(&self) -> Result<Option<MakeRustlsConnect>, PipelineError> {
        let data = async |input: &str| -> Result<Vec<u8>, PipelineError> {
            Ok(match input.strip_prefix("file: ") {
                Some(path) => read(path).await.map_err(|source| PipelineError::Read {
                    path: path.to_owned(),
                    source,
                })?,
                None => input.as_bytes().to_vec(),
            })
        };
        let Some(Dial {
            tls:
                Tls {
                    mode,
                    root,
                    cert,
                    key,
                },
            ..
        }) = &self.dial
        else {
            return Ok(None);
        };
        if matches!(mode, PgSslMode::Disable | PgSslMode::Allow) {
            return Ok(None);
        }
        let provider = Arc::new(default_provider());
        let builder = ClientConfig::builder_with_provider(Arc::clone(&provider))
            .with_safe_default_protocol_versions()?;
        let builder = if matches!(mode, PgSslMode::VerifyCa | PgSslMode::VerifyFull) {
            let mut store = RootCertStore::from_iter(TLS_SERVER_ROOTS.iter().cloned());
            if let Some(root) = root {
                for cert in CertificateDer::pem_slice_iter(&data(root).await?) {
                    store.add(cert.map_err(|source| PipelineError::Certificate {
                        root: root.clone(),
                        source,
                    })?)?;
                }
            }
            if matches!(mode, PgSslMode::VerifyFull) {
                builder.with_root_certificates(store)
            } else {
                let chain = WebPkiServerVerifier::builder_with_provider(
                    Arc::new(store),
                    Arc::clone(&provider),
                )
                .build()?;
                builder
                    .dangerous()
                    .with_custom_certificate_verifier(Arc::new(Unchecked {
                        chain: Some(chain),
                        provider,
                    }))
            }
        } else {
            builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(Unchecked {
                    chain: None,
                    provider,
                }))
        };
        let config = match (cert, key) {
            (Some(cert), Some(key)) => {
                let certs = CertificateDer::pem_slice_iter(&data(cert).await?)
                    .collect::<Result<Vec<_>, _>>()?;
                builder.with_client_auth_cert(
                    certs,
                    PrivateKeyDer::from_pem_slice(&data(key).await?)?,
                )?
            }
            _ => builder.with_no_client_auth(),
        };
        Ok(Some(MakeRustlsConnect::new(config)))
    }

    #[must_use]
    pub fn available(&self) -> bool {
        let backoff = self
            .pipelined
            .as_ref()
            .map_or(Duration::ZERO, |pipelined| pipelined.backoff);
        self.failed.get().is_none_or(|at| at.elapsed() >= backoff)
    }

    async fn reconnect(&self) -> Result<Lane, PipelineError> {
        if self.dial.is_none() {
            return Err(Unavailable("the connection closed".to_owned()).into());
        }
        let connected = self.bounded(self.dialed(), "connecting timed out").await;
        match connected {
            Ok(lane) => {
                self.failed.set(None);
                Ok(lane)
            }
            Err(error) => {
                self.failed.set(Some(Instant::now()));
                Err(Unavailable(chain(&error)).into())
            }
        }
    }

    async fn open(&self) -> Result<Client, PipelineError> {
        let Some(Dial { config, .. }) = &self.dial else {
            return Err(PipelineError::Said(
                "this Postgres host has no dialing options",
            ));
        };
        let client = match self.tls().await? {
            None => {
                let (client, connection) = config.connect(NoTls).await?;
                spawn(connection);
                client
            }
            Some(tls) => {
                let (client, connection) = config.connect(tls).await?;
                spawn(connection);
                client
            }
        };
        Ok(client)
    }

    async fn dialed(&self) -> Result<Lane, PipelineError> {
        let client = self.open().await?;
        if let Some(Alive {
            statement,
            standby,
            lapsed,
        }) = self
            .pipelined
            .as_ref()
            .and_then(|pipelined| pipelined.alive)
        {
            let row = client.query_one(statement, &[]).await?;
            let (alive, replica): (Option<bool>, bool) = (row.try_get(0)?, row.try_get(1)?);
            if replica {
                return Err(PipelineError::Said(standby));
            }
            if alive != Some(true) {
                return Err(PipelineError::Said(lapsed));
            }
        }
        Ok(Lane::new(client))
    }

    async fn fresh(&self, lane: &Lane) -> bool {
        if lane.client.is_closed() {
            return false;
        }
        let Some(pipelined) = &self.pipelined else {
            return true;
        };
        if lane.used.elapsed() < pipelined.ping_idle {
            return true;
        }
        timeout(pipelined.deadline, lane.client.simple_query(""))
            .await
            .is_ok_and(|pinged| pinged.is_ok())
    }

    async fn slot(&self) -> Held<'_, Option<Lane>> {
        let start = self.next.fetch_add(1, Ordering::Relaxed);
        let count = self.lanes.len();
        if let Some(held) =
            (0..count).find_map(|offset| self.lanes[(start + offset) % count].try_acquire())
        {
            return held;
        }
        select_all(self.lanes.iter().map(|slot| Box::pin(slot.acquire())))
            .await
            .0
    }

    async fn lane(&self, slot: &mut Held<'_, Option<Lane>>) -> Result<Lane, PipelineError> {
        match slot.take() {
            Some(lane) if self.fresh(&lane).await => Ok(lane),
            _ => self.reconnect().await,
        }
    }

    fn settings(&self) -> Result<&Pipelined, PipelineError> {
        self.pipelined.as_ref().ok_or(PipelineError::Said(
            "this Postgres host has no pipelined settings",
        ))
    }

    pub async fn once<G>(
        &self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
        lent: impl FnOnce() -> G,
    ) -> Result<Vec<Row>, PipelineError> {
        let mut slot = self.slot().await;
        let mut lane = self.lane(&mut slot).await?;
        let _lent = lent();
        let statement = lane.prepare(sql).await?;
        let ran = self
            .bounded(
                async { Ok(lane.client.query(&statement, params).await) },
                "pipelined statement timed out",
            )
            .await?;
        if !lane.client.is_closed() {
            lane.used = Instant::now();
            *slot = Some(lane);
        }
        Ok(ran?)
    }

    pub async fn locked<G>(
        &self,
        keys: &[LockKey],
        statements: &[(&str, &[&(dyn ToSql + Sync)])],
        lent: impl FnOnce() -> G,
    ) -> Result<Locked, PipelineError> {
        let pipelined = self.settings()?;
        let mut slot = self.slot().await;
        let mut lane = self.lane(&mut slot).await?;
        let _lent = lent();
        let mut fixed = Vec::with_capacity(5);
        for text in [OPEN, pipelined.setup, LOCKS, pipelined.collected, CLOSE] {
            fixed.push(lane.prepare(text).await?);
        }
        let mut prepared = Vec::with_capacity(statements.len());
        for (text, _) in statements {
            prepared.push(lane.prepare(text).await?);
        }
        let classes: Vec<i32> = keys.iter().map(|key| key.class).collect();
        let ids: Vec<i32> = keys.iter().map(|key| key.id).collect();
        let shared: Vec<bool> = keys.iter().map(|key| key.shared).collect();
        let write_lock = pipelined.write_lock;
        let client = &lane.client;
        let ran = timeout(
            pipelined.deadline,
            try_join3(
                try_join3(
                    client.execute(&fixed[0], &[]),
                    client.execute(&fixed[1], &[]),
                    client.execute(&fixed[2], &[&write_lock, &classes, &ids, &shared]),
                ),
                try_join_all(
                    prepared
                        .iter()
                        .zip(statements)
                        .map(|(statement, (_, params))| client.query(statement, params)),
                ),
                try_join(
                    client.query_one(&fixed[3], &[]),
                    client.execute(&fixed[4], &[]),
                ),
            ),
        )
        .await;
        let Ok(ran) = ran else {
            drop(lane);
            return Err(PipelineError::Said("pipelined locked write timed out"));
        };
        if !lane.client.is_closed() {
            lane.used = Instant::now();
            *slot = Some(lane);
        }
        let (_, rows, (collected, _)) = ran?;
        let collected: Option<String> = collected.try_get(0)?;
        Ok(Locked {
            rows,
            collected: collected.unwrap_or_default(),
        })
    }

    async fn ran(
        &self,
        access: SqlAccess,
        statements: &[SqlStatement<'_>],
    ) -> Result<Vec<Vec<SqlRow>>, StoreError> {
        let mut slot = self.slot().await;
        let mut lane = self.lane(&mut slot).await.map_err(piped)?;
        let ran = self
            .bounded(
                async { Ok(run_on(&mut lane, access, statements, self.custom_plans).await) },
                "a Postgres transaction timed out",
            )
            .await;
        if !lane.client.is_closed() {
            lane.used = Instant::now();
            *slot = Some(lane);
        }
        let replies = ran.map_err(piped)?.map_err(backend)?;
        text_rows(&replies)
    }
}

async fn run_on(
    lane: &mut Lane,
    access: SqlAccess,
    statements: &[SqlStatement<'_>],
    custom_plans: bool,
) -> Result<Vec<Vec<Row>>, tokio_postgres::Error> {
    if access != SqlAccess::Schema {
        if custom_plans && !lane.planned {
            lane.client.batch_execute(CUSTOM_PLANS).await?;
            lane.planned = true;
        }
        let missing = statements
            .iter()
            .filter(|statement| !lane.typed.contains_key(statement.sql))
            .map(|statement| (statement.sql, statement.params.len()))
            .collect::<HashMap<_, _>>();
        let client = &lane.client;
        let prepared = try_join_all(missing.iter().map(|(sql, count)| {
            let types = vec![Type::TEXT; *count];
            async move {
                client
                    .prepare_typed(sql, &types)
                    .await
                    .map(|statement| (*sql, statement))
            }
        }))
        .await?;
        lane.typed.extend(prepared);
    }
    let bracket = if access == SqlAccess::Read {
        None
    } else {
        Some((lane.prepare(OPEN).await?, lane.prepare(CLOSE).await?))
    };
    let Lane { client, typed, .. } = &*lane;
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
    let queries = try_join_all(statements.iter().zip(&params).map(|(statement, params)| {
        let prepared = typed.get(statement.sql);
        async move {
            match prepared {
                Some(prepared) => {
                    let values = params.iter().map(|(value, _)| *value).collect::<Vec<_>>();
                    client.query(prepared, &values).await
                }
                None => client.query_typed(statement.sql, params).await,
            }
        }
    }));
    match &bracket {
        None => queries.await,
        Some((open, close)) => {
            let (_, rows, _) = try_join3(
                client.execute(open, &[]),
                queries,
                client.execute(close, &[]),
            )
            .await?;
            Ok(rows)
        }
    }
}

impl Sql for PostgresSql {
    fn transaction<'a>(
        &'a self,
        _operation: &'static str,
        statements: &'a [SqlStatement<'a>],
    ) -> StoreFuture<'a, Result<Vec<Vec<SqlRow>>, StoreError>> {
        Box::pin(self.ran(inferred(statements), statements))
    }

    fn run<'a>(
        &'a self,
        access: SqlAccess,
        _operation: &'static str,
        statements: &'a [SqlStatement<'a>],
    ) -> StoreFuture<'a, Result<Vec<Vec<SqlRow>>, StoreError>> {
        Box::pin(self.ran(access, statements))
    }

    fn dialect(&self) -> SqlDialect {
        SqlDialect::Postgres
    }

    fn concurrent(self: Arc<Self>) -> Option<Arc<dyn Sql>> {
        Some(self)
    }

    fn writers(&self) -> usize {
        self.lanes.len()
    }

    fn readers(&self) -> usize {
        self.lanes.len()
    }
}

fn text_rows(replies: &[Vec<Row>]) -> Result<Vec<Vec<SqlRow>>, StoreError> {
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

fn piped(error: PipelineError) -> StoreError {
    StoreError::Backend(chain(&error))
}

pub fn collected_wakes(collected: &str) -> impl Iterator<Item = (&str, &str)> {
    collected
        .split('\u{1e}')
        .filter(|wake| !wake.is_empty())
        .map(|wake| wake.split_once('\u{1f}').unwrap_or((wake, "")))
}

fn chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut said = error.to_string();
    let mut cause = error.source();
    while let Some(next) = cause {
        said.push_str(": ");
        said.push_str(&next.to_string());
        cause = next.source();
    }
    said
}

struct Tls {
    mode: PgSslMode,
    root: Option<String>,
    cert: Option<String>,
    key: Option<String>,
}

#[derive(Debug)]
struct Unchecked {
    chain: Option<Arc<WebPkiServerVerifier>>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Unchecked {
    fn verify_server_cert(
        &self,
        end: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let Some(chain) = &self.chain else {
            return Ok(ServerCertVerified::assertion());
        };
        match chain.verify_server_cert(end, intermediates, name, ocsp, now) {
            Err(rustls::Error::InvalidCertificate(
                CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. },
            )) => Ok(ServerCertVerified::assertion()),
            verified => verified,
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signed: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            signed,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signed: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            signed,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
