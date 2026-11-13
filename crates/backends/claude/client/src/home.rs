use std::{
    collections::BTreeMap,
    io::{self, Write as _},
    net::Ipv4Addr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream},
};

use crate::{
    ClaudeAuthFuture, ClaudeAuthProvider, ClaudeAuthUnavailable,
    subscription::{
        ClaudeLoginMode, ClaudeSubscription, ClaudeSubscriptionCommit, ClaudeSubscriptionConfig,
        ClaudeSubscriptionError, ClaudeSubscriptionHost, ClaudeSubscriptionHostError,
        ClaudeSubscriptionHttpRequest, ClaudeSubscriptionHttpResponse, ClaudeSubscriptionStatus,
        ClaudeSubscriptionStoreValue, now,
    },
};

const FILE: &str = "claude.json";
const KEY: &str = "claude";
const CALLBACK_PATH: &str = "/callback";
const MAX_CALLBACK_BYTES: usize = 16 * 1024;
const SIGNED_IN: &str = "<!doctype html><title>nanocode</title><p>Signed in to Claude. You can close this tab and return to nanocode.</p>";
const FAILED: &str = "<!doctype html><title>nanocode</title><p>Claude sign-in failed. Return to nanocode for details.</p>";

#[derive(Default, Deserialize, Serialize)]
struct Entry {
    revision: u64,
    payload: Option<String>,
}

struct HomeHost {
    path: PathBuf,
    http: nanocode_http::Client,
}

impl HomeHost {
    fn read(path: &Path) -> io::Result<BTreeMap<String, Entry>> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(error) => Err(error),
        }
    }

    fn swap(
        path: &Path,
        key: &str,
        expected: u64,
        payload: &str,
    ) -> io::Result<ClaudeSubscriptionCommit> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path.with_extension("json.lock"))?;
        lock.lock()?;
        let mut entries = Self::read(path)?;
        let current = entries.get(key).map_or(0, |entry| entry.revision);
        if current != expected {
            return Ok(ClaudeSubscriptionCommit::Conflict(current));
        }
        let revision = expected.saturating_add(1);
        entries.insert(
            key.to_owned(),
            Entry {
                revision,
                payload: Some(payload.to_owned()),
            },
        );
        let temporary = path.with_extension("json.tmp");
        let mut options = std::fs::OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&temporary)?;
        file.write_all(&serde_json::to_vec(&entries).map_err(io::Error::other)?)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        Ok(ClaudeSubscriptionCommit::Committed(revision))
    }
}

impl ClaudeSubscriptionHost for HomeHost {
    fn load<'a>(
        &'a self,
        key: &'a str,
    ) -> ClaudeAuthFuture<'a, Result<ClaudeSubscriptionStoreValue, ClaudeSubscriptionHostError>>
    {
        let path = self.path.clone();
        Box::pin(async move {
            let mut entries = nanocode_platform::spawn_blocking(move || Self::read(&path))
                .await
                .map_err(|_| ClaudeSubscriptionHostError)?
                .map_err(|_| ClaudeSubscriptionHostError)?;
            Ok(entries
                .remove(key)
                .map_or_else(ClaudeSubscriptionStoreValue::default, |entry| {
                    ClaudeSubscriptionStoreValue {
                        revision: entry.revision,
                        payload: entry.payload,
                    }
                }))
        })
    }

    fn compare_and_swap<'a>(
        &'a self,
        key: &'a str,
        expected_revision: u64,
        payload: &'a str,
    ) -> ClaudeAuthFuture<'a, Result<ClaudeSubscriptionCommit, ClaudeSubscriptionHostError>> {
        let (path, key, payload) = (self.path.clone(), key.to_owned(), payload.to_owned());
        Box::pin(async move {
            nanocode_platform::spawn_blocking(move || {
                Self::swap(&path, &key, expected_revision, &payload)
            })
            .await
            .map_err(|_| ClaudeSubscriptionHostError)?
            .map_err(|_| ClaudeSubscriptionHostError)
        })
    }

    fn request(
        &self,
        request: ClaudeSubscriptionHttpRequest,
    ) -> ClaudeAuthFuture<'_, Result<ClaudeSubscriptionHttpResponse, ClaudeSubscriptionHostError>>
    {
        Box::pin(async move {
            let method = nanocode_http::Method::from_bytes(request.method().as_bytes())
                .map_err(|_| ClaudeSubscriptionHostError)?;
            let mut builder = self
                .http
                .request(method, request.url())
                .headers(request.headers().clone())
                .timeout(Duration::from_millis(request.timeout_millis()));
            if !request.body().is_empty() {
                builder = builder
                    .header(nanocode_http::header::CONTENT_TYPE, request.content_type())
                    .body(request.body().to_owned());
            }
            let response = builder
                .send()
                .await
                .map(nanocode_http::decode_flate_response)
                .map_err(|_| ClaudeSubscriptionHostError)?;
            let status = response.status().as_u16();
            let body = response
                .bytes()
                .await
                .map_err(|_| ClaudeSubscriptionHostError)?;
            if body.len() > request.max_response_bytes() {
                return Err(ClaudeSubscriptionHostError);
            }
            Ok(ClaudeSubscriptionHttpResponse {
                status,
                body: String::from_utf8_lossy(&body).into_owned(),
            })
        })
    }
}

impl ClaudeSubscription {
    pub fn home() -> Result<Self, ClaudeSubscriptionError> {
        let home =
            nanocode_home::AgentHome::from_env().map_err(|_| ClaudeSubscriptionError::Host)?;
        nanocode_tls::install_default_rustls_crypto_provider();
        Self::new(
            Arc::new(HomeHost {
                path: home.nanocode_home().join(FILE),
                http: nanocode_http::client(),
            }),
            KEY,
            ClaudeSubscriptionConfig::default(),
        )
    }
}

pub struct ClaudeHomeLogin {
    subscription: ClaudeSubscription,
    listener: TcpListener,
    origin: String,
    authorization_url: String,
    expires_at: u64,
}

impl ClaudeHomeLogin {
    pub async fn start() -> Result<Self, ClaudeSubscriptionError> {
        let subscription = ClaudeSubscription::home()?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| ClaudeSubscriptionError::Host)?;
        let port = listener
            .local_addr()
            .map_err(|_| ClaudeSubscriptionError::Host)?
            .port();
        let origin = format!("http://localhost:{port}");
        let login = subscription
            .begin_login(ClaudeLoginMode::Callback {
                redirect_uri: format!("{origin}{CALLBACK_PATH}"),
            })
            .await?;
        Ok(Self {
            subscription,
            listener,
            origin,
            authorization_url: login.authorization_url,
            expires_at: login.expires_at,
        })
    }

    #[must_use]
    pub fn authorization_url(&self) -> &str {
        &self.authorization_url
    }

    pub async fn complete(self) -> Result<ClaudeSubscriptionStatus, ClaudeSubscriptionError> {
        let remaining = Duration::from_millis(self.expires_at.saturating_sub(now()));
        let (mut stream, target) = tokio::time::timeout(remaining, callback(&self.listener))
            .await
            .map_err(|_| ClaudeSubscriptionError::LoginRequired)?
            .map_err(|_| ClaudeSubscriptionError::Host)?;
        let result = self
            .subscription
            .complete_login(&format!("{}{target}", self.origin))
            .await;
        reply(
            &mut stream,
            200,
            if result.is_ok() { SIGNED_IN } else { FAILED },
        )
        .await;
        result
    }
}

async fn callback(listener: &TcpListener) -> io::Result<(TcpStream, String)> {
    loop {
        let (mut stream, _) = listener.accept().await?;
        let mut request = Vec::new();
        let mut buffer = [0; 2048];
        while !request.windows(4).any(|window| window == b"\r\n\r\n")
            && request.len() < MAX_CALLBACK_BYTES
        {
            let read = stream.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
        }
        let target = String::from_utf8_lossy(&request)
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("GET "))
            .and_then(|rest| rest.split(' ').next())
            .unwrap_or_default()
            .to_owned();
        if target.split('?').next() == Some(CALLBACK_PATH) {
            return Ok((stream, target));
        }
        reply(&mut stream, 404, "").await;
    }
}

async fn reply(stream: &mut TcpStream, status: u16, body: &str) {
    let reason = if status == 200 { "OK" } else { "Not Found" };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    drop(stream.write_all(response.as_bytes()).await);
    drop(stream.shutdown().await);
}

pub(crate) struct HomeFirst {
    home: Option<ClaudeSubscription>,
    fallback: Arc<dyn ClaudeAuthProvider>,
}

impl HomeFirst {
    pub(crate) fn new(fallback: Arc<dyn ClaudeAuthProvider>) -> Self {
        Self {
            home: ClaudeSubscription::home().ok(),
            fallback,
        }
    }

    async fn signed_in(&self) -> Option<&ClaudeSubscription> {
        let home = self.home.as_ref()?;
        matches!(
            home.status().await,
            Ok(ClaudeSubscriptionStatus::Authenticated { .. })
        )
        .then_some(home)
    }
}

impl ClaudeAuthProvider for HomeFirst {
    fn headers(
        &self,
    ) -> ClaudeAuthFuture<'_, Result<http::header::HeaderMap, ClaudeAuthUnavailable>> {
        Box::pin(async move {
            if let Some(home) = &self.home
                && let Some(headers) = home.ready_headers().await
            {
                return headers.map_err(|error| ClaudeAuthUnavailable::new(error.to_string()));
            }
            self.fallback.headers().await.map_err(|error| {
                ClaudeAuthUnavailable::new(format!(
                    "{}; sign in with /login claude",
                    error.reason()
                ))
            })
        })
    }

    fn recover_unauthorized<'a>(
        &'a self,
        rejected: &'a http::header::HeaderMap,
    ) -> ClaudeAuthFuture<'a, Result<bool, ClaudeAuthUnavailable>> {
        Box::pin(async move {
            match self.signed_in().await {
                Some(home) => home.recover_unauthorized(rejected).await,
                None => self.fallback.recover_unauthorized(rejected).await,
            }
        })
    }
}
