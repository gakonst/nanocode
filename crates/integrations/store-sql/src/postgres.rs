use futures_util::future::{select_all, try_join, try_join_all, try_join3};
use nanocode_durability_core::LockKey;
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
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{fs::read, spawn, time::timeout};
use tokio_postgres::{Client, Config, NoTls, Row, Statement, config::SslMode, types::ToSql};
use tokio_postgres_rustls::MakeRustlsConnect;
use webpki_roots::TLS_SERVER_ROOTS;

const OPEN: &str = "BEGIN";
const LOCKS: &str = "SELECT pg_advisory_xact_lock_shared($1::bigint), (SELECT count(*) FROM (
    SELECT CASE WHEN s THEN pg_advisory_xact_lock_shared(c,k)::text ELSE pg_advisory_xact_lock(c,k)::text END
    FROM unnest($2::int[],$3::int[],$4::bool[]) WITH ORDINALITY AS u(c,k,s,n) ORDER BY n) taken)";
const CLOSE: &str = "COMMIT";

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

#[derive(Clone, Debug)]
pub struct Pipelined {
    pub application: String,
    pub settings: Vec<(&'static str, String)>,
    pub setup: &'static str,
    pub write_lock: i64,
    pub collected: &'static str,
    pub deadline: Duration,
    pub backoff: Duration,
    pub ping: fn(Duration) -> bool,
}

pub struct Locked {
    pub rows: Vec<Vec<Row>>,
    pub collected: String,
}

pub struct PostgresSql {
    dial: Dial,
    pipelined: Pipelined,
    lanes: Box<[Owned<Option<Lane>>]>,
    failed: Atom<Option<Instant>>,
    latest: AtomicUsize,
}

struct Dial {
    config: Config,
    tls: Tls,
}

struct Lane {
    client: Client,
    prepared: HashMap<String, Statement>,
    used: Instant,
}

impl Lane {
    fn new(client: Client) -> Self {
        Self {
            client,
            prepared: HashMap::new(),
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

impl PostgresSql {
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
            dial: Dial { config, tls },
            pipelined,
            lanes: (0..size.max(1)).map(|_| Owned::new(None)).collect(),
            failed: Atom::new(None),
            latest: AtomicUsize::new(0),
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
        timeout(self.pipelined.deadline, work)
            .await
            .unwrap_or(Err(PipelineError::Said(late)))
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
        let Tls {
            mode,
            root,
            cert,
            key,
        } = &self.dial.tls;
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
        self.failed
            .get()
            .is_none_or(|at| at.elapsed() >= self.pipelined.backoff)
    }

    async fn reconnect(&self) -> Result<Lane, PipelineError> {
        let dialed = async { Ok(Lane::new(self.open().await?)) };
        let connected = self.bounded(dialed, "connecting timed out").await;
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
        let config = &self.dial.config;
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

    async fn fresh(&self, lane: &Lane) -> bool {
        if lane.client.is_closed() {
            return false;
        }
        if !(self.pipelined.ping)(lane.used.elapsed()) {
            return true;
        }
        timeout(self.pipelined.deadline, lane.client.simple_query(""))
            .await
            .is_ok_and(|pinged| pinged.is_ok())
    }

    async fn slot(&self) -> (usize, Held<'_, Option<Lane>>) {
        let start = self.latest.load(Ordering::Relaxed);
        let count = self.lanes.len();
        if let Some(held) = (0..count).find_map(|offset| {
            let index = (start + offset) % count;
            self.lanes[index].try_acquire().map(|held| (index, held))
        }) {
            return held;
        }
        let (held, index, _) =
            select_all(self.lanes.iter().map(|slot| Box::pin(slot.acquire()))).await;
        (index, held)
    }

    fn release(&self, index: usize, slot: &mut Held<'_, Option<Lane>>, mut lane: Lane) {
        if !lane.client.is_closed() {
            lane.used = Instant::now();
            **slot = Some(lane);
            self.latest.store(index, Ordering::Relaxed);
        }
    }

    async fn lane(&self, slot: &mut Held<'_, Option<Lane>>) -> Result<Lane, PipelineError> {
        match slot.take() {
            Some(lane) if self.fresh(&lane).await => Ok(lane),
            _ => self.reconnect().await,
        }
    }

    pub async fn once<G>(
        &self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
        lent: impl FnOnce() -> G,
    ) -> Result<Vec<Row>, PipelineError> {
        let (index, mut slot) = self.slot().await;
        let mut lane = self.lane(&mut slot).await?;
        let _lent = lent();
        let statement = match lane.prepare(sql).await {
            Ok(statement) => statement,
            Err(error) => {
                self.release(index, &mut slot, lane);
                return Err(error.into());
            }
        };
        let ran = self
            .bounded(
                async { Ok(lane.client.query(&statement, params).await) },
                "pipelined statement timed out",
            )
            .await?;
        self.release(index, &mut slot, lane);
        Ok(ran?)
    }

    pub async fn locked<G>(
        &self,
        keys: &[LockKey],
        statements: &[(&str, &[&(dyn ToSql + Sync)])],
        lent: impl FnOnce() -> G,
    ) -> Result<Locked, PipelineError> {
        let pipelined = &self.pipelined;
        let (index, mut slot) = self.slot().await;
        let mut lane = self.lane(&mut slot).await?;
        let _lent = lent();
        let bracket = [OPEN, pipelined.setup, LOCKS, pipelined.collected, CLOSE];
        let texts = bracket
            .into_iter()
            .chain(statements.iter().map(|(text, _)| *text));
        let mut prepared = Vec::with_capacity(bracket.len() + statements.len());
        for text in texts {
            match lane.prepare(text).await {
                Ok(statement) => prepared.push(statement),
                Err(error) => {
                    self.release(index, &mut slot, lane);
                    return Err(error.into());
                }
            }
        }
        let (fixed, prepared) = prepared.split_at(bracket.len());
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
        self.release(index, &mut slot, lane);
        let (_, rows, (collected, _)) = ran?;
        let collected: Option<String> = collected.try_get(0)?;
        Ok(Locked {
            rows,
            collected: collected.unwrap_or_default(),
        })
    }
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
