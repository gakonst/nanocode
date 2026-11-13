use std::{fmt, sync::Arc};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use http::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use nanocode_digest::Sha256;
use nanocode_platform::UNIX_EPOCH;
use nanocode_sync::SingleFlight;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{ClaudeAuthFuture, ClaudeAuthProvider, ClaudeAuthUnavailable};

pub const CLAUDE_OAUTH_BETA: &str = "oauth-2025-04-20";
const DEFAULT_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const MAX_BYTES: usize = 64 * 1024;
const LOGIN_TTL: u64 = 15 * 60 * 1000;
const RETRIES: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaudeSubscriptionError {
    Host,
    Invalid,
    LoginRequired,
    ExchangeUncertain,
    ProviderRejected,
    AccountOnHold,
    Conflict,
}

impl std::fmt::Display for ClaudeSubscriptionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Host => formatter.write_str("Claude subscription host capability failed"),
            Self::Invalid => formatter.write_str("invalid Claude OAuth configuration, state, or response"),
            Self::LoginRequired => formatter.write_str("Claude subscription login is required"),
            Self::ExchangeUncertain => formatter.write_str("Claude OAuth token exchange is in progress or uncertain; start a new login to recover"),
            Self::ProviderRejected => formatter.write_str("Claude OAuth provider rejected the exchange"),
            Self::AccountOnHold => formatter.write_str("Claude account is on hold"),
            Self::Conflict => formatter.write_str("Claude subscription state changed concurrently"),
        }
    }
}

impl std::error::Error for ClaudeSubscriptionError {}

type Result<T> = std::result::Result<T, ClaudeSubscriptionError>;

#[derive(Clone, Copy, Debug)]
pub struct ClaudeSubscriptionHostError;

impl std::fmt::Display for ClaudeSubscriptionHostError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Claude subscription host failed")
    }
}

impl std::error::Error for ClaudeSubscriptionHostError {}

#[derive(Clone, Default)]
pub struct ClaudeSubscriptionStoreValue {
    pub revision: u64,
    pub payload: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaudeSubscriptionCommit {
    Committed(u64),
    Conflict(u64),
}

pub struct ClaudeSubscriptionHttpRequest {
    method: &'static str,
    url: String,
    headers: HeaderMap,
    body: String,
    timeout_millis: u64,
}
impl ClaudeSubscriptionHttpRequest {
    pub const fn method(&self) -> &'static str {
        self.method
    }
    pub fn url(&self) -> &str {
        &self.url
    }
    pub const fn headers(&self) -> &HeaderMap {
        &self.headers
    }
    pub const fn content_type(&self) -> &'static str {
        "application/json"
    }
    pub fn body(&self) -> &str {
        &self.body
    }
    pub const fn max_response_bytes(&self) -> usize {
        MAX_BYTES
    }
    pub const fn timeout_millis(&self) -> u64 {
        self.timeout_millis
    }
}

pub struct ClaudeSubscriptionHttpResponse {
    pub status: u16,
    pub body: String,
}

#[cfg(not(target_family = "wasm"))]
impl ClaudeSubscriptionHttpRequest {
    pub async fn send(
        &self,
        client: &nanocode_http::Client,
    ) -> std::result::Result<ClaudeSubscriptionHttpResponse, nanocode_http::BoundedError> {
        let method = nanocode_http::Method::from_bytes(self.method.as_bytes())
            .map_err(nanocode_http::BoundedError::Method)?;
        let request = client
            .request(method, &self.url)
            .headers(self.headers.clone())
            .header("content-type", self.content_type())
            .body(self.body.clone())
            .timeout(std::time::Duration::from_millis(self.timeout_millis));
        let (status, body) = nanocode_http::bounded_text(request, MAX_BYTES).await?;
        Ok(ClaudeSubscriptionHttpResponse { status, body })
    }
}

macro_rules! redacted_debug {
    ($($kind:ty),+ $(,)?) => {$(impl fmt::Debug for $kind {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(concat!(stringify!($kind), "([redacted])"))
        }
    })+};
}
redacted_debug!(
    ClaudeSubscriptionStoreValue,
    ClaudeSubscriptionHttpRequest,
    ClaudeSubscriptionHttpResponse
);

pub trait ClaudeSubscriptionHost: Send + Sync + 'static {
    fn load<'a>(
        &'a self,
        key: &'a str,
    ) -> ClaudeAuthFuture<
        'a,
        std::result::Result<ClaudeSubscriptionStoreValue, ClaudeSubscriptionHostError>,
    >;

    fn compare_and_swap<'a>(
        &'a self,
        key: &'a str,
        expected_revision: u64,
        payload: &'a str,
    ) -> ClaudeAuthFuture<
        'a,
        std::result::Result<ClaudeSubscriptionCommit, ClaudeSubscriptionHostError>,
    >;
    fn request(
        &self,
        request: ClaudeSubscriptionHttpRequest,
    ) -> ClaudeAuthFuture<
        '_,
        std::result::Result<ClaudeSubscriptionHttpResponse, ClaudeSubscriptionHostError>,
    >;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClaudeSubscriptionConfig {
    pub authorize_url: String,
    pub token_url: String,
    pub profile_url: String,
    pub manual_redirect_uri: String,
    pub client_id: String,
    pub scopes: Vec<String>,
    pub refresh_margin_millis: u64,
    pub login_ttl_millis: u64,
    pub allow_loopback_http: bool,
}
impl Default for ClaudeSubscriptionConfig {
    fn default() -> Self {
        Self {
            authorize_url: "https://claude.com/cai/oauth/authorize".into(),
            token_url: "https://platform.claude.com/v1/oauth/token".into(),
            profile_url: "https://api.anthropic.com/api/oauth/profile".into(),
            manual_redirect_uri: "https://platform.claude.com/oauth/code/callback".into(),
            client_id: DEFAULT_CLIENT_ID.into(),
            scopes: "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload user:plugins".split_whitespace().map(str::to_owned).collect(),
            refresh_margin_millis: 5 * 60 * 1000,
            login_ttl_millis: LOGIN_TTL,
            allow_loopback_http: false,
        }
    }
}

#[derive(Clone, Debug)]
pub enum ClaudeLoginMode {
    Manual,

    Callback { redirect_uri: String },
}

#[derive(Clone, Serialize)]
pub struct ClaudeLogin {
    pub authorization_url: String,
    pub expires_at: u64,
}
redacted_debug!(ClaudeLogin);

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ClaudeSubscriptionStatus {
    SignedOut,
    AccountOnHold,
    Pending {
        expires_at: u64,
    },
    Expired,
    ExchangeUncertain,
    Validating,
    Authenticated {
        account_id: String,
        organization_id: String,
        expires_at: u64,
    },
}

#[derive(Clone)]
pub struct ClaudeSubscription {
    inner: Arc<Inner>,
}
struct Inner {
    host: Arc<dyn ClaudeSubscriptionHost>,
    key: String,
    config: ClaudeSubscriptionConfig,
    binding: String,
    resolving: SingleFlight<Result<HeaderMap>>,
}
redacted_debug!(ClaudeSubscription);

#[derive(Serialize, Deserialize)]
struct Envelope {
    version: u8,
    binding: String,
    state: State,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind")]
enum State {
    SignedOut,
    AccountOnHold,
    Pending {
        verifier: String,
        state: String,
        redirect: String,
        expires_at: u64,
    },
    Exchanging,
    Validating {
        token: Token,
        prior: Option<Identity>,
    },
    Ready {
        token: Token,
        identity: Identity,
    },
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Identity {
    account: String,
    organization: String,
}
#[derive(Serialize, Deserialize)]
struct Token {
    access: String,
    refresh: String,
    expires_at: u64,
    scopes: Vec<String>,
    refresh_expires_at: Option<u64>,
    expected_account: Option<String>,
    expected_organization: Option<String>,
}
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: u64,
    refresh_token_expires_in: Option<u64>,
    scope: String,
    token_type: Option<String>,
    account: Option<UuidField>,
    organization: Option<UuidField>,
}
#[derive(Deserialize)]
struct UuidField {
    uuid: String,
}
#[derive(Deserialize)]
struct Profile {
    account: UuidField,
    organization: UuidField,
}

impl ClaudeSubscription {
    pub fn new(
        host: Arc<dyn ClaudeSubscriptionHost>,
        key: impl Into<String>,
        config: ClaudeSubscriptionConfig,
    ) -> Result<Self> {
        for endpoint in [
            &config.authorize_url,
            &config.token_url,
            &config.profile_url,
            &config.manual_redirect_uri,
        ] {
            validate_url(endpoint, config.allow_loopback_http)?;
        }
        if config.client_id.is_empty()
            || config.scopes.is_empty()
            || config
                .scopes
                .iter()
                .any(|s| s.is_empty() || s.chars().any(char::is_whitespace))
        {
            return Err(ClaudeSubscriptionError::Invalid);
        }
        let key = key.into();
        if key.is_empty() || config.login_ttl_millis == 0 || config.login_ttl_millis > LOGIN_TTL {
            return Err(ClaudeSubscriptionError::Invalid);
        }
        let binding = URL_SAFE_NO_PAD.encode(Sha256::digest(
            &serde_json::to_vec(&config).map_err(|_| ClaudeSubscriptionError::Invalid)?,
        ));
        Ok(Self {
            inner: Arc::new(Inner {
                host,
                key,
                config,
                binding,
                resolving: SingleFlight::default(),
            }),
        })
    }

    async fn load(&self) -> Result<(u64, State)> {
        let stored = self
            .inner
            .host
            .load(&self.inner.key)
            .await
            .map_err(|_| ClaudeSubscriptionError::Host)?;
        let state = match stored.payload {
            None => State::SignedOut,
            Some(payload) => {
                if payload.len() > MAX_BYTES {
                    return Err(ClaudeSubscriptionError::Invalid);
                }
                let envelope: Envelope =
                    serde_json::from_str(&payload).map_err(|_| ClaudeSubscriptionError::Invalid)?;
                if envelope.version != 1 || envelope.binding != self.inner.binding {
                    return Err(ClaudeSubscriptionError::Invalid);
                }
                envelope.state
            }
        };
        Ok((stored.revision, state))
    }
    async fn save(&self, revision: u64, state: State) -> Result<u64> {
        let payload = serde_json::to_string(&Envelope {
            version: 1,
            binding: self.inner.binding.clone(),
            state,
        })
        .map_err(|_| ClaudeSubscriptionError::Invalid)?;
        if payload.len() > MAX_BYTES {
            return Err(ClaudeSubscriptionError::Invalid);
        }
        match self
            .inner
            .host
            .compare_and_swap(&self.inner.key, revision, &payload)
            .await
            .map_err(|_| ClaudeSubscriptionError::Host)?
        {
            ClaudeSubscriptionCommit::Committed(next) if next > revision => Ok(next),
            ClaudeSubscriptionCommit::Committed(_) => Err(ClaudeSubscriptionError::Invalid),
            ClaudeSubscriptionCommit::Conflict(_) => Err(ClaudeSubscriptionError::Conflict),
        }
    }

    pub async fn begin_login(&self, mode: ClaudeLoginMode) -> Result<ClaudeLogin> {
        let redirect = match mode {
            ClaudeLoginMode::Manual => self.inner.config.manual_redirect_uri.clone(),
            ClaudeLoginMode::Callback { redirect_uri } => {
                validate_url(&redirect_uri, true)?;
                redirect_uri
            }
        };
        let verifier = random_secret()?;
        let state = random_secret()?;
        let expires_at = now().saturating_add(self.inner.config.login_ttl_millis);
        let mut authorization = Url::parse(&self.inner.config.authorize_url)
            .map_err(|_| ClaudeSubscriptionError::Invalid)?;
        authorization.query_pairs_mut().extend_pairs([
            ("code", "true"),
            ("client_id", &self.inner.config.client_id),
            ("response_type", "code"),
            ("redirect_uri", &redirect),
            ("scope", &self.inner.config.scopes.join(" ")),
            (
                "code_challenge",
                &URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
            ),
            ("code_challenge_method", "S256"),
            ("state", &state),
        ]);
        for _ in 0..RETRIES {
            let (revision, _) = self.load().await?;
            match self
                .save(
                    revision,
                    State::Pending {
                        verifier: verifier.clone(),
                        state: state.clone(),
                        redirect: redirect.clone(),
                        expires_at,
                    },
                )
                .await
            {
                Ok(_) => {
                    return Ok(ClaudeLogin {
                        authorization_url: authorization.into(),
                        expires_at,
                    });
                }
                Err(ClaudeSubscriptionError::Conflict) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(ClaudeSubscriptionError::Conflict)
    }

    pub async fn complete_login(&self, callback_or_code: &str) -> Result<ClaudeSubscriptionStatus> {
        let (revision, state) = self.load().await?;
        match state {
            State::Validating { token, prior } => {
                self.resume_validation(revision, token, prior).await?;
            }
            State::Pending {
                verifier,
                state,
                redirect,
                expires_at,
            } => {
                if now() >= expires_at {
                    return Err(ClaudeSubscriptionError::LoginRequired);
                }
                let code = parse_callback(callback_or_code, &redirect, &state)?;
                let claim = self.save(revision, State::Exchanging).await?;
                let body = serde_json::json!({"grant_type":"authorization_code", "code":code, "redirect_uri":redirect, "client_id":self.inner.config.client_id, "code_verifier":verifier, "state":state});
                self.exchange(claim, body, None, None).await?;
            }
            State::Exchanging => return Err(ClaudeSubscriptionError::ExchangeUncertain),
            _ => return Err(ClaudeSubscriptionError::LoginRequired),
        }
        self.status().await
    }

    pub async fn status(&self) -> Result<ClaudeSubscriptionStatus> {
        Ok(match self.load().await?.1 {
            State::SignedOut => ClaudeSubscriptionStatus::SignedOut,
            State::AccountOnHold => ClaudeSubscriptionStatus::AccountOnHold,
            State::Pending { expires_at, .. } if now() < expires_at => {
                ClaudeSubscriptionStatus::Pending { expires_at }
            }
            State::Pending { .. } => ClaudeSubscriptionStatus::Expired,
            State::Exchanging => ClaudeSubscriptionStatus::ExchangeUncertain,
            State::Validating { .. } => ClaudeSubscriptionStatus::Validating,
            State::Ready {
                token, identity, ..
            } => ClaudeSubscriptionStatus::Authenticated {
                account_id: identity.account,
                organization_id: identity.organization,
                expires_at: token.expires_at,
            },
        })
    }

    pub async fn logout(&self) -> Result<()> {
        for _ in 0..RETRIES {
            let value = self
                .inner
                .host
                .load(&self.inner.key)
                .await
                .map_err(|_| ClaudeSubscriptionError::Host)?;
            let refresh = value
                .payload
                .as_deref()
                .and_then(|payload| serde_json::from_str::<Envelope>(payload).ok())
                .filter(|envelope| envelope.binding == self.inner.binding && envelope.version == 1)
                .and_then(|envelope| match envelope.state {
                    State::Ready { token, .. } | State::Validating { token, .. } => {
                        Some(token.refresh)
                    }
                    _ => None,
                });
            match self.save(value.revision, State::SignedOut).await {
                Ok(_) => {
                    if let Some(refresh) = refresh {
                        let body = serde_json::json!({"token":refresh,"token_type_hint":"refresh_token","client_id":self.inner.config.client_id});
                        let mut headers = HeaderMap::new();
                        headers
                            .insert("content-type", HeaderValue::from_static("application/json"));
                        let url = format!(
                            "{}/revoke",
                            self.inner.config.token_url.trim_end_matches('/')
                        );
                        let _ = self
                            .request("POST", &url, headers, body.to_string(), 5_000)
                            .await;
                    }
                    return Ok(());
                }
                Err(ClaudeSubscriptionError::Conflict) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(ClaudeSubscriptionError::Conflict)
    }

    async fn request(
        &self,
        method: &'static str,
        url: &str,
        headers: HeaderMap,
        body: String,
        timeout_millis: u64,
    ) -> Result<ClaudeSubscriptionHttpResponse> {
        let response = self
            .inner
            .host
            .request(ClaudeSubscriptionHttpRequest {
                method,
                url: url.into(),
                headers,
                body,
                timeout_millis,
            })
            .await
            .map_err(|_| ClaudeSubscriptionError::Host)?;
        if response.body.len() > MAX_BYTES {
            return Err(ClaudeSubscriptionError::Invalid);
        }
        Ok(response)
    }

    async fn exchange(
        &self,
        mut claim: u64,
        mut body: serde_json::Value,
        old_token: Option<Token>,
        prior: Option<Identity>,
    ) -> Result<()> {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));

        let mut response = self
            .request(
                "POST",
                &self.inner.config.token_url,
                headers.clone(),
                body.to_string(),
                30_000,
            )
            .await?;

        if response.status == 400
            && let Some(old_token) = &old_token
            && provider_error(&response.body).as_deref() == Some("invalid_scope")
        {
            claim = self.save(claim, State::Exchanging).await?;
            body["scope"] = serde_json::Value::String(old_token.scopes.join(" "));
            response = self
                .request(
                    "POST",
                    &self.inner.config.token_url,
                    headers,
                    body.to_string(),
                    30_000,
                )
                .await?;
        }
        if response.status != 200 {
            let error = provider_error(&response.body);
            let on_hold = serde_json::from_str::<serde_json::Value>(&response.body)
                .ok()
                .is_some_and(|value| {
                    matches!(
                        value["error"].as_str(),
                        Some("invalid_grant" | "access_denied")
                    ) && value["error_description"] == "account_on_hold"
                });
            if matches!(response.status, 400 | 401 | 403) && on_hold {
                self.save(claim, State::AccountOnHold).await?;
                return Err(ClaudeSubscriptionError::AccountOnHold);
            }
            if matches!(response.status, 400 | 401) && error.as_deref() == Some("invalid_grant") {
                self.save(claim, State::SignedOut).await?;
                return Err(ClaudeSubscriptionError::LoginRequired);
            }
            return Err(ClaudeSubscriptionError::ProviderRejected);
        }
        let response: TokenResponse =
            serde_json::from_str(&response.body).map_err(|_| ClaudeSubscriptionError::Invalid)?;
        let scopes: Vec<String> = response
            .scope
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        if response
            .token_type
            .as_deref()
            .is_some_and(|kind| !kind.eq_ignore_ascii_case("bearer"))
            || !["user:profile", "user:inference"]
                .iter()
                .all(|required| scopes.iter().any(|s| s == required))
        {
            return Err(ClaudeSubscriptionError::Invalid);
        }
        let refresh = response
            .refresh_token
            .or_else(|| old_token.as_ref().map(|token| token.refresh.clone()))
            .filter(|v| valid_secret(v))
            .ok_or(ClaudeSubscriptionError::Invalid)?;
        let refresh_expires_at = if let Some(seconds) = response.refresh_token_expires_in {
            Some(
                now()
                    .checked_add(
                        seconds
                            .checked_mul(1000)
                            .ok_or(ClaudeSubscriptionError::Invalid)?,
                    )
                    .ok_or(ClaudeSubscriptionError::Invalid)?,
            )
        } else if let Some(old) = &old_token {
            old.refresh_expires_at
        } else {
            Some(now().saturating_add(30 * 24 * 60 * 60 * 1000))
        };
        bearer(&response.access_token)?;
        let lifetime = response
            .expires_in
            .checked_mul(1000)
            .ok_or(ClaudeSubscriptionError::Invalid)?;
        if lifetime <= self.inner.config.refresh_margin_millis {
            return Err(ClaudeSubscriptionError::Invalid);
        }
        let expected_account = continuity_assertion(
            response.account.map(|value| value.uuid),
            old_token
                .as_ref()
                .and_then(|token| token.expected_account.as_ref()),
        )?;
        let expected_organization = continuity_assertion(
            response.organization.map(|value| value.uuid),
            old_token
                .as_ref()
                .and_then(|token| token.expected_organization.as_ref()),
        )?;
        let token = Token {
            access: response.access_token,
            refresh,
            expires_at: now()
                .checked_add(lifetime)
                .ok_or(ClaudeSubscriptionError::Invalid)?,
            scopes,
            refresh_expires_at,
            expected_account,
            expected_organization,
        };

        let next = self.save(claim, State::Validating { token, prior }).await?;
        let (revision, state) = self.load().await?;
        if revision != next {
            return Err(ClaudeSubscriptionError::Conflict);
        }
        if let State::Validating { token, prior } = state {
            self.validate_profile(revision, token, prior).await
        } else {
            Err(ClaudeSubscriptionError::Conflict)
        }
    }

    async fn validate_profile(
        &self,
        revision: u64,
        mut token: Token,
        prior: Option<Identity>,
    ) -> Result<()> {
        let mut headers = token_headers(&token)?;
        headers.remove("anthropic-beta");
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        headers.insert("cache-control", HeaderValue::from_static("no-cache"));
        let response = self
            .request(
                "GET",
                &self.inner.config.profile_url,
                headers,
                String::new(),
                10_000,
            )
            .await?;
        if response.status != 200 {
            if response.status == 401 {
                token.expires_at = 0;
                self.save(revision, State::Validating { token, prior })
                    .await?;
            }
            return Err(ClaudeSubscriptionError::ProviderRejected);
        }
        let profile: Profile =
            serde_json::from_str(&response.body).map_err(|_| ClaudeSubscriptionError::Invalid)?;
        let identity = Identity {
            account: profile.account.uuid,
            organization: profile.organization.uuid,
        };
        if !valid_identity(&identity.account)
            || !valid_identity(&identity.organization)
            || prior.as_ref().is_some_and(|old| old != &identity)
            || token
                .expected_account
                .as_ref()
                .is_some_and(|v| v != &identity.account)
            || token
                .expected_organization
                .as_ref()
                .is_some_and(|v| v != &identity.organization)
        {
            self.save(revision, State::SignedOut).await?;
            return Err(ClaudeSubscriptionError::Invalid);
        }
        self.save(revision, State::Ready { token, identity })
            .await?;
        Ok(())
    }

    async fn resume_validation(
        &self,
        revision: u64,
        token: Token,
        prior: Option<Identity>,
    ) -> Result<()> {
        if token.expires_at.saturating_sub(now()) <= self.inner.config.refresh_margin_millis {
            self.refresh_token(revision, token, prior).await
        } else {
            self.validate_profile(revision, token, prior).await
        }
    }

    async fn refresh_token(
        &self,
        revision: u64,
        token: Token,
        prior: Option<Identity>,
    ) -> Result<()> {
        if token
            .refresh_expires_at
            .is_some_and(|expiry| now() >= expiry)
        {
            self.save(revision, State::SignedOut).await?;
            return Err(ClaudeSubscriptionError::LoginRequired);
        }
        let mut refresh_scopes = if self.inner.config.client_id == DEFAULT_CLIENT_ID {
            self.inner
                .config
                .scopes
                .iter()
                .filter(|s| s.as_str() != "org:create_api_key")
                .cloned()
                .collect::<Vec<_>>()
        } else {
            token.scopes.clone()
        };
        for scope in &token.scopes {
            if matches!(scope.as_str(), "user:projects:read" | "user:projects:write")
                && !refresh_scopes.contains(scope)
            {
                refresh_scopes.push(scope.clone());
            }
        }
        let body = serde_json::json!({"grant_type":"refresh_token", "refresh_token":token.refresh, "client_id":self.inner.config.client_id, "scope":refresh_scopes.join(" ")});
        let claim = self.save(revision, State::Exchanging).await?;
        self.exchange(claim, body, Some(token), prior).await
    }

    async fn resolve_headers(&self) -> Result<HeaderMap> {
        let subscription = self.clone();
        self.inner
            .resolving
            .run(|| async move { subscription.resolve_loaded(None).await })
            .await
    }

    #[cfg(not(target_family = "wasm"))]
    pub(crate) async fn ready_headers(&self) -> Option<Result<HeaderMap>> {
        match self.load().await {
            Ok((_, State::Ready { .. })) => Some(self.resolve_headers().await),
            _ => None,
        }
    }

    async fn resolve_loaded(&self, mut loaded: Option<(u64, State)>) -> Result<HeaderMap> {
        for _ in 0..RETRIES {
            let (revision, state) = match loaded.take() {
                Some(loaded) => loaded,
                None => self.load().await?,
            };
            match state {
                State::Ready { token, identity } => {
                    if token.expires_at.saturating_sub(now())
                        > self.inner.config.refresh_margin_millis
                    {
                        return token_headers(&token);
                    }
                    match self.refresh_token(revision, token, Some(identity)).await {
                        Err(ClaudeSubscriptionError::Conflict) => continue,
                        result => result?,
                    }
                }
                State::Validating { token, prior } => {
                    self.resume_validation(revision, token, prior).await?;
                }
                State::Exchanging => return Err(ClaudeSubscriptionError::ExchangeUncertain),
                State::AccountOnHold => return Err(ClaudeSubscriptionError::AccountOnHold),
                _ => return Err(ClaudeSubscriptionError::LoginRequired),
            }
        }
        Err(ClaudeSubscriptionError::Conflict)
    }

    async fn reject(&self, rejected: &HeaderMap) -> Result<bool> {
        for _ in 0..RETRIES {
            let (revision, state) = self.load().await?;
            let State::Ready {
                mut token,
                identity,
            } = state
            else {
                return Ok(false);
            };
            if rejected.get(AUTHORIZATION) != Some(&bearer(&token.access)?) {
                return Ok(true);
            }

            if token.expires_at == 0 {
                return Ok(true);
            }
            token.expires_at = 0;
            let replacement = State::Ready { token, identity };
            match self.save(revision, replacement).await {
                Ok(_) => return Ok(true),
                Err(ClaudeSubscriptionError::Conflict) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(ClaudeSubscriptionError::Conflict)
    }
}

pub struct ClaudeSettling {
    subscription: ClaudeSubscription,
    wait: std::time::Duration,
    poll: std::time::Duration,
    waited: std::sync::atomic::AtomicBool,
}

impl ClaudeSubscription {
    #[must_use]
    pub const fn settling(
        self,
        wait: std::time::Duration,
        poll: std::time::Duration,
    ) -> ClaudeSettling {
        ClaudeSettling {
            subscription: self,
            wait,
            poll,
            waited: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

impl ClaudeAuthProvider for ClaudeSettling {
    fn headers(
        &self,
    ) -> ClaudeAuthFuture<'_, std::result::Result<HeaderMap, ClaudeAuthUnavailable>> {
        Box::pin(async move {
            let error = match self.subscription.headers().await {
                Ok(headers) => return Ok(headers),
                Err(error) => error,
            };
            if self.waited.swap(true, std::sync::atomic::Ordering::SeqCst) {
                return Err(error);
            }
            let deadline = nanocode_platform::system_time() + self.wait;
            while nanocode_platform::system_time() < deadline {
                nanocode_platform::sleep(self.poll).await;
                match self.subscription.status().await {
                    Ok(ClaudeSubscriptionStatus::Authenticated { .. }) => {
                        if let Ok(headers) = self.subscription.headers().await {
                            return Ok(headers);
                        }
                    }
                    Ok(
                        ClaudeSubscriptionStatus::ExchangeUncertain
                        | ClaudeSubscriptionStatus::Validating,
                    )
                    | Err(_) => {}
                    Ok(_) => break,
                }
            }
            Err(error)
        })
    }

    fn recover_unauthorized<'a>(
        &'a self,
        rejected: &'a HeaderMap,
    ) -> ClaudeAuthFuture<'a, std::result::Result<bool, ClaudeAuthUnavailable>> {
        self.subscription.recover_unauthorized(rejected)
    }
}

impl ClaudeAuthProvider for ClaudeSubscription {
    fn headers(
        &self,
    ) -> ClaudeAuthFuture<'_, std::result::Result<HeaderMap, ClaudeAuthUnavailable>> {
        Box::pin(async move {
            self.resolve_headers()
                .await
                .map_err(|error| ClaudeAuthUnavailable::new(error.to_string()))
        })
    }
    fn recover_unauthorized<'a>(
        &'a self,
        rejected: &'a HeaderMap,
    ) -> ClaudeAuthFuture<'a, std::result::Result<bool, ClaudeAuthUnavailable>> {
        Box::pin(async move {
            self.reject(rejected)
                .await
                .map_err(|error| ClaudeAuthUnavailable::new(error.to_string()))
        })
    }
}

pub(crate) fn now() -> u64 {
    nanocode_platform::system_time()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}
fn random_secret() -> Result<String> {
    let mut bytes = [0; 32];
    nanocode_platform::fill_random(&mut bytes).map_err(|_| ClaudeSubscriptionError::Host)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
fn valid_secret(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 16 * 1024
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b))
}
fn valid_identity(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}
fn bearer(value: &str) -> Result<HeaderValue> {
    if !valid_secret(value) {
        return Err(ClaudeSubscriptionError::Invalid);
    }
    let mut header = HeaderValue::from_str(&format!("Bearer {value}"))
        .map_err(|_| ClaudeSubscriptionError::Invalid)?;
    header.set_sensitive(true);
    Ok(header)
}
fn token_headers(token: &Token) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, bearer(&token.access)?);
    headers.insert(
        "anthropic-beta",
        HeaderValue::from_static(CLAUDE_OAUTH_BETA),
    );
    Ok(headers)
}
fn validate_url(value: &str, loopback: bool) -> Result<Url> {
    let url = Url::parse(value).map_err(|_| ClaudeSubscriptionError::Invalid)?;
    let is_loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !(url.scheme() == "https" || (loopback && is_loopback && url.scheme() == "http"))
    {
        return Err(ClaudeSubscriptionError::Invalid);
    }
    Ok(url)
}
fn parse_callback(input: &str, redirect: &str, expected_state: &str) -> Result<String> {
    if input.len() > 16 * 1024 {
        return Err(ClaudeSubscriptionError::Invalid);
    }
    let (code, state) = if input.contains("://") {
        let callback = Url::parse(input).map_err(|_| ClaudeSubscriptionError::Invalid)?;
        let expected = Url::parse(redirect).map_err(|_| ClaudeSubscriptionError::Invalid)?;
        if callback.origin() != expected.origin()
            || callback.path() != expected.path()
            || callback.fragment().is_some()
            || !callback.username().is_empty()
            || callback.password().is_some()
        {
            return Err(ClaudeSubscriptionError::Invalid);
        }
        let pairs: Vec<_> = callback.query_pairs().collect();
        if pairs.iter().any(|(key, _)| key == "error")
            || pairs.iter().filter(|(key, _)| key == "code").count() != 1
            || pairs.iter().filter(|(key, _)| key == "state").count() != 1
        {
            return Err(ClaudeSubscriptionError::Invalid);
        }
        (
            pairs
                .iter()
                .find(|(k, _)| k == "code")
                .unwrap()
                .1
                .to_string(),
            pairs
                .iter()
                .find(|(k, _)| k == "state")
                .unwrap()
                .1
                .to_string(),
        )
    } else {
        let (code, state) = input
            .trim()
            .split_once('#')
            .ok_or(ClaudeSubscriptionError::Invalid)?;
        (code.to_owned(), state.to_owned())
    };
    if state != expected_state
        || code.is_empty()
        || code.len() > 8192
        || code.chars().any(char::is_control)
    {
        return Err(ClaudeSubscriptionError::Invalid);
    }
    Ok(code)
}

fn provider_error(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    value
        .get("error")?
        .as_str()
        .or_else(|| value["error"]["type"].as_str())
        .map(str::to_owned)
}

fn continuity_assertion(new: Option<String>, old: Option<&String>) -> Result<Option<String>> {
    if let (Some(new), Some(old)) = (&new, old)
        && new != old
    {
        return Err(ClaudeSubscriptionError::Invalid);
    }
    Ok(new.or_else(|| old.cloned()))
}
