#![cfg_attr(
    target_family = "wasm",
    allow(
        dead_code,
        reason = "tool search, reloads and the native tool surface serve native hosts"
    )
)]

#[cfg(not(target_family = "wasm"))]
pub use nanocode_home::SessionEnvironment;
#[cfg(not(target_family = "wasm"))]
use nanocode_tool_provider::DynamicToolProvider;
use nanocode_tools::{Tool, ToolContext, ToolInput, ToolOutput, ToolOutputContent, ToolResult};

mod catalog;
mod client;
mod config;
#[cfg(not(target_family = "wasm"))]
mod native;
mod pagination;
mod search;
#[cfg(not(target_family = "wasm"))]
pub mod stdio;

#[cfg(not(target_family = "wasm"))]
use std::sync::OnceLock;
use std::{
    collections::{BTreeMap, btree_map::Entry},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[cfg(not(target_family = "wasm"))]
use async_trait::async_trait;
use catalog::{ConnectedCatalog, ProviderState, ToolEntry};
use nanocode_mcp::{
    CallToolParams, CallToolResult, ContentBlock, ContentKind,
    jsonrpc::{INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND, PARSE_ERROR},
    protocol::JsonObject,
};
use nanocode_tools::ToolDefinition;
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::{Instrument, info_span};

pub use config::{
    McpPaymentManagement, McpPaymentProvider, McpPendingPayment, McpServer, McpToolExposure,
};
#[cfg(not(target_family = "wasm"))]
pub use nanocode_mcp_http::oauth::{McpOAuthCredentials, McpOAuthRefreshGuard, McpOAuthStore};
pub use nanocode_mcp_http::streamable_http::BearerSource;

#[cfg(not(target_family = "wasm"))]
pub type McpFuture<'a, T> = futures_util::future::BoxFuture<'a, T>;
#[cfg(target_family = "wasm")]
pub type McpFuture<'a, T> = futures_util::future::LocalBoxFuture<'a, T>;

const MAX_TOOL_SEARCH_SOURCE_DESCRIPTION_BYTES: usize = 4 * 1024;
const CODEX_ENCRYPTED_CONTENT_META_KEY: &str = "codex/encryptedContent";
const CODEX_IMAGE_DETAIL_META_KEY: &str = "codex/imageDetail";

pub struct Mcp {
    servers: Arc<[NamedServer]>,
    state: Arc<ProviderState>,
    search: Arc<McpSearch>,
    #[cfg(not(target_family = "wasm"))]
    oauth_store: Option<Arc<dyn McpOAuthStore>>,
    #[cfg(not(target_family = "wasm"))]
    oauth_metadata: Arc<nanocode_mcp_http::oauth::OAuthMetadataCache>,
    #[cfg(target_family = "wasm")]
    http_sender: Option<Arc<dyn nanocode_http::layer::Sender>>,

    #[cfg(not(target_family = "wasm"))]
    session: Arc<OnceLock<SessionEnvironment>>,
    started: AtomicBool,

    #[cfg(not(target_family = "wasm"))]
    startup_tasks: OnceLock<tokio::task::JoinSet<()>>,
}

pub struct PreparedMcpTool {
    entry: Arc<ToolEntry>,
    timeout: Duration,
}

struct NamedServer {
    name: String,
    model_namespace: String,
    config: McpServer,
}

#[derive(Default)]
pub struct McpBuilder {
    servers: BTreeMap<String, McpServer>,
    #[cfg(not(target_family = "wasm"))]
    oauth_store: Option<Arc<dyn McpOAuthStore>>,
    #[cfg(target_family = "wasm")]
    http_sender: Option<Arc<dyn nanocode_http::layer::Sender>>,
    duplicate: Option<String>,
    discovery_timeout: Option<Duration>,
}

#[cfg(not(target_family = "wasm"))]
#[derive(Clone)]
pub struct McpHandle {
    servers: Arc<[NamedServer]>,
    state: Arc<ProviderState>,
    oauth_store: Option<Arc<dyn McpOAuthStore>>,
    oauth_metadata: Arc<nanocode_mcp_http::oauth::OAuthMetadataCache>,
    session: Arc<OnceLock<SessionEnvironment>>,
}

#[cfg(not(target_family = "wasm"))]
pub struct McpLogin {
    authorization_url: String,
    completion: tokio::task::JoinHandle<Result<usize, McpControlError>>,
}

#[derive(Debug, thiserror::Error)]
pub enum McpControlError {
    #[error("unknown MCP server `{0}`")]
    UnknownServer(String),

    #[error("MCP server `{0}` does not use Streamable HTTP")]
    NotHttp(String),

    #[error("MCP server `{0}` has explicit bearer authentication")]
    ExplicitBearer(String),

    #[error("no MCP OAuth credential store is configured")]
    NoOAuthStore,

    #[error("MCP OAuth login failed: {0}")]
    OAuth(String),

    #[error("MCP server `{server}` failed to reload: {error}")]
    Reload { server: String, error: String },

    #[error("MCP OAuth login task stopped: {0}")]
    LoginTask(String),
}

#[derive(Debug, thiserror::Error)]
pub enum McpCallError {
    #[error("{0}")]
    Session(nanocode_mcp::Error),

    #[error("{0}")]
    Payment(String),

    #[error("{0}")]
    Auth(String),

    #[error("exceeded {:.1} seconds", .0.as_secs_f64())]
    Timeout(Duration),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum McpCallEffect {
    Refused(Option<String>),
    Unsent,
    Unauthorized,
    Unknown,
}

impl McpCallError {
    #[must_use]
    pub fn effect(&self) -> McpCallEffect {
        match self {
            Self::Session(nanocode_mcp::Error::Rpc(error))
                if error.status().is_some()
                    || matches!(
                        error.code,
                        PARSE_ERROR
                            | INVALID_REQUEST
                            | METHOD_NOT_FOUND
                            | INVALID_PARAMS
                            | client::MCP_PAYMENT_REQUIRED_CODE
                    ) =>
            {
                McpCallEffect::Refused(Some(error.message.clone()))
            }
            Self::Session(nanocode_mcp::Error::Transport(error)) if error.is_unsent() => {
                McpCallEffect::Unsent
            }
            Self::Session(nanocode_mcp::Error::Transport(error)) => match error.status() {
                Some(401) => McpCallEffect::Unauthorized,
                Some(status @ 400..=499) => McpCallEffect::Refused(Some(format!("HTTP {status}"))),
                _ => McpCallEffect::Unknown,
            },
            Self::Auth(_) => McpCallEffect::Refused(None),
            _ => McpCallEffect::Unknown,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum McpBuildError {
    #[error("at least one MCP server is required")]
    Empty,

    #[error("MCP server name must not be empty")]
    EmptyName,

    #[error("MCP server `{0}` is configured more than once")]
    DuplicateServer(String),

    #[error("MCP server `{server}` has an empty {field}")]
    EmptyField { server: String, field: &'static str },

    #[error("MCP server `{server}` has a zero {field}")]
    ZeroTimeout { server: String, field: &'static str },

    #[error("MCP server `{server}` does not support option `{option}` for its transport")]
    UnsupportedOption {
        server: String,

        option: &'static str,
    },
}

impl Mcp {
    #[must_use]
    pub fn builder() -> McpBuilder {
        McpBuilder::default()
    }

    #[cfg(not(target_family = "wasm"))]
    #[must_use]
    pub fn handle(&self) -> McpHandle {
        McpHandle {
            servers: Arc::clone(&self.servers),
            state: Arc::clone(&self.state),
            oauth_store: self.oauth_store.clone(),
            oauth_metadata: Arc::clone(&self.oauth_metadata),
            session: Arc::clone(&self.session),
        }
    }

    pub fn prepared_snapshot<'call>(
        &'call self,
        max_timeout: Duration,
    ) -> McpFuture<'call, Result<Vec<PreparedMcpTool>, String>> {
        Box::pin(async move {
            self.start_servers();
            self.state.prepared_entries().await.map(|entries| {
                entries
                    .into_iter()
                    .map(|entry| PreparedMcpTool {
                        timeout: entry.timeout.min(max_timeout),
                        entry,
                    })
                    .collect()
            })
        })
    }
}

impl Mcp {
    pub fn prepared_servers(
        &self,
        max_timeout: Duration,
    ) -> McpFuture<'_, BTreeMap<String, Result<Vec<PreparedMcpTool>, String>>> {
        Box::pin(async move {
            self.start_servers();
            self.state
                .server_entries()
                .await
                .into_iter()
                .map(|(server, entries)| {
                    let tools = entries.map(|entries| {
                        entries
                            .into_iter()
                            .map(|entry| PreparedMcpTool {
                                timeout: entry.timeout.min(max_timeout),
                                entry,
                            })
                            .collect()
                    });
                    (server, tools)
                })
                .collect()
        })
    }
}

impl PreparedMcpTool {
    pub fn server(&self) -> &str {
        &self.entry.server_name
    }

    pub fn read_only(&self) -> bool {
        self.entry.read_only
    }

    pub fn call(
        &self,
        arguments: JsonObject,
    ) -> McpFuture<'_, Result<CallToolResult, McpCallError>> {
        Box::pin(async move {
            self.entry
                .client
                .refresh_oauth()
                .await
                .map_err(McpCallError::Auth)?;
            let params =
                CallToolParams::new(self.entry.remote_name.clone()).with_arguments(arguments);
            nanocode_platform::timeout(self.timeout, self.entry.client.call_tool(params))
                .await
                .map_err(|_| McpCallError::Timeout(self.timeout))?
        })
    }

    pub fn provider(&self) -> &str {
        self.entry.attached_provider()
    }

    pub fn remote_name(&self) -> &str {
        &self.entry.remote_name
    }

    pub fn definition(&self) -> &ToolDefinition {
        &self.entry.definition
    }

    pub fn supports_parallel_tool_calls(&self) -> bool {
        self.entry.supports_parallel_tool_calls
    }

    pub const fn timeout(&self) -> Duration {
        self.timeout
    }

    pub fn execute<'call>(
        &'call self,
        input: Value,
        context: ToolContext<'call>,
    ) -> McpFuture<'call, ToolOutput> {
        Box::pin(async move { execute_mcp_entry(&self.entry, input, self.timeout, context).await })
    }
}

#[cfg_attr(not(target_family = "wasm"), async_trait::async_trait)]
#[cfg_attr(target_family = "wasm", async_trait::async_trait(?Send))]
impl Tool for PreparedMcpTool {
    fn definition(&self) -> ToolDefinition {
        self.entry.definition.clone()
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        self.entry.supports_parallel_tool_calls
    }

    async fn execute(&self, input: ToolInput, context: ToolContext<'_>) -> ToolResult {
        let input = input.decode_json::<Value>()?;
        Ok(execute_mcp_entry(&self.entry, input, self.timeout, context).await)
    }
}

impl McpBuilder {
    #[cfg(target_family = "wasm")]
    #[must_use]
    pub fn http_sender(mut self, sender: Arc<dyn nanocode_http::layer::Sender>) -> Self {
        self.http_sender = Some(sender);
        self
    }

    #[cfg(not(target_family = "wasm"))]
    #[must_use]
    pub fn oauth_store(mut self, store: Arc<dyn McpOAuthStore>) -> Self {
        self.oauth_store = Some(store);
        self
    }

    #[must_use]
    pub const fn discovery_timeout(mut self, timeout: Duration) -> Self {
        self.discovery_timeout = Some(timeout);
        self
    }

    #[must_use]
    pub fn server(mut self, name: impl Into<String>, server: McpServer) -> Self {
        let name = name.into();
        match self.servers.entry(name) {
            Entry::Vacant(entry) => {
                entry.insert(server);
            }
            Entry::Occupied(entry) => {
                self.duplicate.get_or_insert_with(|| entry.key().clone());
            }
        }
        self
    }

    pub fn build(self) -> Result<Mcp, McpBuildError> {
        if self.servers.is_empty() {
            return Err(McpBuildError::Empty);
        }
        if let Some(name) = self.duplicate {
            return Err(McpBuildError::DuplicateServer(name));
        }
        let mut discovery_timeout = Duration::ZERO;
        let mut named = Vec::with_capacity(self.servers.len());
        let model_namespaces =
            catalog::normalized_server_names(self.servers.keys().map(String::as_str));
        for (name, server) in self.servers {
            validate_server(&name, &server)?;
            discovery_timeout = discovery_timeout.max(server.startup_timeout.saturating_mul(2));
            let model_namespace = model_namespaces
                .get(&name)
                .cloned()
                .ok_or(McpBuildError::EmptyName)?;
            named.push(NamedServer {
                name,
                model_namespace,
                config: server,
            });
        }
        let discovery_timeout = self.discovery_timeout.unwrap_or(discovery_timeout);
        let servers: Arc<[NamedServer]> = named.into();
        let state = Arc::new(ProviderState::new(
            servers.iter().map(|server| server.name.clone()),
            discovery_timeout,
        ));
        let search = Arc::new(McpSearch {
            state: Arc::clone(&state),
            description: search_description(&servers),
        });
        Ok(Mcp {
            servers,
            state,
            search,
            #[cfg(not(target_family = "wasm"))]
            oauth_store: self.oauth_store,
            #[cfg(not(target_family = "wasm"))]
            oauth_metadata: Arc::new(nanocode_mcp_http::oauth::OAuthMetadataCache::default()),
            #[cfg(target_family = "wasm")]
            http_sender: self.http_sender,
            #[cfg(not(target_family = "wasm"))]
            session: Arc::new(OnceLock::new()),
            started: AtomicBool::new(false),
            #[cfg(not(target_family = "wasm"))]
            startup_tasks: OnceLock::new(),
        })
    }
}

#[cfg(not(target_family = "wasm"))]
impl McpHandle {
    pub fn reload<'call>(
        &'call self,
        server_name: &'call str,
    ) -> McpFuture<'call, Result<usize, McpControlError>> {
        Box::pin(async move {
            let span = info_span!(
                target: "nanocode_code_mode",
                parent: None,
                "mcp.server_reload",
                otel.kind = "client",
                otel.status_code = tracing::field::Empty,
                mcp.server = server_name,
                status = tracing::field::Empty,
                tool.count = tracing::field::Empty,
            );

            let result = self.reload_inner(server_name, &span).await;
            span.record(
                "status",
                if result.is_ok() {
                    "completed"
                } else {
                    "failed"
                },
            );
            span.record(
                "otel.status_code",
                if result.is_ok() { "OK" } else { "ERROR" },
            );
            if let Ok(tool_count) = &result {
                span.record("tool.count", tool_count);
            }
            result
        })
    }

    async fn reload_inner(
        &self,
        server_name: &str,
        parent: &tracing::Span,
    ) -> Result<usize, McpControlError> {
        let server = self
            .servers
            .iter()
            .find(|server| server.name == server_name)
            .ok_or_else(|| McpControlError::UnknownServer(server_name.to_owned()))?;
        let generation = self.state.begin_server(server_name);
        let result = client::connect(
            server_name,
            &server.config,
            self.session.get(),
            self.oauth_store.clone(),
            Arc::clone(&self.oauth_metadata),
            parent,
        )
        .await;
        match result {
            Ok(connected) => {
                let count = connected.tools.len();
                let entries = ToolEntry::new_many(
                    server_name,
                    &server.model_namespace,
                    connected.tools,
                    Arc::clone(&connected.client),
                    &server.config,
                );
                self.state.complete_server(
                    server_name,
                    generation,
                    Ok(ConnectedCatalog {
                        client: connected.client,
                        entries,
                    }),
                );
                Ok(count)
            }
            Err(error) => {
                self.state
                    .complete_server(server_name, generation, Err(error.clone()));
                Err(McpControlError::Reload {
                    server: server_name.to_owned(),
                    error,
                })
            }
        }
    }

    pub fn login<'call>(
        &'call self,
        server_name: &'call str,
    ) -> McpFuture<'call, Result<McpLogin, McpControlError>> {
        Box::pin(async move {
            let span = info_span!(
                target: "nanocode_code_mode",
                parent: None,
                "mcp.oauth.login",
                otel.kind = "client",
                otel.status_code = tracing::field::Empty,
                mcp.server = server_name,
                status = tracing::field::Empty,
                tool.count = tracing::field::Empty,
            );
            self.login_inner(server_name, span).await
        })
    }

    async fn login_inner(
        &self,
        server_name: &str,
        span: tracing::Span,
    ) -> Result<McpLogin, McpControlError> {
        let server = self
            .servers
            .iter()
            .find(|server| server.name == server_name)
            .ok_or_else(|| McpControlError::UnknownServer(server_name.to_owned()))?;
        let store = self
            .oauth_store
            .clone()
            .ok_or(McpControlError::NoOAuthStore)?;
        let (url, bearer, headers) = match &server.config.transport {
            config::McpTransport::StreamableHttp {
                url,
                bearer,
                source,
                headers,
            } => (
                url.clone(),
                bearer.is_some() || source.is_some(),
                headers.clone(),
            ),
            config::McpTransport::Stdio { .. } => {
                return Err(McpControlError::NotHttp(server_name.to_owned()));
            }
        };
        if bearer {
            return Err(McpControlError::ExplicitBearer(server_name.to_owned()));
        }
        let flow =
            nanocode_mcp_http::oauth::begin_login(server_name.to_owned(), url, headers, store)
                .instrument(span.clone())
                .await
                .map_err(|error| {
                    span.record("status", "failed");
                    span.record("otel.status_code", "ERROR");
                    McpControlError::OAuth(error)
                })?;
        let handle = self.clone();
        let name = server_name.to_owned();
        let completion_span = span.clone();
        let completion = tokio::spawn(
            async move {
                let result = async {
                    flow.completion
                        .await
                        .map_err(|error| McpControlError::LoginTask(error.to_string()))?
                        .map_err(McpControlError::OAuth)?;
                    handle.reload(&name).await
                }
                .await;
                completion_span.record(
                    "status",
                    if result.is_ok() {
                        "completed"
                    } else {
                        "failed"
                    },
                );
                completion_span.record(
                    "otel.status_code",
                    if result.is_ok() { "OK" } else { "ERROR" },
                );
                if let Ok(tool_count) = &result {
                    completion_span.record("tool.count", tool_count);
                }
                result
            }
            .instrument(span),
        );
        Ok(McpLogin {
            authorization_url: flow.authorization_url,
            completion,
        })
    }
}

#[cfg(not(target_family = "wasm"))]
impl McpLogin {
    #[must_use]
    pub fn authorization_url(&self) -> &str {
        &self.authorization_url
    }

    pub fn wait(self) -> McpFuture<'static, Result<usize, McpControlError>> {
        Box::pin(async move {
            self.completion
                .await
                .map_err(|error| McpControlError::LoginTask(error.to_string()))?
        })
    }
}

impl Mcp {
    fn start_servers(&self) {
        if self.started.swap(true, Ordering::AcqRel) {
            return;
        }
        #[cfg(not(target_family = "wasm"))]
        let mut startup_tasks = tokio::task::JoinSet::new();
        for server in &*self.servers {
            let name = server.name.clone();
            let model_namespace = server.model_namespace.clone();
            let config = server.config.clone();
            let state = Arc::clone(&self.state);
            #[cfg(not(target_family = "wasm"))]
            let oauth_store = self.oauth_store.clone();
            #[cfg(not(target_family = "wasm"))]
            let oauth_metadata = Arc::clone(&self.oauth_metadata);
            #[cfg(not(target_family = "wasm"))]
            let session = self.session.get().cloned();
            #[cfg(target_family = "wasm")]
            let sender = self.http_sender.clone();
            let span = info_span!(
                target: "nanocode_code_mode",
                parent: None,
                "mcp.server_start",
                otel.kind = "client",
                otel.status_code = tracing::field::Empty,
                mcp.server = %name,
                status = tracing::field::Empty,
                tool.count = tracing::field::Empty,
            );
            let startup = async move {
                #[cfg(not(target_family = "wasm"))]
                let connected = client::connect(
                    &name,
                    &config,
                    session.as_ref(),
                    oauth_store,
                    oauth_metadata,
                    &span,
                )
                .await;
                #[cfg(target_family = "wasm")]
                let connected = client::connect(&name, &config, sender.as_ref(), &span).await;
                let result = connected.map(|connected| {
                    let entries = ToolEntry::new_many(
                        &name,
                        &model_namespace,
                        connected.tools,
                        Arc::clone(&connected.client),
                        &config,
                    );
                    ConnectedCatalog {
                        client: connected.client,
                        entries,
                    }
                });
                span.record(
                    "status",
                    if result.is_ok() {
                        "completed"
                    } else {
                        "failed"
                    },
                );
                span.record(
                    "otel.status_code",
                    if result.is_ok() { "OK" } else { "ERROR" },
                );
                if let Ok(catalog) = &result {
                    span.record("tool.count", catalog.entries.len());
                }
                state.complete_server(&name, 0, result);
            };
            #[cfg(not(target_family = "wasm"))]
            startup_tasks.spawn(startup);
            #[cfg(target_family = "wasm")]
            nanocode_platform::spawn_local(startup);
        }
        #[cfg(not(target_family = "wasm"))]
        let _ = self.startup_tasks.set(startup_tasks);
    }
}

#[cfg(not(target_family = "wasm"))]
#[async_trait]
impl DynamicToolProvider for Mcp {
    fn bind_session(&self, session: &SessionEnvironment) {
        let _ = self.session.set(session.clone());
    }

    fn start(&self) {
        self.start_servers();
    }

    fn direct_tools(&self) -> Vec<Arc<dyn Tool>> {
        vec![Arc::clone(&self.search) as Arc<dyn Tool>]
    }

    fn available_definitions(&self) -> Vec<ToolDefinition> {
        self.state.available_definitions()
    }

    fn contains(&self, name: &str) -> bool {
        self.state
            .entry(name)
            .is_some_and(|entry| entry.tool_exposure.is_callable())
    }

    fn supports_parallel_tool_calls(&self, name: &str) -> bool {
        self.state.entry(name).is_some_and(|entry| {
            entry.tool_exposure.is_callable() && entry.supports_parallel_tool_calls
        })
    }

    async fn execute(
        &self,
        name: &str,
        input: Value,
        context: ToolContext<'_>,
    ) -> Option<ToolOutput> {
        let entry = self.state.ready_entry(name).await?;
        if !entry.tool_exposure.is_callable() {
            return None;
        }
        Some(execute_mcp_entry(&entry, input, entry.timeout, context).await)
    }
}

async fn execute_mcp_entry(
    entry: &ToolEntry,
    input: Value,
    timeout: Duration,
    context: ToolContext<'_>,
) -> ToolOutput {
    let Value::Object(arguments) = input else {
        return ToolOutput::error(format!(
            "MCP tool {} requires an object argument",
            entry.canonical_name
        ));
    };
    let argument_bytes = serde_json::to_vec(&arguments).map_or(0, |encoded| encoded.len());
    let argument_keys = arguments
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(",");
    let argument_count = arguments.len();
    let mut params = CallToolParams::new(entry.remote_name.clone()).with_arguments(arguments);
    if let Some(turn_id) = context.turn_id() {
        params.insert_meta(
            "x-codex-turn-metadata",
            json!({"session_id":context.session_id(), "thread_id":context.session_id(), "turn_id":turn_id, "call_id":context.call_id(), "model":context.model()}),
        );
    }
    let span = info_span!(
        target: "nanocode_code_mode",
        "mcp.tool_call",
        otel.kind = "client",
        otel.status_code = tracing::field::Empty,
        mcp.server = %entry.server_name,
        mcp.tool = %entry.remote_name,
        mcp.arguments.bytes = argument_bytes,
        mcp.arguments.keys = argument_keys,
        mcp.arguments.count = argument_count,
        status = tracing::field::Empty,
    );
    if let Err(error) = entry.client.refresh_oauth().await {
        span.record("status", "failed");
        span.record("otel.status_code", "ERROR");
        return ToolOutput::error(format!(
            "MCP tool {}/{} could not refresh OAuth credentials: {error}",
            entry.server_name, entry.remote_name
        ));
    }
    let result = match nanocode_platform::timeout(
        timeout,
        entry.client.call_tool(params).instrument(span.clone()),
    )
    .await
    {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => {
            span.record("status", "failed");
            span.record("otel.status_code", "ERROR");
            return ToolOutput::error(format!(
                "MCP tool {}/{} failed: {error}",
                entry.server_name, entry.remote_name
            ));
        }
        Err(_) => {
            span.record("status", "timeout");
            span.record("otel.status_code", "ERROR");
            return ToolOutput::error(format!(
                "MCP tool {}/{} {}",
                entry.server_name,
                entry.remote_name,
                McpCallError::Timeout(timeout)
            ));
        }
    };
    let success = !result.is_error();
    span.record("status", if success { "completed" } else { "failed" });
    span.record("otel.status_code", if success { "OK" } else { "ERROR" });
    match tool_output(result, &entry.server_name, &entry.remote_name) {
        Ok(output) => output,
        Err(error) => {
            span.record("status", "failed");
            span.record("otel.status_code", "ERROR");
            ToolOutput::error(format!("failed to encode MCP tool result: {error}"))
        }
    }
}

pub fn tool_output(
    result: CallToolResult,
    server_name: &str,
    remote_name: &str,
) -> Result<ToolOutput, serde_json::Error> {
    let success = !result.is_error();
    let value = serde_json::to_value(&result)?;
    let result_metadata = result.meta.as_ref();
    let direct_content = direct_mcp_content(&result.content)?;
    let contains_encrypted_content = direct_content.as_ref().is_some_and(|content| {
        content
            .iter()
            .any(|item| matches!(item, ToolOutputContent::EncryptedContent { .. }))
    });
    let mut output = if contains_encrypted_content {
        let mut output = ToolOutput::content(direct_content.unwrap_or_default());
        output.success = success;
        output.with_structured_result(value)
    } else if let Some(content) = direct_content {
        let mut output = ToolOutput::content(content);
        output.success = success;
        output.with_structured_result(value)
    } else if let Some(structured_content) = result
        .structured_content
        .as_ref()
        .filter(|content| !content.is_null())
    {
        ToolOutput::from_json(structured_content.clone(), success).with_structured_result(value)
    } else {
        ToolOutput::from_json(serde_json::to_value(&result.content)?, success)
            .with_structured_result(value)
    };
    output = output.with_metadata(json!({
        "mcp_server": server_name,
        "mcp_tool": remote_name,
        "mcp_result_meta": result_metadata,
    }));
    Ok(output)
}

/// [`tool_output`] for a `tools/call` result held as JSON, such as one a host stored or received
/// itself; a value that is no `CallToolResult` is shown as JSON.
pub fn value_output(result: &Value, server_name: &str, remote_name: &str) -> ToolOutput {
    serde_json::from_value(result.clone())
        .ok()
        .and_then(|parsed| tool_output(parsed, server_name, remote_name).ok())
        .unwrap_or_else(|| ToolOutput::json(result))
}

fn direct_mcp_content(
    content: &[ContentBlock],
) -> Result<Option<Vec<ToolOutputContent>>, serde_json::Error> {
    let mut has_typed_content = false;
    let mut output = Vec::with_capacity(content.len());
    for item in content {
        let item = match item.kind() {
            ContentKind::Text { text, meta }
                if content_meta_bool(meta, CODEX_ENCRYPTED_CONTENT_META_KEY) =>
            {
                has_typed_content = true;
                ToolOutputContent::EncryptedContent {
                    encrypted_content: text.to_owned(),
                }
            }
            ContentKind::Text { text, .. } => ToolOutputContent::InputText {
                text: text.to_owned(),
            },
            ContentKind::Image {
                data,
                mime_type,
                meta,
            } => {
                has_typed_content = true;
                ToolOutputContent::InputImage {
                    image_url: media_data_url(data, mime_type),
                    detail: image_detail(meta),
                }
            }
            ContentKind::Audio {
                data, mime_type, ..
            } => {
                has_typed_content = true;
                ToolOutputContent::InputAudio {
                    audio_url: media_data_url(data, mime_type),
                }
            }
            ContentKind::Other => ToolOutputContent::InputText {
                text: serde_json::to_string(item)?,
            },
        };
        output.push(item);
    }
    Ok(has_typed_content.then_some(output))
}

fn content_meta_bool(meta: Option<&JsonObject>, key: &str) -> bool {
    meta.and_then(|meta| meta.get(key)).and_then(Value::as_bool) == Some(true)
}

fn media_data_url(data: &str, mime_type: &str) -> String {
    if data.starts_with("data:") {
        data.to_owned()
    } else {
        format!("data:{mime_type};base64,{data}")
    }
}

fn image_detail(meta: Option<&JsonObject>) -> nanocode_types::ImageDetail {
    match meta
        .and_then(|meta| meta.get(CODEX_IMAGE_DETAIL_META_KEY))
        .and_then(Value::as_str)
    {
        Some("low") => nanocode_types::ImageDetail::Low,
        Some("high") => nanocode_types::ImageDetail::High,
        Some("original") => nanocode_types::ImageDetail::Original,
        Some("auto") | Some(_) | None => nanocode_types::ImageDetail::Auto,
    }
}

struct McpSearch {
    state: Arc<ProviderState>,
    description: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchInput {
    query: String,
    #[serde(default)]
    limit: Option<usize>,
}

#[cfg_attr(not(target_family = "wasm"), async_trait::async_trait)]
#[cfg_attr(target_family = "wasm", async_trait::async_trait(?Send))]
impl Tool for McpSearch {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::tool_search(
            "client",
            self.description.clone(),
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query for deferred tools."
                    },
                    "limit": {
                        "type": "number",
                        "description": "Maximum number of tools to return. Defaults to 8."
                    }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        )
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    async fn execute(&self, input: ToolInput, _context: ToolContext<'_>) -> ToolResult {
        let input = input.decode_json::<SearchInput>()?;
        let span = info_span!(
            target: "nanocode_code_mode",
            "mcp.catalog_search",
            otel.kind = "internal",
            otel.status_code = tracing::field::Empty,
            query.bytes = input.query.len(),
            result.count = tracing::field::Empty,
            pending_servers = tracing::field::Empty,
            status = tracing::field::Empty,
        );
        tracing::info!(
            target: "nanocode_code_mode",
            parent: &span,
            query = %input.query,
            limit = input.limit,
            "MCP catalog search"
        );
        let result = self
            .state
            .search(&input.query, input.limit)
            .instrument(span.clone())
            .await;
        span.record(
            "status",
            if result.is_ok() {
                "completed"
            } else {
                "failed"
            },
        );
        span.record(
            "otel.status_code",
            if result.is_ok() { "OK" } else { "ERROR" },
        );
        if let Ok(result) = &result {
            span.record("result.count", result.tool_count());
            span.record("pending_servers", result.pending_server_count());
        }
        Ok(match result {
            Ok(result) => match result.loadable_tools() {
                Ok(tools) => ToolOutput::json(&result).with_structured_result(tools),
                Err(error) => {
                    ToolOutput::error(format!("failed to encode MCP tool definitions: {error}"))
                }
            },
            Err(error) => ToolOutput::error(error),
        })
    }
}

fn validate_server(name: &str, server: &McpServer) -> Result<(), McpBuildError> {
    if name.trim().is_empty() {
        return Err(McpBuildError::EmptyName);
    }
    if let Some(option) = server.unsupported_option {
        return Err(McpBuildError::UnsupportedOption {
            server: name.to_owned(),
            option,
        });
    }
    let (field, value) = match &server.transport {
        config::McpTransport::Stdio { command, .. } => ("command", command.as_str()),
        config::McpTransport::StreamableHttp { url, .. } => ("URL", url.as_str()),
    };
    if value.trim().is_empty() {
        return Err(McpBuildError::EmptyField {
            server: name.to_owned(),
            field,
        });
    }
    for (field, timeout) in [
        ("startup timeout", server.startup_timeout),
        ("tool timeout", server.tool_timeout),
    ] {
        if timeout.is_zero() {
            return Err(McpBuildError::ZeroTimeout {
                server: name.to_owned(),
                field,
            });
        }
    }
    Ok(())
}

fn search_description(servers: &[NamedServer]) -> String {
    let reserved_name_bytes = servers
        .iter()
        .fold(servers.len().saturating_sub(1), |reserved, server| {
            reserved.saturating_add(2).saturating_add(server.name.len())
        });
    let mut description_budget =
        MAX_TOOL_SEARCH_SOURCE_DESCRIPTION_BYTES.saturating_sub(reserved_name_bytes);
    let mut sources = String::new();
    for server in servers {
        let separator_bytes = usize::from(!sources.is_empty());
        let required = separator_bytes
            .saturating_add(2)
            .saturating_add(server.name.len());
        if required > MAX_TOOL_SEARCH_SOURCE_DESCRIPTION_BYTES.saturating_sub(sources.len()) {
            continue;
        }
        if !sources.is_empty() {
            sources.push('\n');
        }
        sources.push_str("- ");
        sources.push_str(&server.name);
        if let Some(description) = server.config.description.as_deref()
            && description_budget >= 2
        {
            sources.push_str(": ");
            description_budget -= 2;
            let description = take_bytes_at_char_boundary(description.trim(), description_budget);
            sources.push_str(description);
            description_budget -= description.len();
        }
    }
    format!(
        "# Tool discovery\n\nSearches over deferred tool metadata with BM25 and exposes matching tools for the next model call.\n\nYou have access to tools from the following sources:\n{sources}\nSome of the tools may not have been provided to you upfront. For MCP tool discovery, always use `tool_search`. Each search result reports whether it supports parallel tool calls. After discovery, invoke two or more independent supported tools together from one `exec` cell with `Promise.all` instead of using serial model rounds."
    )
}

fn take_bytes_at_char_boundary(value: &str, max_bytes: usize) -> &str {
    &value[..value.floor_char_boundary(max_bytes)]
}
