use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use http::{HeaderName, HeaderValue};
use nanocode_mcp::oauth::{
    AuthorizationCallback, AuthorizationMetadata, AuthorizationUrl, ClientAuthentication,
    ClientRegistrationRequest, ClientRegistrationResponse, DISCOVERY_PROTOCOL_VERSION,
    PKCE_VERIFIER_BYTES, Pkce, ProtectedResourceMetadata, STATE_BYTES, TokenError, TokenRequest,
    TokenResponse, WwwAuthenticate,
};
use reqwest::{StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

use arc_swap::{ArcSwap, ArcSwapOption};
use tracing::{Instrument, info_span};

use crate::SecretSource;

mod refresh;

const LOGIN_TIMEOUT: Duration = Duration::from_mins(5);
const MAX_CALLBACK_BYTES: usize = 16 * 1024;
const MAX_OAUTH_HTTP_RESPONSE_BODY_BYTES: usize = 1024 * 1024;
const OAUTH_CLIENT_NAME: &str = "Nanocode";

type MetadataEntries = HashMap<(String, String), AuthorizationMetadata>;

#[derive(Default)]
pub struct OAuthMetadataCache {
    entries: ArcSwap<MetadataEntries>,
}

impl OAuthMetadataCache {
    async fn get(&self, server_name: &str, server_url: &str) -> Option<AuthorizationMetadata> {
        self.entries
            .load()
            .get(&(server_name.to_owned(), server_url.to_owned()))
            .cloned()
    }

    async fn insert(&self, server_name: &str, server_url: &str, metadata: AuthorizationMetadata) {
        let key = (server_name.to_owned(), server_url.to_owned());
        self.entries.rcu(|entries| {
            let mut entries = MetadataEntries::clone(entries);
            entries.insert(key.clone(), metadata.clone());
            entries
        });
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct McpOAuthCredentials {
    client_id: String,
    client_secret: Option<String>,
    secret_in_body: bool,
    access_token: String,
    refresh_token: Option<String>,
    issuer: Option<String>,
    expires_at_millis: Option<u64>,
    scopes: Vec<String>,
    token_endpoint: Option<String>,
    resource: Option<String>,
}

pub trait McpOAuthRefreshGuard: Send {}

impl<T: Send> McpOAuthRefreshGuard for T {}

impl McpOAuthCredentials {
    #[must_use]
    pub fn new(client_id: impl Into<String>, access_token: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret: None,
            secret_in_body: false,
            access_token: access_token.into(),
            refresh_token: None,
            issuer: None,
            expires_at_millis: None,
            scopes: Vec::new(),
            token_endpoint: None,
            resource: None,
        }
    }

    #[must_use]
    pub fn client(mut self, client: ClientAuthentication) -> Self {
        self.client_id = client.client_id;
        self.client_secret = client.client_secret;
        self.secret_in_body = client.secret_in_body;
        self
    }

    #[must_use]
    pub fn client_authentication(&self) -> ClientAuthentication {
        ClientAuthentication {
            client_id: self.client_id.clone(),
            client_secret: self.client_secret.clone(),
            secret_in_body: self.secret_in_body,
        }
    }

    #[must_use]
    pub fn refresh_token(mut self, refresh_token: impl Into<String>) -> Self {
        self.refresh_token = Some(refresh_token.into());
        self
    }

    #[must_use]
    pub fn issuer(mut self, issuer: impl Into<String>) -> Self {
        self.issuer = Some(issuer.into());
        self
    }

    #[must_use]
    pub const fn expires_at_millis(mut self, expires_at_millis: u64) -> Self {
        self.expires_at_millis = Some(expires_at_millis);
        self
    }

    #[must_use]
    pub fn scopes(mut self, scopes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.scopes = scopes.into_iter().map(Into::into).collect();
        self
    }

    #[must_use]
    pub fn token_endpoint(mut self, token_endpoint: impl Into<String>) -> Self {
        self.token_endpoint = Some(token_endpoint.into());
        self
    }

    #[must_use]
    pub fn resource(mut self, resource: impl Into<String>) -> Self {
        self.resource = Some(resource.into());
        self
    }

    #[must_use]
    pub fn stored_token_endpoint(&self) -> Option<&str> {
        self.token_endpoint.as_deref()
    }

    #[must_use]
    pub fn resource_indicator(&self) -> Option<&str> {
        self.resource.as_deref()
    }

    #[must_use]
    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    #[must_use]
    pub fn access_token(&self) -> &str {
        &self.access_token
    }

    #[must_use]
    pub fn refresh_token_value(&self) -> Option<&str> {
        self.refresh_token.as_deref()
    }

    #[must_use]
    pub fn authorization_issuer(&self) -> Option<&str> {
        self.issuer.as_deref()
    }

    #[must_use]
    pub const fn expires_at(&self) -> Option<u64> {
        self.expires_at_millis
    }

    #[must_use]
    pub fn granted_scopes(&self) -> &[String] {
        &self.scopes
    }

    fn from_token_response(
        client_id: String,
        response: &TokenResponse,
        issuer: Option<String>,
    ) -> Self {
        let expires_at_millis = response
            .expires_in
            .and_then(|expires_in| now_millis().checked_add(expires_in.checked_mul(1000)?));
        Self {
            client_id,
            client_secret: None,
            secret_in_body: false,
            access_token: response.access_token.clone(),
            refresh_token: response.refresh_token.clone(),
            issuer,
            expires_at_millis,
            scopes: response.scopes().unwrap_or_default(),
            token_endpoint: None,
            resource: None,
        }
    }
}

#[async_trait]
pub trait McpOAuthStore: Send + Sync {
    async fn load(
        &self,
        server_name: &str,
        server_url: &str,
    ) -> Result<Option<McpOAuthCredentials>, String>;

    async fn save(
        &self,
        server_name: &str,
        server_url: &str,
        credentials: &McpOAuthCredentials,
    ) -> Result<(), String>;

    async fn acquire_refresh_lock(
        &self,
        _server_name: &str,
        _server_url: &str,
    ) -> Result<Box<dyn McpOAuthRefreshGuard>, String> {
        Ok(Box::new(()))
    }

    async fn authorization_required(&self, _server_name: &str, _server_url: &str) {}

    fn refresh_key(&self) -> Option<String> {
        None
    }
}

pub struct OAuthRuntime {
    server_name: String,
    server_url: String,
    base_url: Option<Url>,
    http: reqwest::Client,
    store: Arc<dyn McpOAuthStore>,
    credentials: ArcSwapOption<McpOAuthCredentials>,
    settled: ArcSwapOption<String>,
}

impl OAuthRuntime {
    pub fn stored(
        server_name: impl Into<String>,
        server_url: impl Into<String>,
        http: reqwest::Client,
        store: Arc<dyn McpOAuthStore>,
        credentials: McpOAuthCredentials,
    ) -> Result<Arc<Self>, String> {
        if credentials.token_endpoint.is_none() {
            return Err("OAuth credentials do not name their token endpoint".to_owned());
        }
        Ok(Arc::new(Self {
            server_name: server_name.into(),
            server_url: server_url.into(),
            base_url: None,
            http,
            store,
            credentials: ArcSwapOption::from_pointee(credentials),
            settled: ArcSwapOption::empty(),
        }))
    }

    pub fn access_token<'call>(
        self: &'call Arc<Self>,
    ) -> futures_util::future::BoxFuture<'call, Result<String, String>> {
        Box::pin(async move {
            self.refresh_if_needed().await?;
            self.current_access_token().await
        })
    }

    async fn current_access_token(&self) -> Result<String, String> {
        self.credentials
            .load()
            .as_ref()
            .map(|credentials| credentials.access_token.clone())
            .ok_or_else(|| {
                format!(
                    "OAuth credentials for `{}` were removed; authorization required",
                    self.server_name
                )
            })
    }
}

pub struct OAuthTransport {
    pub runtime: Arc<OAuthRuntime>,
    pub metadata_cache_hit: bool,
}

fn credentials_for_manager(
    credentials: &McpOAuthCredentials,
    authorization_issuer: Option<&str>,
) -> McpOAuthCredentials {
    let mut staged = credentials.clone();
    if validate_refresh_token_issuer(credentials, authorization_issuer).is_err() {
        staged.refresh_token = None;
        staged.issuer = None;
    }
    staged
}

fn validate_refresh_token_issuer(
    credentials: &McpOAuthCredentials,
    authorization_issuer: Option<&str>,
) -> Result<(), String> {
    if credentials.refresh_token.is_none() {
        return Ok(());
    }
    let Some(stored_issuer) = credentials.issuer.as_deref() else {
        return Err("OAuth refresh credentials are missing an authorization server issuer; authorization required".to_owned());
    };
    let Some(authorization_issuer) = authorization_issuer else {
        return Err(
            "OAuth metadata did not include an authorization server issuer; authorization required"
                .to_owned(),
        );
    };
    if stored_issuer != authorization_issuer {
        return Err("OAuth authorization server issuer changed; authorization required".to_owned());
    }
    Ok(())
}

fn authorization_issuer(metadata: &AuthorizationMetadata) -> Result<Option<String>, String> {
    metadata
        .issuer
        .as_deref()
        .filter(|issuer| !issuer.trim().is_empty())
        .map(|issuer| {
            url::Url::parse(issuer).map_err(|error| {
                format!("OAuth authorization server issuer is invalid: {error}")
            })?;
            Ok(issuer.to_owned())
        })
        .transpose()
}

fn validate_authorization_server_endpoints(metadata: &AuthorizationMetadata) -> Result<(), String> {
    let authorization_endpoint = url::Url::parse(&metadata.authorization_endpoint)
        .map_err(|error| format!("OAuth authorization endpoint is invalid: {error}"))?;
    let token_endpoint = url::Url::parse(&metadata.token_endpoint)
        .map_err(|error| format!("OAuth token endpoint is invalid: {error}"))?;
    let issuer = metadata
        .issuer
        .as_deref()
        .filter(|issuer| !issuer.trim().is_empty())
        .map(url::Url::parse)
        .transpose()
        .map_err(|error| format!("OAuth authorization server issuer is invalid: {error}"))?;
    let issuer_bound_callbacks = metadata
        .additional_fields
        .get("authorization_response_iss_parameter_supported")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    if issuer_bound_callbacks {
        if issuer.is_none() {
            return Err(
                "OAuth issuer-bound callbacks require an authorization server issuer".to_owned(),
            );
        }
        return Ok(());
    }

    if let Some(issuer) = issuer {
        let compatible_provider = matches!(
            (
                issuer.as_str(),
                authorization_endpoint
                    .origin()
                    .ascii_serialization()
                    .as_str(),
                token_endpoint.origin().ascii_serialization().as_str(),
            ),
            (
                "https://api.figma.com/",
                "https://www.figma.com",
                "https://api.figma.com",
            ) | (
                "https://agent.robinhood.com/mcp/trading",
                "https://robinhood.com",
                "https://api.robinhood.com",
            )
        );
        if authorization_endpoint.origin() == issuer.origin()
            || authorization_endpoint.origin() == token_endpoint.origin()
            || compatible_provider
        {
            return Ok(());
        }
        return Err(
            "OAuth authorization endpoint origin does not match the authorization server origin without issuer-bound callbacks".to_owned(),
        );
    }

    if token_endpoint.origin() != authorization_endpoint.origin() {
        return Err(
            "OAuth token endpoint origin does not match the authorization server origin without issuer-bound callbacks".to_owned(),
        );
    }
    Ok(())
}

pub fn transport_from_credentials<'call>(
    server_name: &'call str,
    server_url: &'call str,
    http_client: reqwest::Client,
    store: Arc<dyn McpOAuthStore>,
    credentials: McpOAuthCredentials,
    metadata_cache: &'call OAuthMetadataCache,
) -> futures_util::future::BoxFuture<'call, Result<OAuthTransport, String>> {
    Box::pin(async move {
        let base_url = Url::parse(server_url).map_err(|error| {
            format!("failed to initialize MCP OAuth state: URL parse error: {error}")
        })?;
        if credentials.token_endpoint.is_some() {
            let runtime =
                OAuthRuntime::stored(server_name, server_url, http_client, store, credentials)?;
            return Ok(OAuthTransport {
                runtime,
                metadata_cache_hit: false,
            });
        }
        let (metadata, metadata_cache_hit) =
            if let Some(metadata) = metadata_cache.get(server_name, server_url).await {
                (metadata, true)
            } else {
                let metadata = discover(&http_client, &base_url)
                    .await
                    .map_err(|error| format!("failed to discover MCP OAuth metadata: {error}"))?
                    .metadata;
                metadata_cache
                    .insert(server_name, server_url, metadata.clone())
                    .await;
                (metadata, false)
            };
        validate_authorization_server_endpoints(&metadata)?;
        let authorization_issuer = authorization_issuer(&metadata)?;
        let staged_credentials =
            credentials_for_manager(&credentials, authorization_issuer.as_deref());
        let runtime = Arc::new(OAuthRuntime {
            server_name: server_name.to_owned(),
            server_url: server_url.to_owned(),
            base_url: Some(base_url),
            http: http_client,
            store,
            credentials: ArcSwapOption::from_pointee(staged_credentials),
            settled: ArcSwapOption::empty(),
        });
        Ok(OAuthTransport {
            runtime,
            metadata_cache_hit,
        })
    })
}

pub(crate) struct Discovery {
    pub(crate) metadata: AuthorizationMetadata,
    resource: Option<String>,
    challenge_scopes: Vec<String>,
    resource_scopes: Vec<String>,
}

struct DiscoveryResponse {
    status: StatusCode,
    headers: reqwest::header::HeaderMap,
    body: Vec<u8>,
}

async fn read_limited(mut response: reqwest::Response) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| crate::error_chain(&error))?
    {
        if chunk.len() > MAX_OAUTH_HTTP_RESPONSE_BODY_BYTES - body.len() {
            return Err(format!(
                "OAuth HTTP response body exceeds {MAX_OAUTH_HTTP_RESPONSE_BODY_BYTES} bytes"
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn discovery_get(http: &reqwest::Client, url: &Url) -> Result<DiscoveryResponse, String> {
    let failed = |error: String| {
        format!("Metadata error: OAuth metadata discovery failed for {url}\n  Caused by: {error}")
    };
    let response = http
        .get(url.as_str())
        .header("MCP-Protocol-Version", DISCOVERY_PROTOCOL_VERSION)
        .send()
        .await
        .map(nanocode_http::decode_response)
        .map_err(|error| failed(crate::error_chain(&error)))?;
    let status = response.status();
    if status.is_server_error()
        || matches!(
            status,
            StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_MANY_REQUESTS
        )
        || status.as_u16() == 425
    {
        return Err(failed(format!("unexpected HTTP status {status}")));
    }
    let headers = response.headers().clone();
    let body = read_limited(response).await.map_err(failed)?;
    Ok(DiscoveryResponse {
        status,
        headers,
        body,
    })
}

pub(crate) async fn discover(http: &reqwest::Client, base_url: &Url) -> Result<Discovery, String> {
    let mut discovery = Discovery {
        metadata: AuthorizationMetadata::default(),
        resource: None,
        challenge_scopes: Vec::new(),
        resource_scopes: Vec::new(),
    };
    if let Some(resource_metadata_url) =
        discover_resource_metadata_url(http, base_url, &mut discovery.challenge_scopes).await?
        && let Some(metadata) =
            discover_from_resource_metadata(http, base_url, &resource_metadata_url, &mut discovery)
                .await?
    {
        discovery.metadata = metadata;
        return Ok(discovery);
    }
    if let Some(metadata) = discover_authorization_server(http, base_url).await? {
        discovery.metadata = metadata;
        return Ok(discovery);
    }
    tracing::debug!("falling back to legacy OAuth endpoints derived from the base URL");
    discovery.metadata = AuthorizationMetadata::legacy(base_url);
    Ok(discovery)
}

fn challenged_resource_metadata_url(
    headers: &reqwest::header::HeaderMap,
    base_url: &Url,
    challenge_scopes: &mut Vec<String>,
) -> Option<Url> {
    for value in headers.get_all(reqwest::header::WWW_AUTHENTICATE) {
        let Ok(value) = value.to_str() else {
            continue;
        };
        let challenge = WwwAuthenticate::parse(value, base_url);
        if let Some(url) = challenge.resource_metadata_url.clone() {
            let scopes = challenge.scopes();
            if !scopes.is_empty() {
                *challenge_scopes = scopes;
            }
            return Some(url);
        }
    }
    None
}

async fn initialize_challenge(
    http: &reqwest::Client,
    base_url: &Url,
    challenge_scopes: &mut Vec<String>,
) -> Option<Url> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 0,
        "method": "initialize",
        "params": {
            "protocolVersion": DISCOVERY_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": OAUTH_CLIENT_NAME, "version": env!("CARGO_PKG_VERSION")},
        },
    });
    let response = http
        .post(base_url.as_str())
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/event-stream",
        )
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header("MCP-Protocol-Version", DISCOVERY_PROTOCOL_VERSION)
        .body(body.to_string())
        .send()
        .await
        .inspect_err(|error| {
            tracing::debug!(
                "unauthenticated MCP initialize probe failed: {}",
                crate::error_chain(error)
            );
        })
        .ok()?;
    if response.status() != StatusCode::UNAUTHORIZED {
        return None;
    }
    challenged_resource_metadata_url(response.headers(), base_url, challenge_scopes)
}

async fn discover_resource_metadata_url(
    http: &reqwest::Client,
    base_url: &Url,
    challenge_scopes: &mut Vec<String>,
) -> Result<Option<Url>, String> {
    if let Some(url) = initialize_challenge(http, base_url, challenge_scopes).await {
        return Ok(Some(url));
    }
    let candidates = std::iter::once(base_url.clone()).chain(
        nanocode_mcp::oauth::protected_resource_metadata_urls(base_url),
    );
    for candidate in candidates {
        let response = discovery_get(http, &candidate).await?;
        match response.status {
            StatusCode::OK => return Ok(Some(candidate)),
            StatusCode::UNAUTHORIZED => {
                if let Some(url) =
                    challenged_resource_metadata_url(&response.headers, base_url, challenge_scopes)
                {
                    return Ok(Some(url));
                }
            }
            status => {
                tracing::debug!("resource metadata probe returned unexpected status: {status}");
            }
        }
    }
    Ok(None)
}

async fn discover_from_resource_metadata(
    http: &reqwest::Client,
    base_url: &Url,
    resource_metadata_url: &Url,
    discovery: &mut Discovery,
) -> Result<Option<AuthorizationMetadata>, String> {
    let response = discovery_get(http, resource_metadata_url).await?;
    if response.status != StatusCode::OK {
        return Ok(None);
    }
    let Ok(resource_metadata) = serde_json::from_slice::<ProtectedResourceMetadata>(&response.body)
    else {
        return Ok(None);
    };
    // A missing or mismatched `resource` is tolerated: the server's own URL
    // becomes the resource indicator, and its authorization servers are
    // still used.
    discovery.resource = match resource_metadata.validate_resource(base_url) {
        Ok(()) => Some(resource_metadata.resource.clone().unwrap_or_default()),
        Err(error) => {
            tracing::debug!("ignoring protected resource metadata resource: {error}");
            None
        }
    };
    if let Some(scopes) = resource_metadata
        .scopes_supported
        .clone()
        .filter(|scopes| !scopes.is_empty())
    {
        discovery.resource_scopes = scopes;
    }
    for candidate in resource_metadata.authorization_server_candidates() {
        let url = match Url::parse(&candidate).or_else(|_| resource_metadata_url.join(&candidate)) {
            Ok(url) => url,
            Err(error) => {
                tracing::debug!(
                    "Failed to resolve authorization server URL `{candidate}`: {error}"
                );
                continue;
            }
        };
        if !nanocode_mcp::oauth::is_allowed_authorization_server_url(base_url, &url) {
            tracing::warn!("rejecting authorization server metadata URL `{url}`");
            continue;
        }
        let metadata = if url.path().contains("/.well-known/") {
            fetch_authorization_metadata(http, &url).await?
        } else {
            discover_authorization_server(http, &url).await?
        };
        if metadata.is_some() {
            return Ok(metadata);
        }
    }
    Ok(None)
}

async fn discover_authorization_server(
    http: &reqwest::Client,
    base_url: &Url,
) -> Result<Option<AuthorizationMetadata>, String> {
    for url in nanocode_mcp::oauth::authorization_server_metadata_urls(base_url) {
        if let Some(metadata) = fetch_authorization_metadata(http, &url).await? {
            return Ok(Some(metadata));
        }
    }
    Ok(None)
}

async fn fetch_authorization_metadata(
    http: &reqwest::Client,
    url: &Url,
) -> Result<Option<AuthorizationMetadata>, String> {
    let response = discovery_get(http, url).await?;
    if response.status != StatusCode::OK {
        return Ok(None);
    }
    match serde_json::from_slice::<AuthorizationMetadata>(&response.body) {
        Ok(mut metadata) => {
            // Metadata that states no issuer is the issuer's it was fetched
            // for, as servers that leave the field out still mean it.
            if metadata.issuer.is_none() {
                metadata.issuer = nanocode_mcp::oauth::expected_issuer(url)
                    .map(|issuer| issuer.trim_end_matches('/').to_owned());
            }
            nanocode_mcp::oauth::validate_metadata_issuer(url, &metadata)?;
            Ok(Some(metadata))
        }
        Err(error) => {
            tracing::debug!("Failed to parse metadata for {url}: {error}");
            Ok(None)
        }
    }
}

#[derive(Clone, Debug)]
pub struct OAuthServer {
    pub metadata: AuthorizationMetadata,
    pub resource: Option<String>,
    pub scopes: Vec<String>,
}

impl OAuthServer {
    #[must_use]
    pub fn issuer(&self) -> Option<&str> {
        let issuer = self.metadata.issuer.as_deref();
        issuer.map(|issuer| issuer.trim_end_matches('/'))
    }

    pub fn endpoints(&self) -> impl Iterator<Item = &str> {
        let metadata = &self.metadata;
        [
            Some(metadata.authorization_endpoint.as_str()),
            Some(metadata.token_endpoint.as_str()),
            metadata.registration_endpoint.as_deref(),
            metadata.revocation_endpoint(),
        ]
        .into_iter()
        .flatten()
    }
}

pub async fn discover_server(
    http: &reqwest::Client,
    server_url: &Url,
) -> Result<OAuthServer, String> {
    let Discovery {
        metadata,
        resource,
        challenge_scopes,
        resource_scopes,
    } = discover(http, server_url).await?;
    validate_authorization_server_endpoints(&metadata)?;
    let scopes = nanocode_mcp::oauth::select_scopes(&challenge_scopes, &resource_scopes, &metadata);
    Ok(OAuthServer {
        metadata,
        resource: Some(resource.unwrap_or_else(|| server_url.to_string())),
        scopes,
    })
}

pub async fn discover_issuer(http: &reqwest::Client, issuer: &Url) -> Result<OAuthServer, String> {
    let metadata = discover_authorization_server(http, issuer)
        .await?
        .ok_or_else(|| format!("Metadata error: no authorization server metadata for {issuer}"))?;
    validate_authorization_server_endpoints(&metadata)?;
    Ok(OAuthServer {
        metadata,
        resource: None,
        scopes: Vec::new(),
    })
}

async fn send_token_endpoint_request(
    http: &reqwest::Client,
    endpoint: &str,
    request: TokenRequest,
) -> Result<(u16, Option<String>, Vec<u8>), TokenError> {
    let mut builder = http
        .post(endpoint)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(request.body);
    if let Some(authorization) = request.authorization {
        builder = builder.header(reqwest::header::AUTHORIZATION, authorization);
    }
    let response = builder
        .send()
        .await
        .map(nanocode_http::decode_response)
        .map_err(|error| {
            TokenError::Invalid(format!("Request failed: {}", crate::error_chain(&error)))
        })?;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = read_limited(response)
        .await
        .map_err(|error| TokenError::Invalid(format!("Request failed: {error}")))?;
    Ok((status, content_type, body))
}

pub async fn token_endpoint_request(
    http: &reqwest::Client,
    endpoint: &str,
    request: TokenRequest,
) -> Result<TokenResponse, TokenError> {
    let (status, content_type, body) = send_token_endpoint_request(http, endpoint, request).await?;
    nanocode_mcp::oauth::parse_token_response(status, content_type.as_deref(), &body)
}

pub async fn revoke_token(
    http: &reqwest::Client,
    endpoint: &str,
    request: TokenRequest,
) -> Result<(), TokenError> {
    let (status, _, body) = send_token_endpoint_request(http, endpoint, request).await?;
    nanocode_mcp::oauth::parse_revocation_response(status, &body)
}

pub async fn revoke_sign_in(
    http: &reqwest::Client,
    endpoint: &str,
    client: &ClientAuthentication,
    access_token: &str,
    refresh_token: Option<&str>,
) -> Result<(), TokenError> {
    let (token, hint) = refresh_token.map_or((access_token, "access_token"), |token| {
        (token, "refresh_token")
    });
    let request = nanocode_mcp::oauth::revocation_request(client, token, Some(hint));
    revoke_token(http, endpoint, request).await
}

pub struct WebLogin<'a> {
    pub server: &'a OAuthServer,
    pub client_id: &'a str,
    pub redirect_uri: &'a str,
    pub scopes: &'a [String],
    pub params: &'a [(&'a str, &'a str)],
    pub state_prefix: &'a str,
}

pub struct StartedWebLogin {
    pub authorization_url: String,
    pub pending: PendingWebLogin,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingWebLogin {
    pub state: String,
    pub code_verifier: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub token_endpoint: String,
    pub resource: Option<String>,
    pub issuer: Option<String>,
    pub require_issuer: bool,
}

impl WebLogin<'_> {
    pub fn start(&self) -> Result<StartedWebLogin, String> {
        let metadata = &self.server.metadata;
        metadata.validate_authorization_code_support()?;
        let pkce = Pkce::from_random(&random_bytes::<PKCE_VERIFIER_BYTES>()?);
        let state = format!(
            "{}{}",
            self.state_prefix,
            nanocode_mcp::oauth::state_from_random(&random_bytes::<STATE_BYTES>()?)
        );
        let authorization_url = AuthorizationUrl {
            metadata,
            client_id: self.client_id,
            redirect_uri: self.redirect_uri,
            state: &state,
            code_challenge: &pkce.challenge,
            scopes: self.scopes,
            resource: self.server.resource.as_deref().unwrap_or_default(),
        }
        .build_with(self.params)?
        .to_string();
        Ok(StartedWebLogin {
            authorization_url,
            pending: PendingWebLogin {
                state,
                code_verifier: pkce.verifier,
                client_id: self.client_id.to_owned(),
                redirect_uri: self.redirect_uri.to_owned(),
                token_endpoint: metadata.token_endpoint.clone(),
                resource: self.server.resource.clone(),
                issuer: metadata.issuer.clone(),
                require_issuer: metadata.requires_issuer_in_callback(),
            },
        })
    }
}

impl PendingWebLogin {
    pub async fn complete(
        &self,
        http: &reqwest::Client,
        client: &ClientAuthentication,
        callback: &AuthorizationCallback,
    ) -> Result<TokenResponse, TokenError> {
        if callback.state != self.state {
            return Err(TokenError::Invalid(
                "Internal error: Authorization state not found".to_owned(),
            ));
        }
        if client.client_id != self.client_id {
            return Err(TokenError::Invalid(
                "OAuth client changed during authorization".to_owned(),
            ));
        }
        callback
            .validate_issuer(self.issuer.as_deref(), self.require_issuer)
            .map_err(TokenError::Invalid)?;
        let request = nanocode_mcp::oauth::authorization_code_request(
            client,
            &callback.code,
            &self.code_verifier,
            &self.redirect_uri,
            self.resource.as_deref().unwrap_or_default(),
        );
        token_endpoint_request(http, &self.token_endpoint, request).await
    }
}

fn random_bytes<const N: usize>() -> Result<[u8; N], String> {
    let mut bytes = [0_u8; N];
    getrandom::fill(&mut bytes)
        .map_err(|error| format!("failed to generate OAuth randomness: {error}"))?;
    Ok(bytes)
}

pub(crate) struct LoginSession {
    client: ClientAuthentication,
    pending: PendingWebLogin,
    authorization_url: String,
}

impl LoginSession {
    async fn start(
        http: &reqwest::Client,
        base_url: &Url,
        discovery: Discovery,
        redirect_uri: &str,
    ) -> Result<Self, String> {
        let Discovery {
            metadata,
            resource,
            challenge_scopes,
            resource_scopes,
        } = discovery;
        let scopes =
            nanocode_mcp::oauth::select_scopes(&challenge_scopes, &resource_scopes, &metadata);
        let request =
            ClientRegistrationRequest::public_client(OAUTH_CLIENT_NAME, redirect_uri, &scopes);
        let client = register_client(http, &metadata, &request)
            .await
            .map_err(|error| {
                format!("Registration failed: Dynamic registration failed: {error}")
            })?;
        let server = OAuthServer {
            metadata,
            resource: Some(resource.unwrap_or_else(|| base_url.to_string())),
            scopes,
        };
        let started = WebLogin {
            server: &server,
            client_id: &client.client_id,
            redirect_uri,
            scopes: &server.scopes,
            params: &[],
            state_prefix: "",
        }
        .start()?;
        Ok(Self {
            client,
            pending: started.pending,
            authorization_url: started.authorization_url,
        })
    }

    async fn exchange(
        &self,
        http: &reqwest::Client,
        callback: &str,
    ) -> Result<TokenResponse, String> {
        let callback = Url::parse(callback).map_err(|error| format!("URL parse error: {error}"))?;
        let callback = AuthorizationCallback::from_url(&callback)?;
        self.pending
            .complete(http, &self.client, &callback)
            .await
            .map_err(|error| format!("OAuth token exchange failed: {error}"))
    }
}

pub async fn register_web_client(
    http: &reqwest::Client,
    metadata: &AuthorizationMetadata,
    client_name: &str,
    redirect_uri: &str,
    scopes: &[String],
) -> Result<ClientAuthentication, String> {
    let methods = metadata.token_endpoint_auth_methods();
    let method = ["client_secret_basic", "client_secret_post", "none"]
        .into_iter()
        .find(|method| methods.contains(method))
        .ok_or_else(|| {
            "Registration failed: no supported token endpoint authentication method".to_owned()
        })?;
    let request = ClientRegistrationRequest::web_client(
        client_name,
        redirect_uri,
        scopes,
        method,
        metadata.issues_refresh_tokens(),
    );
    register_client(http, metadata, &request).await
}

async fn register_client(
    http: &reqwest::Client,
    metadata: &AuthorizationMetadata,
    request: &ClientRegistrationRequest,
) -> Result<ClientAuthentication, String> {
    let Some(endpoint) = metadata.registration_endpoint.as_deref() else {
        return Err("Registration failed: Dynamic client registration not supported".to_owned());
    };
    metadata.validate_authorization_code_support()?;
    let body =
        serde_json::to_vec(request).map_err(|error| format!("Registration failed: {error}"))?;
    let response = http
        .post(endpoint)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
        .map(nanocode_http::decode_response)
        .map_err(|error| {
            format!(
                "Registration failed: HTTP request error: {}",
                crate::error_chain(&error)
            )
        })?;
    let status = response.status();
    let body = read_limited(response)
        .await
        .map_err(|error| format!("Registration failed: HTTP request error: {error}"))?;
    if !status.is_success() {
        return Err(format!(
            "Registration failed: HTTP {status}: {}",
            String::from_utf8_lossy(&body)
        ));
    }
    let registration = serde_json::from_slice::<ClientRegistrationResponse>(&body)
        .map_err(|error| format!("Registration failed: analyze response error: {error}"))?;
    let client_secret = registration
        .client_secret
        .filter(|secret| !secret.is_empty());
    match registration
        .token_endpoint_auth_method
        .as_deref()
        .unwrap_or(&request.token_endpoint_auth_method)
    {
        "none" if client_secret.is_some() => Ok(ClientAuthentication {
            client_id: registration.client_id,
            client_secret,
            secret_in_body: metadata.uses_client_secret_post(),
        }),
        method => ClientAuthentication::with_method(registration.client_id, client_secret, method)
            .map_err(|error| format!("Registration failed: {error}")),
    }
}

pub struct OAuthLoginFlow {
    pub authorization_url: String,
    pub completion: JoinHandle<Result<(), String>>,
}

pub fn begin_login(
    server_name: String,
    server_url: String,
    headers: BTreeMap<String, SecretSource>,
    store: Arc<dyn McpOAuthStore>,
) -> futures_util::future::BoxFuture<'static, Result<OAuthLoginFlow, String>> {
    Box::pin(async move {
        let client = oauth_http_client(headers)?;
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|error| format!("failed to bind MCP OAuth callback: {error}"))?;
        let address = listener
            .local_addr()
            .map_err(|error| format!("failed to inspect MCP OAuth callback: {error}"))?;
        let redirect_uri = format!("http://{address}/callback");
        let authorization_span = info_span!(
            target: "nanocode_code_mode",
            "mcp.oauth.authorization_start",
            otel.kind = "client",
            otel.status_code = tracing::field::Empty,
            status = tracing::field::Empty,
        );
        let authorization = async {
            let base_url = Url::parse(&server_url).map_err(|error| {
                format!("failed to discover MCP OAuth metadata: URL parse error: {error}")
            })?;
            let discovery = discover(&client, &base_url)
                .await
                .map_err(|error| format!("failed to discover MCP OAuth metadata: {error}"))?;
            validate_authorization_server_endpoints(&discovery.metadata)?;
            let authorization_issuer = authorization_issuer(&discovery.metadata)?;
            let session = LoginSession::start(&client, &base_url, discovery, &redirect_uri)
                .await
                .map_err(|error| format!("failed to start MCP OAuth authorization: {error}"))?;
            Ok::<_, String>((session, authorization_issuer))
        }
        .instrument(authorization_span.clone())
        .await;
        authorization_span.record(
            "status",
            if authorization.is_ok() {
                "completed"
            } else {
                "failed"
            },
        );
        authorization_span.record(
            "otel.status_code",
            if authorization.is_ok() { "OK" } else { "ERROR" },
        );
        let (session, authorization_issuer) = authorization?;
        let authorization_url = session.authorization_url.clone();

        let parent = tracing::Span::current();
        let completion = tokio::spawn(
            complete_login(
                listener,
                client,
                session,
                authorization_issuer,
                store,
                server_name,
                server_url,
            )
            .instrument(parent),
        );
        Ok(OAuthLoginFlow {
            authorization_url,
            completion,
        })
    })
}

async fn complete_login(
    listener: TcpListener,
    client: reqwest::Client,
    session: LoginSession,
    authorization_issuer: Option<String>,
    store: Arc<dyn McpOAuthStore>,
    server_name: String,
    server_url: String,
) -> Result<(), String> {
    let callback_span = info_span!(
        target: "nanocode_code_mode",
        "mcp.oauth.callback_wait",
        otel.kind = "server",
        otel.status_code = tracing::field::Empty,
        status = tracing::field::Empty,
    );
    let callback = match tokio::time::timeout(
        LOGIN_TIMEOUT,
        receive_callback(listener, &session.pending.redirect_uri),
    )
    .instrument(callback_span.clone())
    .await
    {
        Ok(callback) => callback,
        Err(_) => Err("timed out waiting for MCP OAuth callback".to_owned()),
    };
    callback_span.record(
        "status",
        if callback.is_ok() {
            "completed"
        } else {
            "failed"
        },
    );
    callback_span.record(
        "otel.status_code",
        if callback.is_ok() { "OK" } else { "ERROR" },
    );
    let callback = callback?;
    let exchange_span = info_span!(
        target: "nanocode_code_mode",
        "mcp.oauth.code_exchange",
        otel.kind = "client",
        otel.status_code = tracing::field::Empty,
        status = tracing::field::Empty,
    );
    let result = session
        .exchange(&client, &callback)
        .instrument(exchange_span.clone())
        .await
        .map_err(|error| format!("failed to exchange MCP OAuth code: {error}"));
    exchange_span.record(
        "status",
        if result.is_ok() {
            "completed"
        } else {
            "failed"
        },
    );
    exchange_span.record(
        "otel.status_code",
        if result.is_ok() { "OK" } else { "ERROR" },
    );
    let response = result?;
    let credentials = McpOAuthCredentials::from_token_response(
        session.client.client_id.clone(),
        &response,
        authorization_issuer,
    )
    .client(session.client.clone());
    let save_span = info_span!(
        target: "nanocode_code_mode",
        "mcp.oauth.credentials_save",
        otel.kind = "internal",
        otel.status_code = tracing::field::Empty,
        reason = "login",
        status = tracing::field::Empty,
    );
    let saved = store
        .save(&server_name, &server_url, &credentials)
        .instrument(save_span.clone())
        .await;
    save_span.record("status", if saved.is_ok() { "completed" } else { "failed" });
    save_span.record(
        "otel.status_code",
        if saved.is_ok() { "OK" } else { "ERROR" },
    );
    saved
}

pub type HttpClient = reqwest::Client;

pub fn http_client(
    user_agent: &str,
    https_only: bool,
    timeout: Duration,
) -> reqwest::Result<HttpClient> {
    nanocode_tls::install_default_rustls_crypto_provider();
    reqwest::Client::builder()
        .user_agent(user_agent)
        .redirect(reqwest::redirect::Policy::none())
        .https_only(https_only)
        .timeout(timeout)
        .build()
}

fn oauth_http_client(headers: BTreeMap<String, SecretSource>) -> Result<reqwest::Client, String> {
    let mut resolved = reqwest::header::HeaderMap::with_capacity(headers.len());
    for (name, source) in headers {
        let name = name
            .parse::<HeaderName>()
            .map_err(|error| format!("invalid HTTP header name `{name}`: {error}"))?;
        let value = source.resolve()?;
        let mut value = HeaderValue::from_str(&value)
            .map_err(|error| format!("invalid value for HTTP header `{name}`: {error}"))?;
        value.set_sensitive(true);
        resolved.insert(name, value);
    }
    let replays_plaintext_proxy_credentials =
        resolved.contains_key(reqwest::header::PROXY_AUTHORIZATION);
    nanocode_tls::install_default_rustls_crypto_provider();
    reqwest::Client::builder()
        .default_headers(resolved)
        .pool_max_idle_per_host(0)
        .redirect(crate::same_origin_redirect_policy(
            replays_plaintext_proxy_credentials,
        ))
        .build()
        .map_err(|error| format!("failed to build MCP OAuth HTTP client: {error}"))
}

async fn receive_callback(listener: TcpListener, redirect_uri: &str) -> Result<String, String> {
    let (mut stream, _) = listener
        .accept()
        .await
        .map_err(|error| format!("failed to accept MCP OAuth callback: {error}"))?;
    let mut bytes = Vec::with_capacity(2048);
    loop {
        let mut chunk = [0_u8; 1024];
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|error| format!("failed to read MCP OAuth callback: {error}"))?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if bytes.len() > MAX_CALLBACK_BYTES {
            return Err("MCP OAuth callback headers were too large".to_owned());
        }
    }
    let request = std::str::from_utf8(&bytes)
        .map_err(|_| "MCP OAuth callback was not valid HTTP".to_owned())?;
    let target = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| "MCP OAuth callback did not contain a request target".to_owned())?;
    let base = reqwest::Url::parse(redirect_uri)
        .map_err(|error| format!("invalid MCP OAuth redirect URI: {error}"))?;
    let callback = base
        .join(target)
        .map_err(|error| format!("invalid MCP OAuth callback target: {error}"))?;
    if callback.path() != base.path() {
        let _ = respond(&mut stream, 400, "Invalid OAuth callback path").await;
        return Err("MCP OAuth callback used an unexpected path".to_owned());
    }
    respond(
        &mut stream,
        200,
        "Authentication received. You may close this window.",
    )
    .await?;
    Ok(callback.to_string())
}

async fn respond(
    stream: &mut tokio::net::TcpStream,
    status: u16,
    body: &str,
) -> Result<(), String> {
    let reason = if status == 200 { "OK" } else { "Bad Request" };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .await
        .map_err(|error| format!("failed to answer MCP OAuth callback: {error}"))
}

fn now_millis() -> u64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis();
    u64::try_from(millis).unwrap_or(u64::MAX)
}
