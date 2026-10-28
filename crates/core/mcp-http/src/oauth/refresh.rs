use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Weak},
    time::Duration,
};

use arc_swap::ArcSwap;
use nanocode_sync::SingleFlight;

use nanocode_mcp::oauth::{TokenError, TokenResponse};
use tokio::time::timeout;

use super::{McpOAuthCredentials, OAuthRuntime, now_millis};

const REFRESH_SKEW_MILLIS: u64 = 30_000;
const REFRESH_LOCK_TIMEOUT: Duration = Duration::from_secs(60);
const REFRESH_REQUEST_TIMEOUT: Duration = Duration::from_secs(45);

type Refresh = SingleFlight<Result<(), String>>;

static REFRESHES: LazyLock<ArcSwap<HashMap<String, Weak<Refresh>>>> =
    LazyLock::new(ArcSwap::default);

fn refresh_for(key: &str) -> Arc<Refresh> {
    let mut refresh = None;
    REFRESHES.rcu(|refreshes| {
        let mut refreshes: HashMap<_, _> = refreshes
            .iter()
            .filter(|(_, refresh)| refresh.strong_count() > 0)
            .map(|(key, refresh)| (key.clone(), refresh.clone()))
            .collect();
        let current = refreshes
            .get(key)
            .and_then(Weak::upgrade)
            .unwrap_or_else(|| Arc::new(Refresh::default()));
        refreshes.insert(key.to_owned(), Arc::downgrade(&current));
        refresh = Some(current);
        refreshes
    });
    refresh.unwrap_or_default()
}

impl McpOAuthCredentials {
    fn expired(&self) -> bool {
        self.expires_at_millis
            .is_some_and(|expires_at| now_millis() >= expires_at)
    }

    fn needs_refresh(&self) -> bool {
        self.expires_at_millis.is_some_and(|expires_at| {
            now_millis().saturating_add(REFRESH_SKEW_MILLIS) >= expires_at
        })
    }

    fn from_refresh_response(client_id: String, response: &TokenResponse, previous: &Self) -> Self {
        let mut credentials =
            Self::from_token_response(client_id, response, previous.issuer.clone());
        credentials
            .client_secret
            .clone_from(&previous.client_secret);
        credentials.secret_in_body = previous.secret_in_body;
        credentials
            .token_endpoint
            .clone_from(&previous.token_endpoint);
        credentials.resource.clone_from(&previous.resource);
        if response.refresh_token.is_none() {
            credentials
                .refresh_token
                .clone_from(&previous.refresh_token);
        }
        if response.scope.is_none() {
            credentials.scopes.clone_from(&previous.scopes);
        }
        credentials
    }
}

impl OAuthRuntime {
    pub fn refresh_if_needed<'call>(
        self: &'call Arc<Self>,
    ) -> futures_util::future::BoxFuture<'call, Result<(), String>> {
        Box::pin(async move {
            let refresh_needed = self.credentials.load().as_ref().is_none_or(|credentials| {
                credentials.needs_refresh()
                    && (credentials.expired() || !self.settled_on(&credentials.access_token))
            });
            if !refresh_needed {
                return Ok(());
            }
            self.spawn_refresh("expiry", None).await
        })
    }

    fn settled_on(&self, access_token: &str) -> bool {
        self.settled
            .load()
            .as_deref()
            .is_some_and(|settled| settled == access_token)
    }

    pub async fn refresh_after_rejection(
        self: &Arc<Self>,
        rejected_access_token: &str,
    ) -> Result<String, String> {
        self.spawn_refresh("rejected", Some(rejected_access_token.to_owned()))
            .await?;
        self.current_access_token().await
    }

    pub async fn refused_after_refresh(&self, access_token: &str) {
        let current = self
            .credentials
            .load()
            .as_ref()
            .is_some_and(|credentials| credentials.access_token == access_token);
        if current {
            self.store
                .authorization_required(&self.server_name, &self.server_url)
                .await;
        }
    }

    pub async fn with_access_token<T, Call, Answer>(
        self: &Arc<Self>,
        mut call: Call,
        refused: impl Fn(&T) -> bool,
    ) -> Result<T, String>
    where
        Call: FnMut(String) -> Answer,
        Answer: std::future::Future<Output = T>,
    {
        let token = self.access_token().await?;
        let outcome = call(token.clone()).await;
        if !refused(&outcome) {
            return Ok(outcome);
        }
        let token = self.refresh_after_rejection(&token).await?;
        let outcome = call(token.clone()).await;
        if refused(&outcome) {
            self.refused_after_refresh(&token).await;
        }
        Ok(outcome)
    }

    async fn spawn_refresh(
        self: &Arc<Self>,
        reason: &'static str,
        rejected_access_token: Option<String>,
    ) -> Result<(), String> {
        let runtime = Arc::clone(self);
        let store = self.store.refresh_key().map_or_else(
            || format!("{:p}", Arc::as_ptr(&self.store)),
            |key| format!("key:{key}"),
        );
        let refresh = refresh_for(&format!(
            "{store}\0{}\0{}",
            self.server_name, self.server_url
        ));
        refresh
            .run(|| async move {
                let server_name = runtime.server_name.clone();
                let task = tokio::spawn(async move {
                    let result = runtime
                        .refresh_transaction(rejected_access_token.as_deref())
                        .await;
                    if let Err(error) = &result {
                        tracing::warn!(
                            server = %runtime.server_name,
                            refresh_reason = reason,
                            %error,
                            "MCP OAuth refresh transaction failed"
                        );
                    }
                    result
                });
                task.await.map_err(|error| {
                    format!("OAuth refresh task failed for MCP server `{server_name}`: {error}")
                })?
            })
            .await?;
        if let Some(latest) = self
            .store
            .load(&self.server_name, &self.server_url)
            .await
            .map_err(|error| format!("failed to reread authoritative OAuth credentials: {error}"))?
        {
            self.credentials.store(Some(Arc::new(latest)));
        }
        Ok(())
    }

    async fn refresh_transaction(&self, rejected_access_token: Option<&str>) -> Result<(), String> {
        let held = self
            .credentials
            .load()
            .as_ref()
            .map(|credentials| credentials.access_token.clone());
        let _refresh_lock = timeout(
            REFRESH_LOCK_TIMEOUT,
            self.store
                .acquire_refresh_lock(&self.server_name, &self.server_url),
        )
        .await
        .map_err(|_| {
            format!(
                "timed out after {REFRESH_LOCK_TIMEOUT:?} waiting for the OAuth refresh lock for `{}`",
                self.server_name
            )
        })??;

        let latest = self
            .store
            .load(&self.server_name, &self.server_url)
            .await
            .map_err(|error| {
                format!("failed to reread authoritative OAuth credentials: {error}")
            })?;
        let Some(latest) = latest else {
            self.credentials.store(None);
            return Err(format!(
                "OAuth credentials for `{}` were removed; authorization required",
                self.server_name
            ));
        };

        let rejected = rejected_access_token.is_some_and(|token| token == latest.access_token);
        let rotated = rejected_access_token.is_none()
            && held.as_deref() != Some(latest.access_token.as_str())
            && !latest.expired();
        if (!latest.needs_refresh() || rotated) && !rejected {
            self.settled
                .store(Some(Arc::new(latest.access_token.clone())));
            self.credentials.store(Some(Arc::new(latest)));
            return Ok(());
        }
        let Some(refresh_token) = latest
            .refresh_token
            .as_deref()
            .filter(|token| !token.trim().is_empty())
        else {
            self.store
                .authorization_required(&self.server_name, &self.server_url)
                .await;
            return Err(format!(
                "OAuth credentials for `{}` cannot be refreshed; authorization required",
                self.server_name
            ));
        };

        self.credentials.store(Some(Arc::new(latest.clone())));
        let mut scopes = latest.scopes.clone();
        let (token_endpoint, resource) = match (&latest.token_endpoint, &self.base_url) {
            (Some(token_endpoint), _) => (
                token_endpoint.clone(),
                latest
                    .resource
                    .clone()
                    .unwrap_or_else(|| self.server_url.clone()),
            ),
            (None, Some(base_url)) => {
                let metadata = self.current_metadata(base_url, &latest).await?;
                metadata.add_offline_access(&mut scopes);
                let resource = latest
                    .resource
                    .clone()
                    .unwrap_or_else(|| base_url.to_string());
                (metadata.token_endpoint, resource)
            }
            (None, None) => {
                return Err(format!(
                    "OAuth credentials for `{}` do not name their token endpoint",
                    self.server_name
                ));
            }
        };
        let request = nanocode_mcp::oauth::refresh_token_request(
            &latest.client_authentication(),
            refresh_token,
            &scopes,
            &resource,
        );
        let response = match timeout(
            REFRESH_REQUEST_TIMEOUT,
            super::token_endpoint_request(&self.http, &token_endpoint, request),
        )
        .await
        {
            Ok(Ok(response)) => response,
            Ok(Err(error @ TokenError::Rejected(_))) => {
                self.store
                    .authorization_required(&self.server_name, &self.server_url)
                    .await;
                return Err(format!(
                    "OAuth refresh token for `{}` was rejected; authorization required: OAuth refresh token was rejected: {error}",
                    self.server_name
                ));
            }
            Ok(Err(_)) | Err(_) if rejected_access_token.is_none() && !latest.expired() => {
                self.credentials.store(Some(Arc::new(latest)));
                return Ok(());
            }
            Ok(Err(error)) => {
                return Err(format!(
                    "temporarily failed to refresh OAuth credentials for `{}`: OAuth token refresh failed: {error}",
                    self.server_name
                ));
            }
            Err(_) => {
                return Err(format!(
                    "timed out after {REFRESH_REQUEST_TIMEOUT:?} refreshing OAuth credentials for `{}`",
                    self.server_name
                ));
            }
        };
        let refreshed = McpOAuthCredentials::from_refresh_response(
            latest.client_id.clone(),
            &response,
            &latest,
        );

        if let Err(error) = self
            .store
            .save(&self.server_name, &self.server_url, &refreshed)
            .await
        {
            return Err(format!(
                "failed to persist refreshed OAuth credentials for `{}`: {error}",
                self.server_name
            ));
        }

        self.settled
            .store(Some(Arc::new(refreshed.access_token.clone())));
        self.credentials.store(Some(Arc::new(refreshed)));
        Ok(())
    }

    async fn current_metadata(
        &self,
        base_url: &reqwest::Url,
        latest: &McpOAuthCredentials,
    ) -> Result<nanocode_mcp::oauth::AuthorizationMetadata, String> {
        let policy = super::DiscoveryPolicy::default();
        let discovery = super::discover(&self.http, base_url, &policy);
        let metadata = timeout(REFRESH_REQUEST_TIMEOUT, discovery)
            .await
            .map_err(|_| {
                format!(
                    "timed out after {REFRESH_REQUEST_TIMEOUT:?} resolving current OAuth metadata for `{}`",
                    self.server_name
                )
            })?
            .map_err(|error| {
                format!(
                    "failed to resolve current OAuth metadata for `{}`: {error}",
                    self.server_name
                )
            })?
            .metadata;
        super::validate_authorization_server_endpoints(&metadata, false).map_err(|error| {
            format!(
                "current OAuth metadata for `{}` is invalid: {error}",
                self.server_name
            )
        })?;
        let current_issuer = super::authorization_issuer(&metadata)?;
        super::validate_refresh_token_issuer(latest, current_issuer.as_deref()).map_err(
            |error| {
                format!(
                    "OAuth refresh credentials for `{}` cannot be used: {error}",
                    self.server_name
                )
            },
        )?;
        Ok(metadata)
    }
}
