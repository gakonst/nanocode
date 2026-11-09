use std::{path::PathBuf, sync::Arc, time::Duration};

use serde_json::{Value, json};

use crate::{
    ClaudeAccessToken, ClaudeAuthFuture, ClaudeAuthUnavailable, ClaudeClient, ClaudeTokenSource,
    RefreshingClaudeAuth,
};

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1";
const JWT_BEARER_GRANT: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";
const MANDATORY_REFRESH: Duration = Duration::from_secs(30);

const FEDERATION_RULE_ID_ENV: &str = "ANTHROPIC_FEDERATION_RULE_ID";
const ORGANIZATION_ID_ENV: &str = "ANTHROPIC_ORGANIZATION_ID";
const SERVICE_ACCOUNT_ID_ENV: &str = "ANTHROPIC_SERVICE_ACCOUNT_ID";
const WORKSPACE_ID_ENV: &str = "ANTHROPIC_WORKSPACE_ID";
const IDENTITY_TOKEN_FILE_ENV: &str = "ANTHROPIC_IDENTITY_TOKEN_FILE";
const IDENTITY_TOKEN_ENV: &str = "ANTHROPIC_IDENTITY_TOKEN";

#[derive(Clone)]
pub enum ClaudeIdentityToken {
    File(PathBuf),
    Value(String),
}

#[derive(Clone)]
pub struct ClaudeWorkloadIdentity {
    pub federation_rule_id: String,
    pub organization_id: String,
    pub service_account_id: String,
    pub workspace_id: Option<String>,
    pub identity_token: ClaudeIdentityToken,
}

impl ClaudeWorkloadIdentity {
    pub fn from_env() -> Result<Option<Self>, ClaudeAuthUnavailable> {
        let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
        let identity_token = match (var(IDENTITY_TOKEN_FILE_ENV), var(IDENTITY_TOKEN_ENV)) {
            (Some(_), Some(_)) => {
                return Err(ClaudeAuthUnavailable::new(format!(
                    "set only one of {IDENTITY_TOKEN_FILE_ENV} and {IDENTITY_TOKEN_ENV}"
                )));
            }
            (Some(path), None) => Some(ClaudeIdentityToken::File(path.into())),
            (None, Some(token)) => Some(ClaudeIdentityToken::Value(token)),
            (None, None) => None,
        };
        match (
            var(FEDERATION_RULE_ID_ENV),
            var(ORGANIZATION_ID_ENV),
            var(SERVICE_ACCOUNT_ID_ENV),
            identity_token,
        ) {
            (
                Some(federation_rule_id),
                Some(organization_id),
                Some(service_account_id),
                Some(identity_token),
            ) => Ok(Some(Self {
                federation_rule_id,
                organization_id,
                service_account_id,
                workspace_id: var(WORKSPACE_ID_ENV),
                identity_token,
            })),
            (None, None, None, None) => Ok(None),
            (rule, organization, account, token) => {
                let missing = [
                    (rule.is_none(), FEDERATION_RULE_ID_ENV),
                    (organization.is_none(), ORGANIZATION_ID_ENV),
                    (account.is_none(), SERVICE_ACCOUNT_ID_ENV),
                    (
                        token.is_none(),
                        "ANTHROPIC_IDENTITY_TOKEN_FILE or ANTHROPIC_IDENTITY_TOKEN",
                    ),
                ]
                .into_iter()
                .filter_map(|(missing, name)| missing.then_some(name))
                .collect::<Vec<_>>()
                .join(", ");
                Err(ClaudeAuthUnavailable::new(format!(
                    "Claude workload identity federation is partly configured; missing {missing}"
                )))
            }
        }
    }

    async fn assertion(&self) -> Result<String, ClaudeAuthUnavailable> {
        let token = match &self.identity_token {
            ClaudeIdentityToken::File(path) => {
                tokio::fs::read_to_string(path).await.map_err(|error| {
                    ClaudeAuthUnavailable::new(format!(
                        "cannot read the identity token {}: {error}",
                        path.display()
                    ))
                })?
            }
            ClaudeIdentityToken::Value(token) => token.clone(),
        };
        let token = token.trim();
        if token.is_empty() {
            return Err(ClaudeAuthUnavailable::new("the identity token is empty"));
        }
        Ok(token.to_owned())
    }
}

struct FederatedToken {
    http: reqwest::Client,
    token_url: String,
    identity: ClaudeWorkloadIdentity,
}

impl FederatedToken {
    async fn exchange(&self) -> Result<ClaudeAccessToken, ClaudeAuthUnavailable> {
        let identity = &self.identity;
        let mut body = json!({
            "grant_type": JWT_BEARER_GRANT,
            "assertion": identity.assertion().await?,
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
        _rejected: &'a http::HeaderValue,
    ) -> ClaudeAuthFuture<'a, Result<Option<ClaudeAccessToken>, ClaudeAuthUnavailable>> {
        Box::pin(async move { self.exchange().await.map(Some) })
    }
}

impl ClaudeClient {
    pub fn workload_identity(identity: ClaudeWorkloadIdentity) -> Self {
        nanocode_tls::install_default_rustls_crypto_provider();
        Self::federated(reqwest::Client::new(), ANTHROPIC_API_URL, identity)
    }

    pub fn messages_api_federated(
        base_url: &str,
        identity: ClaudeWorkloadIdentity,
        connect_timeout: Duration,
        idle_timeout: Duration,
    ) -> Result<Self, reqwest::Error> {
        nanocode_tls::install_default_rustls_crypto_provider();
        let http = reqwest::Client::builder()
            .connect_timeout(connect_timeout)
            .read_timeout(idle_timeout)
            .build()?;
        Ok(Self::federated(http, base_url, identity))
    }

    fn federated(http: reqwest::Client, base_url: &str, identity: ClaudeWorkloadIdentity) -> Self {
        let base_url = base_url.trim_end_matches('/');
        let token = FederatedToken {
            http: http.clone(),
            token_url: format!("{base_url}/oauth/token"),
            identity,
        };
        Self::with_auth_provider(
            http,
            format!("{base_url}/messages"),
            Arc::new(RefreshingClaudeAuth::new(
                Arc::new(token),
                MANDATORY_REFRESH,
            )),
        )
    }
}
