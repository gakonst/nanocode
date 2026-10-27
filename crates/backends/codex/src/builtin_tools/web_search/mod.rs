mod history;
pub(crate) mod schema;
pub(crate) mod wire;

use std::time::Duration;

use crate::client::auth::{OpenAiAuth, OpenAiAuthError, OpenAiAuthMode, OpenAiAuthSnapshot};
use nanocode_tools::ToolDefinition;
use reqwest::header::{AUTHORIZATION, USER_AGENT};
use serde_json::json;
use tokio::time::{sleep, timeout};

use self::{
    history::recent_input,
    schema::commands_schema,
    wire::{SearchCommands, SearchRequest, SearchResponse, SearchSettings},
};
use super::{Tool, ToolContext, ToolInput, ToolOutput, ToolResult, WebSearchConfig};

const DESCRIPTION: &str = include_str!("web_run_description.md");
const ERROR_BODY_LIMIT: usize = 4_096;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_ATTEMPTS: usize = 2;
const TOOL_TIMEOUT: Duration = Duration::from_secs(45);
const RETRY_DELAY: Duration = Duration::from_millis(200);

pub struct WebSearchHandler {
    client: reqwest::Client,
    endpoint: String,
    auth: OpenAiAuth,
    http_layer: Option<std::sync::Arc<dyn nanocode_http::layer::Layer>>,
}

impl WebSearchHandler {
    pub fn with_client(config: WebSearchConfig, client: reqwest::Client) -> Self {
        Self {
            client,
            endpoint: config.endpoint,
            auth: config.auth,
            http_layer: config.http_layer,
        }
    }

    async fn run(&self, input: &str, context: ToolContext<'_>) -> ToolOutput {
        match timeout(TOOL_TIMEOUT, self.run_inner(input, context)).await {
            Ok(execution) => execution,
            Err(_) => ToolOutput::error(format!(
                "standalone web search timed out after {} seconds",
                TOOL_TIMEOUT.as_secs()
            )),
        }
    }

    async fn run_inner(&self, input: &str, context: ToolContext<'_>) -> ToolOutput {
        let commands = if input.trim().is_empty() {
            SearchCommands::default()
        } else {
            match serde_json::from_str(input) {
                Ok(commands) => commands,
                Err(error) => {
                    return ToolOutput::error(format!(
                        "failed to parse web.run arguments: {error}"
                    ));
                }
            }
        };
        let input = recent_input(context.history());
        let request = SearchRequest {
            id: context.session_id(),
            model: context.model(),
            input: input.as_deref(),
            commands: &commands,
            settings: SearchSettings {
                allowed_callers: ["direct"],
                external_web_access: true,
            },
            max_output_tokens: (context.output_token_budget() != usize::MAX)
                .then(|| u64::try_from(context.output_token_budget()).unwrap_or(u64::MAX)),
        };
        match self.search(&request).await {
            Ok(response) => {
                let mut output = ToolOutput::text(response.output);
                if let Some(results) = response.results {
                    output = output.with_metadata(json!({ "results": results }));
                }
                output
            }
            Err(error) => ToolOutput::error(error),
        }
    }

    async fn search(&self, request: &SearchRequest<'_>) -> Result<SearchResponse, String> {
        for attempt in 1..=MAX_ATTEMPTS {
            let (status, body) = match self.send(request).await {
                Ok(response) => response,
                Err(error) => {
                    if error.retryable && attempt < MAX_ATTEMPTS {
                        sleep(RETRY_DELAY).await;
                        continue;
                    }
                    return Err(error.message);
                }
            };
            let retryable = status.is_server_error();
            if retryable && attempt < MAX_ATTEMPTS {
                sleep(RETRY_DELAY).await;
                continue;
            }
            if !status.is_success() {
                return Err(format!(
                    "standalone web search returned HTTP {status}: {}",
                    body_preview(&body)
                ));
            }
            return serde_json::from_slice(&body).map_err(|error| {
                format!("failed to decode standalone web search response: {error}")
            });
        }
        Err("standalone web search exhausted its retry attempts".to_owned())
    }

    async fn send(
        &self,
        request: &SearchRequest<'_>,
    ) -> Result<(reqwest::StatusCode, Vec<u8>), RequestFailure> {
        let auth = self
            .auth
            .snapshot()
            .await
            .map_err(|error| auth_failure(&error))?;
        let response = self.send_authorized(request, &auth).await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            && auth.mode() == OpenAiAuthMode::ChatGpt
        {
            self.auth
                .recover_unauthorized(&auth)
                .await
                .map_err(|error| auth_failure(&error))?;
            let refreshed = self
                .auth
                .snapshot()
                .await
                .map_err(|error| auth_failure(&error))?;
            return self
                .read_response(
                    self.send_authorized(request, &refreshed).await?,
                    &[&auth, &refreshed],
                )
                .await;
        }
        self.read_response(response, &[&auth]).await
    }

    async fn send_authorized(
        &self,
        body: &SearchRequest<'_>,
        auth: &OpenAiAuthSnapshot,
    ) -> Result<reqwest::Response, RequestFailure> {
        let mut request = self
            .client
            .post(&self.endpoint)
            .header(USER_AGENT, concat!("nanocode/", env!("CARGO_PKG_VERSION")))
            .header(AUTHORIZATION, format!("Bearer {}", auth.bearer()));
        if let Some(account_id) = auth.account_id() {
            request = request.header("ChatGPT-Account-ID", account_id);
        }
        if auth.is_fedramp() {
            request = request.header("X-OpenAI-Fedramp", "true");
        }
        let request = request.json(body).build().map_err(|error| RequestFailure {
            message: format!("standalone web search request failed: {error}"),
            retryable: false,
        })?;
        nanocode_http::execute(&self.client, self.http_layer.as_deref(), request)
            .await
            .map_err(|error| RequestFailure {
                message: format!("standalone web search request failed: {error}"),
                retryable: error.is_retryable() || error.is_unsent(),
            })
    }

    async fn read_response(
        &self,
        response: reqwest::Response,
        used: &[&OpenAiAuthSnapshot],
    ) -> Result<(reqwest::StatusCode, Vec<u8>), RequestFailure> {
        let status = response.status();
        let body = read_response_body(response).await.map_err(|mut failure| {
            failure.retryable |= status.is_server_error();
            failure
        })?;
        if status.is_success() {
            return Ok((status, body));
        }
        Ok((status, redact_credentials(body, used)))
    }
}

fn redact_credentials(body: Vec<u8>, used: &[&OpenAiAuthSnapshot]) -> Vec<u8> {
    let mut text = match String::from_utf8_lossy(&body) {
        text if used
            .iter()
            .any(|auth| auth.sends_bearer() && text.contains(auth.bearer())) =>
        {
            text.into_owned()
        }
        _ => return body,
    };
    for auth in used.iter().filter(|auth| auth.sends_bearer()) {
        text = text.replace(auth.bearer(), "[redacted]");
    }
    text.into_bytes()
}

fn auth_failure(error: &OpenAiAuthError) -> RequestFailure {
    RequestFailure {
        message: error.to_string(),
        retryable: false,
    }
}

#[async_trait::async_trait]
impl Tool for WebSearchHandler {
    fn definition(&self) -> ToolDefinition {
        static DEFINITION: std::sync::LazyLock<ToolDefinition> = std::sync::LazyLock::new(|| {
            ToolDefinition::function("web__run", DESCRIPTION, commands_schema())
        });
        DEFINITION.clone()
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    async fn execute(&self, input: ToolInput, context: ToolContext<'_>) -> ToolResult {
        let input = input.function_json()?;
        Ok(self.run(input.get(), context).await)
    }
}

fn body_preview(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let end = text.floor_char_boundary(ERROR_BODY_LIMIT);
    let suffix = if end < text.len() { "…" } else { "" };
    format!("{}{suffix}", &text[..end])
}

struct RequestFailure {
    message: String,
    retryable: bool,
}

async fn read_response_body(mut response: reqwest::Response) -> Result<Vec<u8>, RequestFailure> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(response_too_large());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| RequestFailure {
        message: format!("failed to read standalone web search response: {error}"),
        retryable: true,
    })? {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(response_too_large());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn response_too_large() -> RequestFailure {
    RequestFailure {
        message: format!(
            "standalone web search response exceeded the {MAX_RESPONSE_BYTES}-byte limit"
        ),
        retryable: false,
    }
}
