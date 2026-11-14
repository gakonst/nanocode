use std::{sync::Arc, time::Duration};

use serde_json::{Value, json};

use crate::{
    ClaudeAccessToken, ClaudeAuthFuture, ClaudeAuthUnavailable, ClaudeClient, ClaudeTokenSource,
    RefreshingClaudeAuth,
};

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1";
const JWT_BEARER_GRANT: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";
const MANDATORY_REFRESH: Duration = Duration::from_secs(30);

pub trait ClaudeIdentityTokenSource: Send + Sync {
    fn identity_token(&self) -> ClaudeAuthFuture<'_, Result<String, ClaudeAuthUnavailable>>;
}

#[derive(Clone)]
pub struct ClaudeWorkloadIdentity {
    pub federation_rule_id: String,
    pub organization_id: String,
    pub service_account_id: String,
    pub workspace_id: Option<String>,
    pub identity_token: Arc<dyn ClaudeIdentityTokenSource>,
}

struct FederatedToken {
    http: nanocode_http::Client,
    token_url: String,
    identity: ClaudeWorkloadIdentity,
}

impl FederatedToken {
    async fn exchange(&self) -> Result<ClaudeAccessToken, ClaudeAuthUnavailable> {
        let identity = &self.identity;
        let assertion = identity.identity_token.identity_token().await?;
        if assertion.is_empty() {
            return Err(ClaudeAuthUnavailable::new("the identity token is empty"));
        }
        let mut body = json!({
            "grant_type": JWT_BEARER_GRANT,
            "assertion": assertion,
            "federation_rule_id": identity.federation_rule_id,
            "organization_id": identity.organization_id,
            "service_account_id": identity.service_account_id,
        });
        if let Some(workspace_id) = &identity.workspace_id {
            body["workspace_id"] = Value::String(workspace_id.clone());
        }
        let response = self
            .http
            .post(&self.token_url)
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .map(nanocode_http::decode_response)
            .map_err(|error| {
                ClaudeAuthUnavailable::new(format!(
                    "Claude token exchange could not reach {}: {}",
                    self.token_url,
                    error.without_url()
                ))
            })?;
        let status = response.status();
        let request_id = response
            .headers()
            .get("request-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let text = response.text().await.map_err(|error| {
            ClaudeAuthUnavailable::new(format!(
                "Claude token exchange response failed: {}",
                error.without_url()
            ))
        })?;
        let reply: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if !status.is_success() {
            let error = &reply["error"];
            return Err(ClaudeAuthUnavailable::new(format!(
                "Claude token exchange for {} was rejected with HTTP {} ({}: {}){}",
                identity.federation_rule_id,
                status.as_u16(),
                error["type"].as_str().unwrap_or("unknown_error"),
                error["message"].as_str().unwrap_or(&text),
                request_id.map_or_else(String::new, |id| format!(", request {id}")),
            )));
        }
        let access_token = reply["access_token"].as_str().ok_or_else(|| {
            ClaudeAuthUnavailable::new("Claude token exchange returned no access_token")
        })?;
        let expires_in = reply["expires_in"].as_u64().ok_or_else(|| {
            ClaudeAuthUnavailable::new("Claude token exchange returned no expires_in")
        })?;
        ClaudeAccessToken::new(
            access_token,
            nanocode_platform::system_time() + Duration::from_secs(expires_in),
        )
    }
}

impl ClaudeTokenSource for FederatedToken {
    fn refresh(&self) -> ClaudeAuthFuture<'_, Result<ClaudeAccessToken, ClaudeAuthUnavailable>> {
        Box::pin(self.exchange())
    }

    fn refresh_rejected<'a>(
        &'a self,
        _rejected: &'a nanocode_http::types::HeaderValue,
    ) -> ClaudeAuthFuture<'a, Result<Option<ClaudeAccessToken>, ClaudeAuthUnavailable>> {
        Box::pin(async move { self.exchange().await.map(Some) })
    }
}

impl ClaudeClient {
    pub fn workload_identity(identity: ClaudeWorkloadIdentity) -> Self {
        Self::federated(Self::http_client(), ANTHROPIC_API_URL, identity)
    }

    pub fn messages_api_federated(
        base_url: &str,
        identity: ClaudeWorkloadIdentity,
        connect_timeout: Duration,
        idle_timeout: Duration,
    ) -> Result<Self, nanocode_http::Error> {
        nanocode_tls::install_default_rustls_crypto_provider();
        let http = nanocode_http::Client::builder()
            .connect_timeout(connect_timeout)
            .read_timeout(idle_timeout)
            .default_headers(nanocode_http::accept_encoding(
                nanocode_http::header::HeaderMap::new(),
            ))
            .build()?;
        Ok(Self::federated(http, base_url, identity))
    }

    fn federated(
        http: nanocode_http::Client,
        base_url: &str,
        identity: ClaudeWorkloadIdentity,
    ) -> Self {
        let auth = identity.auth(http.clone(), base_url);
        Self::with_auth_provider(
            http,
            format!("{}/messages", base_url.trim_end_matches('/')),
            Arc::new(auth),
        )
    }
}

impl ClaudeWorkloadIdentity {
    #[must_use]
    pub fn auth(self, http: nanocode_http::Client, base_url: &str) -> RefreshingClaudeAuth {
        let token = FederatedToken {
            http,
            token_url: format!("{}/oauth/token", base_url.trim_end_matches('/')),
            identity: self,
        };
        RefreshingClaudeAuth::new(Arc::new(token), MANDATORY_REFRESH)
    }
}
