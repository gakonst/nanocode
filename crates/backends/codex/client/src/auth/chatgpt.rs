use std::{
    collections::BTreeMap,
    fmt,
    fs::{File, OpenOptions},
    io::{self, ErrorKind, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};

use super::{
    OpenAiAuth, OpenAiAuthError, OpenAiAuthFuture, OpenAiAuthMode, OpenAiAuthSnapshot,
    OpenAiAuthSource,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use nanocode_digest::Sha256;
use nanocode_sync::{SingleFlight, Swap};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    time::{Duration, timeout},
};
use tracing::Instrument;
use url::Url;

const AUTH_ISSUER: &str = "https://auth.openai.com";
const AUTH_API_BASE_URL: &str = "https://auth.openai.com/api/accounts";
const AUTH_API_BASE_URL_ENV: &str = "CODEX_AUTHAPI_BASE_URL";
const PERSONAL_ACCESS_TOKEN_PREFIX: &str = "at-";
const WHOAMI_PATH: &str = "/v1/user-auth-credential/whoami";
const OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const OAUTH_SCOPE: &str =
    "openid profile email offline_access api.connectors.read api.connectors.invoke";
const CALLBACK_PATH: &str = "/auth/callback";
const CALLBACK_PORTS: [u16; 2] = [1455, 1457];
const REFRESH_EARLY_SECONDS: i64 = 5 * 60;
const LOGIN_TIMEOUT: Duration = Duration::from_mins(5);
const AUTH_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChatGptAuthStatus {
    pub account_id: String,

    pub email: Option<String>,

    pub plan: Option<String>,

    pub fedramp: bool,
}

#[derive(Debug)]
pub enum ChatGptAuthError {
    Storage { path: PathBuf, source: io::Error },

    InvalidStore { path: PathBuf, detail: String },

    InvalidToken(String),

    CallbackUnavailable,

    CallbackTimeout,

    StateMismatch,

    LoginRejected(String),

    TokenExchange(String),

    AccessTokenMetadata(String),
}

impl std::fmt::Display for ChatGptAuthError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage { path, source } => {
                let displayed_path = path.display();
                write!(
                    formatter,
                    "failed to access ChatGPT authorization file {displayed_path}: {source}"
                )
            }
            Self::InvalidStore { path, detail } => {
                let displayed_path = path.display();
                write!(
                    formatter,
                    "ChatGPT authorization file {displayed_path} is invalid: {detail}"
                )
            }
            Self::InvalidToken(value) => {
                write!(formatter, "ChatGPT OAuth response was invalid: {value}")
            }
            Self::CallbackUnavailable => formatter.write_str(
                "could not listen for the OAuth callback on localhost ports 1455 or 1457",
            ),
            Self::CallbackTimeout => {
                formatter.write_str("timed out waiting for the ChatGPT OAuth callback")
            }
            Self::StateMismatch => {
                formatter.write_str("the OAuth callback did not match this login attempt")
            }
            Self::LoginRejected(value) => write!(formatter, "ChatGPT login was rejected: {value}"),
            Self::TokenExchange(value) => {
                write!(formatter, "ChatGPT token exchange failed: {value}")
            }
            Self::AccessTokenMetadata(value) => write!(
                formatter,
                "ChatGPT access-token metadata request failed: {value}"
            ),
        }
    }
}

impl std::error::Error for ChatGptAuthError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Storage { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub struct ChatGptLogin {
    issuer: String,
    authorization_url: String,
    redirect_uri: String,
    state: String,
    code_verifier: String,
    auth_file: PathBuf,
    listener: TcpListener,
    client: nanocode_http::Client,
}

impl fmt::Debug for ChatGptLogin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChatGptLogin")
            .field("issuer", &"[redacted]")
            .field("authorization_url", &"[redacted]")
            .field("redirect_uri", &self.redirect_uri)
            .field("state", &"[redacted]")
            .field("code_verifier", &"[redacted]")
            .field("auth_file", &self.auth_file)
            .finish_non_exhaustive()
    }
}

impl ChatGptLogin {
    pub async fn start(auth_file: impl Into<PathBuf>) -> Result<Self, ChatGptAuthError> {
        Self::start_with_issuer(auth_file.into(), AUTH_ISSUER).await
    }

    async fn start_with_issuer(auth_file: PathBuf, issuer: &str) -> Result<Self, ChatGptAuthError> {
        let listener = bind_callback().await?;
        let port = listener
            .local_addr()
            .map_err(|_| ChatGptAuthError::CallbackUnavailable)?
            .port();
        let redirect_uri = format!("http://localhost:{port}{CALLBACK_PATH}");
        let state = random_urlsafe()?;
        let code_verifier = random_urlsafe()?;
        let code_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()));
        let authorization_url = authorize_url(issuer, &redirect_uri, &state, &code_challenge)?;

        Ok(Self {
            issuer: issuer.to_owned(),
            authorization_url,
            redirect_uri,
            state,
            code_verifier,
            auth_file,
            listener,
            client: auth_client()?,
        })
    }

    #[must_use]
    pub fn authorization_url(&self) -> &str {
        &self.authorization_url
    }

    pub async fn complete(self) -> Result<ChatGptAuthStatus, ChatGptAuthError> {
        let callback = timeout(LOGIN_TIMEOUT, receive_callback(&self.listener))
            .await
            .map_err(|_| ChatGptAuthError::CallbackTimeout)??;
        let result = self.complete_callback(&callback.target).await;
        let reply = callback.reply(result.is_ok()).await;
        match (result, reply) {
            (Ok(status), Ok(())) => Ok(status),
            (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        }
    }

    async fn complete_callback(
        &self,
        callback_target: &str,
    ) -> Result<ChatGptAuthStatus, ChatGptAuthError> {
        let callback = Url::parse(&format!("http://localhost{callback_target}"))
            .map_err(|error| ChatGptAuthError::LoginRejected(error.to_string()))?;
        if callback.path() != CALLBACK_PATH {
            return Err(ChatGptAuthError::LoginRejected(
                "invalid callback path".into(),
            ));
        }
        let query = callback
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect::<std::collections::HashMap<_, _>>();
        if query.get("state") != Some(&self.state) {
            return Err(ChatGptAuthError::StateMismatch);
        }
        if let Some(error) = query.get("error") {
            let detail = query
                .get("error_description")
                .map_or(error.as_str(), String::as_str);
            return Err(ChatGptAuthError::LoginRejected(detail.to_owned()));
        }
        let code = query
            .get("code")
            .ok_or_else(|| ChatGptAuthError::LoginRejected("missing authorization code".into()))?;
        let credentials = exchange_code(
            &self.client,
            &self.issuer,
            code,
            &self.redirect_uri,
            &self.code_verifier,
        )
        .await?;
        let status = credentials.status();
        run_blocking(self.auth_file.clone(), move |path| {
            let _lock = lock_auth_file(path)?;
            write_store(path, &credentials)
        })
        .await?;
        Ok(status)
    }
}

struct OAuthCallback {
    target: String,
    stream: tokio::net::TcpStream,
}

impl OAuthCallback {
    async fn reply(mut self, success: bool) -> Result<(), ChatGptAuthError> {
        let (status, body): (&str, &[u8]) = if success {
            (
                "200 OK",
                b"ChatGPT login completed. You can close this window.",
            )
        } else {
            (
                "400 Bad Request",
                b"ChatGPT login failed. Return to the terminal for details.",
            )
        };
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        self.stream
            .write_all(response.as_bytes())
            .await
            .map_err(|error| ChatGptAuthError::LoginRejected(error.to_string()))?;
        self.stream
            .write_all(body)
            .await
            .map_err(|error| ChatGptAuthError::LoginRejected(error.to_string()))
    }
}

pub fn load_chatgpt_auth(auth_file: impl Into<PathBuf>) -> Result<OpenAiAuth, ChatGptAuthError> {
    let auth_file = auth_file.into();
    let document = read_document(&auth_file)?;
    if let Some(access_token) = personal_access_token_from_document(&document, &auth_file)? {
        return chatgpt_access_token(access_token);
    }
    let credentials = StoredCredentials::from_document(&document, &auth_file)?;
    let manager = ManagedAuthInner {
        auth_file,
        issuer: Arc::from(AUTH_ISSUER),
        client: auth_client()?,
        state: Swap::from_pointee(ManagedState::new(credentials, document, 0)),
        refreshing: SingleFlight::default(),
    };
    Ok(OpenAiAuth::managed_chatgpt(Arc::new(ManagedChatGptAuth {
        inner: Arc::new(manager),
    })))
}

pub fn chatgpt_access_token(
    access_token: impl Into<Arc<str>>,
) -> Result<OpenAiAuth, ChatGptAuthError> {
    personal_access_token_auth(access_token.into(), &auth_api_base_url())
}

pub async fn resolve_chatgpt_auth_status(
    auth_file: impl AsRef<Path>,
) -> Result<ChatGptAuthStatus, ChatGptAuthError> {
    let auth_file = auth_file.as_ref();
    let document = run_blocking(auth_file.to_path_buf(), read_document).await?;
    if let Some(access_token) = personal_access_token_from_document(&document, auth_file)? {
        return hydrate_personal_access_token(
            &auth_client()?,
            &whoami_endpoint(&auth_api_base_url()),
            &access_token,
        )
        .await;
    }
    let credentials = StoredCredentials::from_document(&document, auth_file)?;
    credentials.validate(auth_file)?;
    Ok(credentials.status())
}

pub fn chatgpt_auth_status(
    auth_file: impl AsRef<Path>,
) -> Result<ChatGptAuthStatus, ChatGptAuthError> {
    let auth_file = auth_file.as_ref();
    let credentials = read_store(auth_file)?;
    credentials.validate(auth_file)?;
    Ok(credentials.status())
}

pub fn logout_chatgpt(auth_file: impl AsRef<Path>) -> Result<bool, ChatGptAuthError> {
    let auth_file = auth_file.as_ref();
    if !auth_file
        .try_exists()
        .map_err(|source| ChatGptAuthError::Storage {
            path: auth_file.to_path_buf(),
            source,
        })?
    {
        return Ok(false);
    }
    let lock = open_auth_lock_file(auth_file)?;
    lock.try_lock()
        .map_err(|source| ChatGptAuthError::Storage {
            path: auth_file.to_path_buf(),
            source: source.into(),
        })?;
    match std::fs::remove_file(auth_file) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(source) => Err(ChatGptAuthError::Storage {
            path: auth_file.to_path_buf(),
            source,
        }),
    }
}

#[derive(Clone)]
struct StoredCredentials {
    id_token: String,
    access_token: String,
    refresh_token: String,
    account_id: String,
    email: Option<String>,
    plan: Option<String>,
    fedramp: bool,
}

impl StoredCredentials {
    fn from_tokens(tokens: TokenResponse) -> Result<Self, ChatGptAuthError> {
        let claims: IdClaims = decode_jwt(&tokens.id)?;
        let auth = claims.auth.unwrap_or_default();
        let account_id = auth.account_id.ok_or_else(|| {
            ChatGptAuthError::InvalidToken("ID token has no ChatGPT account ID".into())
        })?;
        Ok(Self {
            id_token: tokens.id,
            access_token: tokens.access,
            refresh_token: tokens.refresh,
            account_id,
            email: claims
                .email
                .or_else(|| claims.profile.and_then(|profile| profile.email)),
            plan: auth.plan,
            fedramp: auth.fedramp,
        })
    }

    fn from_document(document: &CodexAuthDocument, path: &Path) -> Result<Self, ChatGptAuthError> {
        if document
            .auth_mode
            .as_deref()
            .is_some_and(|mode| mode != "chatgpt")
        {
            return Err(ChatGptAuthError::InvalidStore {
                path: path.to_path_buf(),
                detail: "Codex is not logged in with ChatGPT".into(),
            });
        }
        let tokens = document
            .tokens
            .as_ref()
            .ok_or_else(|| ChatGptAuthError::InvalidStore {
                path: path.to_path_buf(),
                detail: "auth.json has no ChatGPT tokens".into(),
            })?;
        let claims: IdClaims = decode_jwt(&tokens.id_token)?;
        let auth = claims.auth.unwrap_or_default();
        let account_id = tokens
            .account_id
            .clone()
            .or(auth.account_id)
            .ok_or_else(|| ChatGptAuthError::InvalidStore {
                path: path.to_path_buf(),
                detail: "auth.json has no ChatGPT account ID".into(),
            })?;
        let credentials = Self {
            id_token: tokens.id_token.clone(),
            access_token: tokens.access_token.clone(),
            refresh_token: tokens.refresh_token.clone(),
            account_id,
            email: claims
                .email
                .or_else(|| claims.profile.and_then(|profile| profile.email)),
            plan: auth.plan,
            fedramp: auth.fedramp,
        };
        credentials.validate(path)?;
        Ok(credentials)
    }

    fn validate(&self, path: &Path) -> Result<(), ChatGptAuthError> {
        if self.access_token.trim().is_empty()
            || self.refresh_token.trim().is_empty()
            || self.account_id.trim().is_empty()
        {
            return Err(ChatGptAuthError::InvalidStore {
                path: path.to_path_buf(),
                detail: "required credential field is empty".into(),
            });
        }
        Ok(())
    }

    fn status(&self) -> ChatGptAuthStatus {
        ChatGptAuthStatus {
            account_id: self.account_id.clone(),
            email: self.email.clone(),
            plan: self.plan.clone(),
            fedramp: self.fedramp,
        }
    }
}

#[derive(Clone, Default, Deserialize, PartialEq, Serialize)]
struct CodexAuthDocument {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    auth_mode: Option<String>,
    #[serde(rename = "OPENAI_API_KEY", default)]
    openai_api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tokens: Option<CodexTokenData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_refresh: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    personal_access_token: Option<String>,
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Deserialize, PartialEq, Serialize)]
struct CodexTokenData {
    id_token: String,
    access_token: String,
    refresh_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}

struct ManagedChatGptAuth {
    inner: Arc<ManagedAuthInner>,
}

struct ManagedAuthInner {
    auth_file: PathBuf,
    issuer: Arc<str>,
    client: nanocode_http::Client,
    state: Swap<ManagedState>,
    refreshing: SingleFlight<Result<(), OpenAiAuthError>>,
}

enum CurrentSnapshot {
    Fresh(OpenAiAuthSnapshot),
    Stale(u64),
}

#[derive(Clone)]
struct ManagedState {
    credentials: StoredCredentials,
    auth_record: CodexAuthDocument,
    snapshot: OpenAiAuthSnapshot,
    expires_at: Option<i64>,
    permanent_failure: Option<Arc<str>>,
}

impl ManagedState {
    fn new(credentials: StoredCredentials, auth_record: CodexAuthDocument, revision: u64) -> Self {
        let snapshot = OpenAiAuthSnapshot::new(
            OpenAiAuthMode::ChatGpt,
            Arc::<str>::from(credentials.access_token.as_str()),
            Some(Arc::<str>::from(credentials.account_id.as_str())),
            credentials.fedramp,
            revision,
        );
        Self {
            expires_at: jwt_expiration(&credentials.access_token),
            credentials,
            auth_record,
            snapshot,
            permanent_failure: None,
        }
    }

    fn next(&self, credentials: StoredCredentials, auth_record: CodexAuthDocument) -> Self {
        Self::new(
            credentials,
            auth_record,
            self.snapshot.revision().wrapping_add(1),
        )
    }

    fn needs_refresh(&self) -> bool {
        self.permanent_failure.is_some()
            || self
                .expires_at
                .is_some_and(|expiry| expiry <= unix_now() + REFRESH_EARLY_SECONDS)
    }

    fn checked_snapshot(&self) -> Result<OpenAiAuthSnapshot, OpenAiAuthError> {
        match &self.permanent_failure {
            Some(detail) => Err(OpenAiAuthError::LoginRequired(Arc::clone(detail))),
            None => Ok(self.snapshot.clone()),
        }
    }
}

#[derive(Deserialize)]
struct PersonalAccessTokenMetadata {
    #[serde(default)]
    email: Option<String>,
    #[serde(rename = "chatgpt_user_id")]
    _chatgpt_user_id: String,
    chatgpt_account_id: String,
    chatgpt_plan_type: String,
    chatgpt_account_is_fedramp: bool,
}

struct PersonalAccessTokenAuth {
    access_token: Arc<str>,
    client: nanocode_http::Client,
    whoami_endpoint: Arc<str>,
    status: tokio::sync::OnceCell<ChatGptAuthStatus>,
}

impl PersonalAccessTokenAuth {
    async fn status(&self) -> Result<&ChatGptAuthStatus, OpenAiAuthError> {
        self.status
            .get_or_try_init(|| async {
                hydrate_personal_access_token(
                    &self.client,
                    &self.whoami_endpoint,
                    &self.access_token,
                )
                .await
                .map_err(|error| OpenAiAuthError::Unavailable(Arc::from(error.to_string())))
            })
            .await
    }
}

impl OpenAiAuthSource for PersonalAccessTokenAuth {
    fn validate(&self) -> Result<(), OpenAiAuthError> {
        validate_personal_access_token(&self.access_token)
            .map_err(|error| OpenAiAuthError::Unavailable(Arc::from(error.to_string())))
    }

    fn snapshot(&self) -> OpenAiAuthFuture<'_, Result<OpenAiAuthSnapshot, OpenAiAuthError>> {
        Box::pin(async move {
            let status = self.status().await?;
            Ok(OpenAiAuthSnapshot::new(
                OpenAiAuthMode::ChatGpt,
                Arc::clone(&self.access_token),
                Some(Arc::<str>::from(status.account_id.as_str())),
                status.fedramp,
                0,
            ))
        })
    }

    fn recover_unauthorized(
        &self,
        _rejected: &OpenAiAuthSnapshot,
    ) -> OpenAiAuthFuture<'_, Result<(), OpenAiAuthError>> {
        Box::pin(async {
            Err(OpenAiAuthError::LoginRequired(Arc::from(
                "persistent access token was rejected and cannot be refreshed",
            )))
        })
    }
}

fn personal_access_token_auth(
    access_token: Arc<str>,
    auth_api_base_url: &str,
) -> Result<OpenAiAuth, ChatGptAuthError> {
    validate_personal_access_token(&access_token)?;
    let source = PersonalAccessTokenAuth {
        access_token,
        client: auth_client()?,
        whoami_endpoint: Arc::from(whoami_endpoint(auth_api_base_url)),
        status: tokio::sync::OnceCell::new(),
    };
    Ok(OpenAiAuth::managed_chatgpt(Arc::new(source)))
}

fn validate_personal_access_token(access_token: &str) -> Result<(), ChatGptAuthError> {
    if access_token.trim().is_empty() {
        return Err(ChatGptAuthError::InvalidToken(
            "persistent access token is empty".into(),
        ));
    }
    if !access_token.starts_with(PERSONAL_ACCESS_TOKEN_PREFIX) {
        return Err(ChatGptAuthError::InvalidToken(
            "persistent access token must start with `at-`".into(),
        ));
    }
    Ok(())
}

fn personal_access_token_from_document(
    document: &CodexAuthDocument,
    path: &Path,
) -> Result<Option<Arc<str>>, ChatGptAuthError> {
    let selected = match document.auth_mode.as_deref() {
        Some("personalAccessToken") => true,
        Some(_) => false,
        None => document.personal_access_token.is_some(),
    };
    if !selected {
        return Ok(None);
    }
    let access_token = document.personal_access_token.as_deref().ok_or_else(|| {
        ChatGptAuthError::InvalidStore {
            path: path.to_path_buf(),
            detail: "auth.json has no personal access token".into(),
        }
    })?;
    validate_personal_access_token(access_token).map_err(|error| {
        ChatGptAuthError::InvalidStore {
            path: path.to_path_buf(),
            detail: error.to_string(),
        }
    })?;
    Ok(Some(Arc::from(access_token)))
}

async fn hydrate_personal_access_token(
    client: &nanocode_http::Client,
    endpoint: &str,
    access_token: &str,
) -> Result<ChatGptAuthStatus, ChatGptAuthError> {
    let response = client
        .get(endpoint)
        .bearer_auth(access_token)
        .send()
        .await
        .map(nanocode_http::decode_flate_response)
        .map_err(|error| ChatGptAuthError::AccessTokenMetadata(error.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(ChatGptAuthError::AccessTokenMetadata(format!(
            "authorization service returned HTTP {status}"
        )));
    }
    let metadata = response
        .json::<PersonalAccessTokenMetadata>()
        .await
        .map_err(|error| ChatGptAuthError::AccessTokenMetadata(error.to_string()))?;
    if metadata.chatgpt_account_id.trim().is_empty() {
        return Err(ChatGptAuthError::AccessTokenMetadata(
            "authorization service returned an empty ChatGPT account ID".into(),
        ));
    }
    Ok(ChatGptAuthStatus {
        account_id: metadata.chatgpt_account_id,
        email: metadata.email,
        plan: Some(metadata.chatgpt_plan_type),
        fedramp: metadata.chatgpt_account_is_fedramp,
    })
}

fn whoami_endpoint(auth_api_base_url: &str) -> String {
    format!(
        "{}{WHOAMI_PATH}",
        auth_api_base_url.trim().trim_end_matches('/')
    )
}

fn auth_api_base_url() -> String {
    std::env::var(AUTH_API_BASE_URL_ENV)
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| AUTH_API_BASE_URL.to_owned())
}

impl ManagedChatGptAuth {
    async fn refresh_if_current(&self, rejected_revision: u64) -> Result<(), OpenAiAuthError> {
        if self.inner.state().snapshot.revision() != rejected_revision {
            return Ok(());
        }
        let inner = Arc::clone(&self.inner);
        self.inner
            .refreshing
            .run(|| async move {
                tokio::spawn(
                    inner
                        .refresh_if_current(rejected_revision)
                        .in_current_span(),
                )
                .await
                .map_err(|error| OpenAiAuthError::Unavailable(Arc::from(error.to_string())))?
            })
            .await
    }
}

impl ManagedAuthInner {
    fn state(&self) -> Arc<ManagedState> {
        self.state.load_full()
    }

    fn current(&self) -> CurrentSnapshot {
        let state = self.state.load();
        if state.needs_refresh() {
            CurrentSnapshot::Stale(state.snapshot.revision())
        } else {
            CurrentSnapshot::Fresh(state.snapshot.clone())
        }
    }

    fn replace_state(&self, next: ManagedState) {
        self.state.store(Arc::new(next));
    }

    async fn refresh_if_current(
        self: Arc<Self>,
        rejected_revision: u64,
    ) -> Result<(), OpenAiAuthError> {
        if self.state().snapshot.revision() != rejected_revision {
            return Ok(());
        }
        let (lock, stored) = run_blocking(self.auth_file.clone(), |path| {
            let lock = lock_auth_file(path)?;
            Ok((lock, read_stored_auth(path)?))
        })
        .await
        .map_err(|error| auth_store_error(&error))?;
        if self.reload_if_changed(stored)? {
            return Ok(());
        }

        let attempted = self.state();
        attempted.checked_snapshot()?;
        let refresh_token = attempted.credentials.refresh_token.clone();
        let response = self
            .client
            .post(format!("{}/oauth/token", self.issuer.trim_end_matches('/')))
            .json(&RefreshRequest {
                client_id: OAUTH_CLIENT_ID,
                grant_type: "refresh_token",
                refresh_token,
            })
            .send()
            .await
            .map(nanocode_http::decode_flate_response)
            .map_err(|error| OpenAiAuthError::Refresh(Arc::from(error.to_string())))?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .map_err(|error| OpenAiAuthError::Refresh(Arc::from(error.to_string())))?;
        if !status.is_success() {
            let code = refresh_error_code(&body);
            let permanent = status == nanocode_http::StatusCode::UNAUTHORIZED
                || matches!(
                    code.as_deref(),
                    Some(
                        "refresh_token_expired"
                            | "refresh_token_reused"
                            | "refresh_token_invalidated"
                    )
                );
            let detail: Arc<str> =
                Arc::from(code.unwrap_or_else(|| format!("token endpoint returned HTTP {status}")));
            if permanent {
                let (_lock, stored) = read_stored_auth_locked(lock, self.auth_file.clone())
                    .await
                    .map_err(|error| auth_store_error(&error))?;
                if self.reload_if_changed(stored)? {
                    return Ok(());
                }
                if self.record_permanent_failure_if_unchanged(&attempted, Arc::clone(&detail))? {
                    return Err(OpenAiAuthError::LoginRequired(detail));
                }
                return Ok(());
            }
            return Err(OpenAiAuthError::Refresh(detail));
        }
        let refreshed: RefreshResponse = serde_json::from_slice(&body)
            .map_err(|error| OpenAiAuthError::Refresh(Arc::from(error.to_string())))?;
        let (lock, stored) = read_stored_auth_locked(lock, self.auth_file.clone())
            .await
            .map_err(|error| auth_store_error(&error))?;
        if self.reload_if_changed(stored)? {
            return Ok(());
        }
        let current = self.state();
        let next = refreshed_credentials(&current.credentials, refreshed)?;
        let saved = next.clone();
        let auth_record = run_blocking(self.auth_file.clone(), move |path| {
            let auth_record = write_store(path, &saved);
            drop(lock);
            auth_record
        })
        .await
        .map_err(|error| auth_store_error(&error))?;
        self.replace_state(current.next(next, auth_record));
        Ok(())
    }

    fn reload_if_changed(
        &self,
        (stored, auth_record): (StoredCredentials, CodexAuthDocument),
    ) -> Result<bool, OpenAiAuthError> {
        let state = self.state();
        if stored.account_id != state.credentials.account_id {
            return Err(OpenAiAuthError::AccountChanged);
        }
        if auth_record == state.auth_record {
            return Ok(false);
        }
        self.replace_state(state.next(stored, auth_record));
        Ok(true)
    }

    fn record_permanent_failure_if_unchanged(
        &self,
        attempted: &ManagedState,
        detail: Arc<str>,
    ) -> Result<bool, OpenAiAuthError> {
        let state = self.state();
        if state.auth_record != attempted.auth_record {
            return Ok(false);
        }
        let mut failed = ManagedState::clone(&state);
        failed.permanent_failure = Some(detail);
        self.replace_state(failed);
        Ok(true)
    }
}

fn refreshed_credentials(
    current: &StoredCredentials,
    refreshed: RefreshResponse,
) -> Result<StoredCredentials, OpenAiAuthError> {
    let mut next = current.clone();
    if let Some(access_token) = refreshed.access {
        next.access_token = access_token;
    }
    if let Some(refresh_token) = refreshed.refresh {
        next.refresh_token = refresh_token;
    }
    if let Some(id_token) = refreshed.id {
        let claims: IdClaims = decode_jwt(&id_token)
            .map_err(|error| OpenAiAuthError::Refresh(Arc::from(error.to_string())))?;
        let auth = claims.auth.unwrap_or_default();
        if let Some(account_id) = auth.account_id
            && account_id != next.account_id
        {
            return Err(OpenAiAuthError::AccountChanged);
        }
        next.id_token = id_token;
        next.email = claims
            .email
            .or_else(|| claims.profile.and_then(|profile| profile.email));
        next.plan = auth.plan;
        next.fedramp = auth.fedramp;
    }
    Ok(next)
}

impl OpenAiAuthSource for ManagedChatGptAuth {
    fn validate(&self) -> Result<(), OpenAiAuthError> {
        self.inner.state().checked_snapshot().map(|_| ())
    }

    fn snapshot(&self) -> OpenAiAuthFuture<'_, Result<OpenAiAuthSnapshot, OpenAiAuthError>> {
        Box::pin(async move {
            let revision = match self.inner.current() {
                CurrentSnapshot::Fresh(snapshot) => return Ok(snapshot),
                CurrentSnapshot::Stale(revision) => revision,
            };
            if let Err(error) = self.refresh_if_current(revision).await {
                tracing::warn!(error = %error, "proactive ChatGPT token refresh failed");
            }
            self.inner.state().checked_snapshot()
        })
    }

    fn recover_unauthorized(
        &self,
        rejected: &OpenAiAuthSnapshot,
    ) -> OpenAiAuthFuture<'_, Result<(), OpenAiAuthError>> {
        let mode = rejected.mode();
        let revision = rejected.revision();
        Box::pin(async move {
            if mode != OpenAiAuthMode::ChatGpt {
                return Err(OpenAiAuthError::LoginRequired(Arc::from(
                    "authorization mode changed",
                )));
            }
            self.refresh_if_current(revision).await
        })
    }
}

#[derive(Default, Deserialize)]
struct IdClaims {
    #[serde(default)]
    email: Option<String>,
    #[serde(rename = "https://api.openai.com/profile", default)]
    profile: Option<ProfileClaims>,
    #[serde(rename = "https://api.openai.com/auth", default)]
    auth: Option<AuthClaims>,
}

#[derive(Deserialize)]
struct ProfileClaims {
    #[serde(default)]
    email: Option<String>,
}

#[derive(Default, Deserialize)]
struct AuthClaims {
    #[serde(rename = "chatgpt_plan_type", default)]
    plan: Option<String>,
    #[serde(rename = "chatgpt_account_id", default)]
    account_id: Option<String>,
    #[serde(rename = "chatgpt_account_is_fedramp", default)]
    fedramp: bool,
}

#[derive(Deserialize)]
struct ExpClaims {
    #[serde(default)]
    exp: Option<i64>,
}

#[derive(Deserialize)]
struct TokenResponse {
    #[serde(rename = "id_token")]
    id: String,
    #[serde(rename = "access_token")]
    access: String,
    #[serde(rename = "refresh_token")]
    refresh: String,
}

#[derive(Deserialize)]
struct RefreshResponse {
    #[serde(rename = "id_token")]
    id: Option<String>,
    #[serde(rename = "access_token")]
    access: Option<String>,
    #[serde(rename = "refresh_token")]
    refresh: Option<String>,
}

#[derive(Serialize)]
struct RefreshRequest<'a> {
    client_id: &'a str,
    grant_type: &'a str,
    refresh_token: String,
}

fn decode_jwt<T: DeserializeOwned>(jwt: &str) -> Result<T, ChatGptAuthError> {
    let payload = jwt
        .split('.')
        .nth(1)
        .filter(|payload| !payload.is_empty())
        .ok_or_else(|| ChatGptAuthError::InvalidToken("invalid JWT format".into()))?;
    let decoded = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|error| ChatGptAuthError::InvalidToken(error.to_string()))?;
    serde_json::from_slice(&decoded)
        .map_err(|error| ChatGptAuthError::InvalidToken(error.to_string()))
}

fn jwt_expiration(jwt: &str) -> Option<i64> {
    decode_jwt::<ExpClaims>(jwt).ok()?.exp
}

fn unix_now() -> i64 {
    nanocode_platform::system_time()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

fn rfc3339_utc(timestamp: i64) -> String {
    const SECONDS_PER_DAY: i64 = 86_400;

    let days = timestamp.div_euclid(SECONDS_PER_DAY);
    let seconds = timestamp.rem_euclid(SECONDS_PER_DAY);
    let hour = seconds / 3_600;
    let minute = (seconds % 3_600) / 60;
    let second = seconds % 60;

    let shifted_days = days + 719_468;
    let era = if shifted_days >= 0 {
        shifted_days
    } else {
        shifted_days - 146_096
    } / 146_097;
    let day_of_era = shifted_days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    year += i64::from(month <= 2);

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn read_stored_auth(
    path: &Path,
) -> Result<(StoredCredentials, CodexAuthDocument), ChatGptAuthError> {
    let document = read_document(path)?;
    let credentials = StoredCredentials::from_document(&document, path)?;
    Ok((credentials, document))
}

fn read_store(path: &Path) -> Result<StoredCredentials, ChatGptAuthError> {
    read_stored_auth(path).map(|(credentials, _)| credentials)
}

fn read_document(path: &Path) -> Result<CodexAuthDocument, ChatGptAuthError> {
    let bytes = std::fs::read(path).map_err(|source| ChatGptAuthError::Storage {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_slice(&bytes).map_err(|error| ChatGptAuthError::InvalidStore {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })
}

async fn run_blocking<T: Send + 'static>(
    path: PathBuf,
    task: impl FnOnce(&Path) -> Result<T, ChatGptAuthError> + Send + 'static,
) -> Result<T, ChatGptAuthError> {
    let task_path = path.clone();
    tokio::task::spawn_blocking(move || task(&task_path))
        .await
        .map_err(|source| ChatGptAuthError::Storage {
            path,
            source: io::Error::other(source),
        })?
}

async fn read_stored_auth_locked(
    lock: File,
    path: PathBuf,
) -> Result<(File, (StoredCredentials, CodexAuthDocument)), ChatGptAuthError> {
    run_blocking(path, move |path| Ok((lock, read_stored_auth(path)?))).await
}

fn lock_auth_file(path: &Path) -> Result<File, ChatGptAuthError> {
    let file = open_auth_lock_file(path)?;
    file.lock().map_err(|source| ChatGptAuthError::Storage {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(file)
}

fn open_auth_lock_file(path: &Path) -> Result<File, ChatGptAuthError> {
    let parent = path.parent().ok_or_else(|| ChatGptAuthError::Storage {
        path: path.to_path_buf(),
        source: io::Error::new(ErrorKind::InvalidInput, "auth file has no parent directory"),
    })?;
    std::fs::create_dir_all(parent).map_err(|source| ChatGptAuthError::Storage {
        path: parent.to_path_buf(),
        source,
    })?;
    let mut lock_path = path.as_os_str().to_os_string();
    lock_path.push(".lock");
    let lock_path = PathBuf::from(lock_path);
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(&lock_path)
        .map_err(|source| ChatGptAuthError::Storage {
            path: lock_path.clone(),
            source,
        })?;
    Ok(file)
}

fn write_store(
    path: &Path,
    credentials: &StoredCredentials,
) -> Result<CodexAuthDocument, ChatGptAuthError> {
    let parent = path.parent().ok_or_else(|| ChatGptAuthError::Storage {
        path: path.to_path_buf(),
        source: io::Error::new(ErrorKind::InvalidInput, "auth file has no parent directory"),
    })?;
    std::fs::create_dir_all(parent).map_err(|source| ChatGptAuthError::Storage {
        path: parent.to_path_buf(),
        source,
    })?;
    let mut document = match read_document(path) {
        Ok(document) => document,
        Err(ChatGptAuthError::Storage { source, .. }) if source.kind() == ErrorKind::NotFound => {
            CodexAuthDocument::default()
        }
        Err(error) => return Err(error),
    };
    let token_extra = document
        .tokens
        .take()
        .map_or_else(BTreeMap::new, |tokens| tokens.extra);
    document.auth_mode = Some("chatgpt".into());
    document.tokens = Some(CodexTokenData {
        id_token: credentials.id_token.clone(),
        access_token: credentials.access_token.clone(),
        refresh_token: credentials.refresh_token.clone(),
        account_id: Some(credentials.account_id.clone()),
        extra: token_extra,
    });
    document.last_refresh = Some(rfc3339_utc(unix_now()));

    let temporary = path.with_extension(format!("json.{}.tmp", random_urlsafe()?));
    let bytes =
        serde_json::to_vec_pretty(&document).map_err(|error| ChatGptAuthError::InvalidStore {
            path: path.to_path_buf(),
            detail: error.to_string(),
        })?;
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|source| ChatGptAuthError::Storage {
            path: temporary.clone(),
            source,
        })?;
    if let Err(source) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(&temporary);
        return Err(ChatGptAuthError::Storage {
            path: temporary,
            source,
        });
    }
    drop(file);
    if let Err(source) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(ChatGptAuthError::Storage {
            path: path.to_path_buf(),
            source,
        });
    }
    Ok(document)
}

fn auth_store_error(error: &ChatGptAuthError) -> OpenAiAuthError {
    OpenAiAuthError::Unavailable(Arc::from(error.to_string()))
}

fn auth_client() -> Result<nanocode_http::Client, ChatGptAuthError> {
    crate::client::transport::install_default_rustls_crypto_provider();
    nanocode_http::Client::builder()
        .timeout(AUTH_REQUEST_TIMEOUT)
        .default_headers(nanocode_http::accept_encoding(
            nanocode_http::header::HeaderMap::new(),
        ))
        .build()
        .map_err(|error| ChatGptAuthError::TokenExchange(error.to_string()))
}

fn refresh_error_code(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    value
        .get("error")
        .and_then(|error| match error {
            serde_json::Value::String(code) => Some(code.as_str()),
            serde_json::Value::Object(error) => error.get("code")?.as_str(),
            _ => None,
        })
        .or_else(|| value.get("code")?.as_str())
        .map(str::to_owned)
}

async fn bind_callback() -> Result<TcpListener, ChatGptAuthError> {
    for port in CALLBACK_PORTS {
        if let Ok(listener) = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await {
            return Ok(listener);
        }
    }
    Err(ChatGptAuthError::CallbackUnavailable)
}

async fn receive_callback(listener: &TcpListener) -> Result<OAuthCallback, ChatGptAuthError> {
    let (mut stream, _) = listener
        .accept()
        .await
        .map_err(|error| ChatGptAuthError::LoginRejected(error.to_string()))?;
    let mut bytes = Vec::with_capacity(2048);
    loop {
        let read = stream
            .read_buf(&mut bytes)
            .await
            .map_err(|error| ChatGptAuthError::LoginRejected(error.to_string()))?;
        if read == 0 || bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if bytes.len() > 16 * 1024 {
            return Err(ChatGptAuthError::LoginRejected(
                "OAuth callback request was too large".into(),
            ));
        }
    }
    let request = std::str::from_utf8(&bytes)
        .map_err(|error| ChatGptAuthError::LoginRejected(error.to_string()))?;
    let target = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .filter(|target| target.starts_with(CALLBACK_PATH))
        .ok_or_else(|| ChatGptAuthError::LoginRejected("invalid callback request".into()))?;
    Ok(OAuthCallback {
        target: target.to_owned(),
        stream,
    })
}

fn authorize_url(
    issuer: &str,
    redirect_uri: &str,
    state: &str,
    challenge: &str,
) -> Result<String, ChatGptAuthError> {
    let mut url = Url::parse(&format!("{}/oauth/authorize", issuer.trim_end_matches('/')))
        .map_err(|error| ChatGptAuthError::LoginRejected(error.to_string()))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", OAUTH_CLIENT_ID)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", OAUTH_SCOPE)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true")
        .append_pair("state", state)
        .append_pair("originator", "nanocode");
    Ok(url.into())
}

async fn exchange_code(
    client: &nanocode_http::Client,
    issuer: &str,
    code: &str,
    redirect_uri: &str,
    code_verifier: &str,
) -> Result<StoredCredentials, ChatGptAuthError> {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "authorization_code")
        .append_pair("code", code)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("client_id", OAUTH_CLIENT_ID)
        .append_pair("code_verifier", code_verifier)
        .finish();
    let response = client
        .post(format!("{}/oauth/token", issuer.trim_end_matches('/')))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map(nanocode_http::decode_flate_response)
        .map_err(|error| ChatGptAuthError::TokenExchange(error.to_string()))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|error| ChatGptAuthError::TokenExchange(error.to_string()))?;
    if !status.is_success() {
        let code = refresh_error_code(&bytes)
            .unwrap_or_else(|| format!("token endpoint returned HTTP {status}"));
        return Err(ChatGptAuthError::TokenExchange(code));
    }
    let tokens: TokenResponse = serde_json::from_slice(&bytes)
        .map_err(|error| ChatGptAuthError::TokenExchange(error.to_string()))?;
    StoredCredentials::from_tokens(tokens)
}

fn random_urlsafe() -> Result<String, ChatGptAuthError> {
    let mut bytes = [0_u8; 32];
    nanocode_platform::fill_random(&mut bytes)
        .map_err(|error| ChatGptAuthError::LoginRejected(error.to_string()))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
