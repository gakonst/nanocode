use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[cfg(not(target_family = "wasm"))]
use http::header::PROXY_AUTHORIZATION;
use http::{HeaderMap, HeaderName, HeaderValue, header::USER_AGENT};
use nanocode_mcp::{
    CallToolParams, CallToolResult, Error as SessionError, Implementation, Inbound, Outgoing,
    Session, Tool,
};
use serde_json::Value;
#[cfg(not(target_family = "wasm"))]
use tokio::io::AsyncReadExt;
use tracing::{Instrument, Span, info_span};

#[cfg(not(target_family = "wasm"))]
use nanocode_mcp_http::oauth::{
    McpOAuthStore, OAuthMetadataCache, OAuthRuntime, transport_from_credentials,
};
#[cfg(not(target_family = "wasm"))]
use nanocode_mcp_http::streamable_http::BearerSource;
use nanocode_mcp_http::{
    SecretSource, error_chain,
    streamable_http::{self, HttpAuth},
};

use super::config::{McpPaymentManagement, McpPaymentProvider, McpServer, McpTransport};
use super::pagination::collect_paginated;
#[cfg(not(target_family = "wasm"))]
use super::stdio;
use super::{McpCallError, McpServerError};

const MCP_CLIENT_NAME: &str = "nanocode-mcp-client";
const MCP_USER_AGENT: &str = concat!("nanocode-mcp-client/", env!("CARGO_PKG_VERSION"));
#[cfg(not(target_family = "wasm"))]
const MCP_STDERR_CHUNK_BYTES: usize = 8 * 1024;

#[cfg(target_family = "wasm")]
pub(crate) enum OAuthRuntime {}

#[cfg(target_family = "wasm")]
impl OAuthRuntime {
    async fn refresh_if_needed(&self) -> Result<(), String> {
        match *self {}
    }
}

pub(crate) type Client = Arc<ClientInner>;

pub(crate) struct ClientInner {
    session: Session,
    oauth: Option<Arc<OAuthRuntime>>,
    payment: Option<Arc<dyn McpPaymentProvider>>,
}

const MCP_CREDENTIAL_META_KEY: &str = "org.paymentauth/credential";
pub(crate) const MCP_PAYMENT_REQUIRED_CODE: i64 = -32042;
const MCP_PAYMENT_REQUIRED_META_KEY: &str = "org.paymentauth/payment-required";
const MCP_RECEIPT_META_KEY: &str = "org.paymentauth/receipt";

impl ClientInner {
    pub(crate) async fn call_tool(
        &self,
        params: CallToolParams,
    ) -> Result<CallToolResult, McpCallError> {
        let first = self.session.call_tool(&params).await;
        let (payment_required, payment) = match (&first, &self.payment) {
            (Err(SessionError::Rpc(error)), Some(payment))
                if error.code == MCP_PAYMENT_REQUIRED_CODE =>
            {
                let Some(data) = error.data.as_ref() else {
                    return first.map_err(McpCallError::Session);
                };
                (data, payment)
            }
            (Ok(result), Some(payment)) => {
                let Some(data) = payment_required_result(result) else {
                    return first.map_err(McpCallError::Session);
                };
                (data, payment)
            }
            _ => return first.map_err(McpCallError::Session),
        };
        let management = ToolManagement {
            session: &self.session,
            params: params.clone(),
        };
        let Some(pending) = payment
            .prepare_with_management(payment_required, &management)
            .await
            .map_err(McpCallError::Payment)?
        else {
            return first.map_err(McpCallError::Session);
        };

        let mut paid = params;
        paid.insert_meta(MCP_CREDENTIAL_META_KEY, pending.credential().clone());
        match self.session.call_tool(&paid).await {
            Ok(result) if payment_required_result(&result).is_some() => {
                pending.rollback().await.map_err(McpCallError::Payment)?;
                Ok(result)
            }
            Ok(result) => {
                let mut pending = pending;
                if let Some(receipt) = result
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.get(MCP_RECEIPT_META_KEY))
                {
                    pending.receipt(receipt);
                }
                pending.commit().await.map_err(McpCallError::Payment)?;
                Ok(result)
            }
            Err(error @ SessionError::Rpc(_)) => {
                pending.rollback().await.map_err(McpCallError::Payment)?;
                Err(McpCallError::Session(error))
            }
            Err(error) => Err(McpCallError::Session(error)),
        }
    }

    async fn list_all_tools(&self) -> Result<Vec<Tool>, McpServerError> {
        let refused = AtomicBool::new(false);
        let tools = collect_paginated("tools/list", |cursor| {
            let refused = &refused;
            async move {
                let result = self.session.list_tools(cursor).await.map_err(|error| {
                    refused.store(is_refusal(&error), Ordering::Relaxed);
                    error_chain(&error)
                })?;
                Ok((result.tools, result.next_cursor))
            }
        })
        .await;
        tools.map_err(|message| McpServerError {
            message,
            refused: refused.load(Ordering::Relaxed),
        })
    }

    pub(crate) async fn native_tools(&self) -> Result<Vec<Tool>, String> {
        if !self.session.supports_tools() {
            return Ok(Vec::new());
        }
        self.refresh_oauth().await?;
        self.list_all_tools().await.map_err(|error| error.message)
    }

    pub(crate) fn supports_resources(&self) -> bool {
        self.session.supports_resources()
    }

    pub(crate) async fn list_resources(&self) -> Result<Value, String> {
        self.refresh_oauth().await?;
        let resources = collect_paginated("resources/list", |cursor| async move {
            let result = self
                .session
                .list_resources(cursor)
                .await
                .map_err(|e| e.to_string())?;
            Ok((result.resources, result.next_cursor))
        })
        .await?;
        let templates = collect_paginated("resources/templates/list", |cursor| async move {
            let result = self
                .session
                .list_resource_templates(cursor)
                .await
                .map_err(|e| e.to_string())?;
            Ok((result.resource_templates, result.next_cursor))
        })
        .await?;
        Ok(serde_json::json!({"resources": resources, "resourceTemplates": templates}))
    }

    pub(crate) async fn read_resource(&self, uri: &str) -> Result<Value, String> {
        self.refresh_oauth().await?;
        let result = self
            .session
            .read_resource(uri)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    pub(crate) async fn refresh_oauth(&self) -> Result<(), String> {
        if let Some(oauth) = &self.oauth {
            oauth.refresh_if_needed().await?;
        }
        Ok(())
    }
}

fn payment_required_result(result: &CallToolResult) -> Option<&Value> {
    result.meta.as_ref()?.get(MCP_PAYMENT_REQUIRED_META_KEY)
}

struct ToolManagement<'a> {
    session: &'a Session,
    params: CallToolParams,
}

#[async_trait::async_trait]
impl McpPaymentManagement for ToolManagement<'_> {
    async fn send(&self, credential: Value) -> Result<Option<Value>, String> {
        let mut params = self.params.clone();
        params.insert_meta(MCP_CREDENTIAL_META_KEY, credential);
        let result = self
            .session
            .call_tool(&params)
            .await
            .map_err(|error| error.to_string())?;
        if payment_required_result(&result).is_some() {
            return Err("the MCP server rejected the payment management credential".to_owned());
        }
        Ok(result
            .meta
            .as_ref()
            .and_then(|meta| meta.get(MCP_RECEIPT_META_KEY))
            .cloned())
    }
}

pub(crate) struct ConnectedServer {
    pub client: Client,
    pub tools: Vec<Tool>,
}

#[cfg(not(target_family = "wasm"))]
struct HttpConnect<'a> {
    server_name: &'a str,
    server: &'a McpServer,
    url: &'a str,
    bearer: Option<&'a SecretSource>,
    source: Option<Arc<dyn BearerSource>>,
    headers: &'a std::collections::BTreeMap<String, SecretSource>,
    oauth_store: Option<Arc<dyn McpOAuthStore>>,
    oauth_metadata: Arc<OAuthMetadataCache>,
    parent: &'a Span,
}

#[cfg(not(target_family = "wasm"))]
struct StoredOAuthConnect<'a> {
    server_name: &'a str,
    server: &'a McpServer,
    url: &'a str,
    http_client: reqwest::Client,
    store: Arc<dyn McpOAuthStore>,
    metadata: Arc<OAuthMetadataCache>,
    parent: &'a Span,
}

#[cfg(target_family = "wasm")]
pub(crate) async fn connect(
    server_name: &str,
    server: &McpServer,
    sender: Option<&Arc<dyn nanocode_http::layer::Sender>>,
    parent: &Span,
) -> Result<ConnectedServer, McpServerError> {
    let McpTransport::StreamableHttp {
        url,
        bearer,
        source,
        headers,
    } = &server.transport
    else {
        return Err(
            format!("MCP server `{server_name}` uses stdio, which needs a native target").into(),
        );
    };
    let sender = sender.ok_or_else(|| {
        format!("MCP server `{server_name}` needs an HTTP sender (McpBuilder::http_sender)")
    })?;
    if url.trim().is_empty() {
        return Err("Streamable HTTP URL must not be empty".to_owned().into());
    }
    let headers = resolve_http_headers(headers)?;
    streamable_http::validate_custom_headers(headers.keys())?;
    let auth = match (bearer, source) {
        (Some(bearer), _) => {
            let token = bearer.resolve()?;
            if token.trim().is_empty() {
                return Err("resolved bearer token must not be empty".to_owned().into());
            }
            HttpAuth::Bearer(token)
        }
        (None, Some(source)) => HttpAuth::Source(Arc::clone(source)),
        (None, None) => HttpAuth::None,
    };
    let (session, outgoing, inbound): (Session, Outgoing, Inbound) = Session::new();
    streamable_http::spawn_with_sender_options(
        url.clone(),
        Arc::clone(sender),
        headers,
        auth,
        server.http,
        outgoing,
        inbound,
    );
    let session = connect_session(server, session, parent).await?;
    finish_startup(server, session, None, parent).await
}

#[cfg(not(target_family = "wasm"))]
pub(crate) async fn connect(
    server_name: &str,
    server: &McpServer,
    session: Option<&crate::SessionEnvironment>,
    oauth_store: Option<Arc<dyn McpOAuthStore>>,
    oauth_metadata: Arc<OAuthMetadataCache>,
    parent: &Span,
) -> Result<ConnectedServer, McpServerError> {
    let (transport_name, auth_mode) = match &server.transport {
        McpTransport::Stdio { .. } => ("stdio", "none"),
        McpTransport::StreamableHttp { bearer, .. } if bearer.is_some() => {
            ("streamable_http", "bearer")
        }
        McpTransport::StreamableHttp { source, .. } if source.is_some() => {
            ("streamable_http", "bearer_source")
        }
        McpTransport::StreamableHttp { .. } if oauth_store.is_some() => {
            ("streamable_http", "oauth_store")
        }
        McpTransport::StreamableHttp { .. } => ("streamable_http", "none"),
    };
    let span = info_span!(
        target: "nanocode_code_mode",
        parent: parent,
        "mcp.transport_connect",
        otel.kind = "client",
        otel.status_code = tracing::field::Empty,
        mcp.server = server_name,
        mcp.transport = transport_name,
        mcp.auth = auth_mode,
        status = tracing::field::Empty,
    );

    let result = match &server.transport {
        McpTransport::Stdio {
            command,
            args,
            env,
            cwd,
        } => {
            if command.trim().is_empty() {
                return Err("stdio command must not be empty".to_owned().into());
            }
            let mut command = tokio::process::Command::new(command);
            command.args(args).envs(env);

            match session {
                Some(session) => session.apply(command.as_std_mut()),
                None => crate::SessionEnvironment::clear(command.as_std_mut()),
            }
            if let Some(cwd) = cwd {
                command.current_dir(cwd);
            }
            let (session, outgoing, inbound) = Session::new();
            let stderr = stdio::spawn(command, outgoing, inbound)
                .map_err(|error| format!("failed to launch stdio transport: {error}"))?;
            if let Some(stderr) = stderr {
                drain_server_stderr(server_name.to_owned(), stderr);
            }
            let session = connect_session(server, session, &span).await?;
            finish_startup(server, session, None, &span).await
        }
        McpTransport::StreamableHttp {
            url,
            bearer,
            source,
            headers,
        } => {
            connect_http(HttpConnect {
                server_name,
                server,
                url,
                bearer: bearer.as_ref(),
                source: source.clone(),
                headers,
                oauth_store,
                oauth_metadata,
                parent: &span,
            })
            .await
        }
    };
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
    result
}

#[cfg(not(target_family = "wasm"))]
async fn connect_http(input: HttpConnect<'_>) -> Result<ConnectedServer, McpServerError> {
    let HttpConnect {
        server_name,
        server,
        url,
        bearer,
        source,
        headers,
        oauth_store,
        oauth_metadata,
        parent,
    } = input;
    if url.trim().is_empty() {
        return Err("Streamable HTTP URL must not be empty".to_owned().into());
    }
    let default_headers = resolve_http_headers(headers)?;
    streamable_http::validate_custom_headers(default_headers.keys())?;
    let replays_plaintext_proxy_credentials = default_headers.contains_key(PROXY_AUTHORIZATION);
    nanocode_tls::install_default_rustls_crypto_provider();
    let mut http_client = reqwest::Client::builder();
    if !server.http.reuse_connections {
        http_client = http_client.pool_max_idle_per_host(0);
    }
    let http_client = http_client
        .redirect(nanocode_mcp_http::same_origin_redirect_policy(
            replays_plaintext_proxy_credentials,
        ))
        .default_headers(default_headers)
        .build()
        .map_err(|error| format!("failed to build MCP HTTP client: {error}"))?;
    if let Some(bearer) = bearer {
        let token = bearer.resolve()?;
        if token.trim().is_empty() {
            return Err("resolved bearer token must not be empty".to_owned().into());
        }
        let session =
            connect_http_session(server, url, http_client, HttpAuth::Bearer(token), parent).await?;
        return finish_startup(server, session, None, parent).await;
    }
    if let Some(source) = source {
        let session =
            connect_http_session(server, url, http_client, HttpAuth::Source(source), parent)
                .await?;
        return finish_startup(server, session, None, parent).await;
    }
    if let Some(store) = oauth_store {
        return connect_stored_oauth(StoredOAuthConnect {
            server_name,
            server,
            url,
            http_client,
            store,
            metadata: oauth_metadata,
            parent,
        })
        .await;
    }
    let session = connect_http_session(server, url, http_client, HttpAuth::None, parent).await?;
    finish_startup(server, session, None, parent).await
}

#[cfg(not(target_family = "wasm"))]
async fn connect_http_session(
    server: &McpServer,
    url: &str,
    http_client: reqwest::Client,
    auth: HttpAuth,
    parent: &Span,
) -> Result<Session, McpServerError> {
    let (session, outgoing, inbound): (Session, Outgoing, Inbound) = Session::new();
    streamable_http::spawn_with_options(
        url.to_owned(),
        http_client,
        auth,
        server.http,
        outgoing,
        inbound,
    );
    connect_session(server, session, parent).await
}

fn resolve_http_headers(
    headers: &std::collections::BTreeMap<String, SecretSource>,
) -> Result<HeaderMap, String> {
    let mut default_headers = HeaderMap::with_capacity(headers.len().saturating_add(1));
    default_headers.insert(USER_AGENT, HeaderValue::from_static(MCP_USER_AGENT));
    for (name, source) in headers {
        let name = name
            .parse::<HeaderName>()
            .map_err(|error| format!("invalid HTTP header name `{name}`: {error}"))?;
        let value = source.resolve()?;
        let mut value = HeaderValue::from_str(&value)
            .map_err(|error| format!("invalid value for HTTP header `{name}`: {error}"))?;
        value.set_sensitive(true);
        default_headers.insert(name, value);
    }
    Ok(default_headers)
}

#[cfg(not(target_family = "wasm"))]
async fn connect_stored_oauth(
    input: StoredOAuthConnect<'_>,
) -> Result<ConnectedServer, McpServerError> {
    let StoredOAuthConnect {
        server_name,
        server,
        url,
        http_client,
        store,
        metadata,
        parent,
    } = input;
    let load_span = info_span!(
        target: "nanocode_code_mode",
        parent: parent,
        "mcp.oauth.credentials_load",
        otel.kind = "internal",
        otel.status_code = tracing::field::Empty,
        status = tracing::field::Empty,
        credential.found = tracing::field::Empty,
    );
    let credentials = store
        .load(server_name, url)
        .instrument(load_span.clone())
        .await;
    load_span.record(
        "status",
        if credentials.is_ok() {
            "completed"
        } else {
            "failed"
        },
    );
    load_span.record(
        "otel.status_code",
        if credentials.is_ok() { "OK" } else { "ERROR" },
    );
    if let Ok(credentials) = &credentials {
        load_span.record("credential.found", credentials.is_some());
    }
    let Some(credentials) = credentials? else {
        let session =
            connect_http_session(server, url, http_client, HttpAuth::None, parent).await?;
        return finish_startup(server, session, None, parent).await;
    };

    let restore_span = info_span!(
        target: "nanocode_code_mode",
        parent: parent,
        "mcp.oauth.restore",
        otel.kind = "internal",
        otel.status_code = tracing::field::Empty,
        status = tracing::field::Empty,
        metadata.cache_hit = tracing::field::Empty,
    );
    let oauth = transport_from_credentials(
        server_name,
        url,
        http_client.clone(),
        store,
        credentials,
        &metadata,
    )
    .instrument(restore_span.clone())
    .await;
    restore_span.record("status", if oauth.is_ok() { "completed" } else { "failed" });
    restore_span.record(
        "otel.status_code",
        if oauth.is_ok() { "OK" } else { "ERROR" },
    );
    if let Ok(oauth) = &oauth {
        restore_span.record("metadata.cache_hit", oauth.metadata_cache_hit);
    }
    let oauth = oauth?;
    let runtime = oauth.runtime;
    runtime.refresh_if_needed().await?;
    let session = connect_http_session(
        server,
        url,
        http_client,
        HttpAuth::OAuth(Arc::clone(&runtime)),
        parent,
    )
    .await?;
    finish_startup(server, session, Some(runtime), parent).await
}

async fn connect_session(
    server: &McpServer,
    session: Session,
    parent: &Span,
) -> Result<Session, McpServerError> {
    let span = info_span!(
        target: "nanocode_code_mode",
        parent: parent,
        "mcp.initialize",
        otel.kind = "client",
        otel.status_code = tracing::field::Empty,
        status = tracing::field::Empty,
    );

    let client_info = Implementation::new(MCP_CLIENT_NAME, env!("CARGO_PKG_VERSION"));
    let result =
        match nanocode_platform::timeout(server.startup_timeout, session.initialize(client_info))
            .await
        {
            Ok(Ok(_)) => Ok(session),
            Ok(Err(error)) => Err(McpServerError {
                message: format!("MCP initialize failed: {}", error_chain(&error)),
                refused: is_refusal(&error),
            }),
            Err(_) => Err(startup_timeout(server, "initialize").into()),
        };
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
    result
}

#[cfg(not(target_family = "wasm"))]
fn drain_server_stderr(server_name: String, stderr: tokio::process::ChildStderr) {
    drop(tokio::spawn(async move {
        let mut stderr = stderr;
        let mut chunk = [0_u8; MCP_STDERR_CHUNK_BYTES];
        let mut chunk_index = 0_u64;
        loop {
            match stderr.read(&mut chunk).await {
                Ok(0) => break,
                Ok(size) => {
                    let content = &chunk[..size];
                    if let Ok(message) = std::str::from_utf8(content) {
                        tracing::info!(
                            target: "nanocode_code_mode",
                            server = %server_name,
                            chunk.index = chunk_index,
                            chunk.bytes = size,
                            message,
                            "MCP server stderr"
                        );
                    } else {
                        tracing::info!(
                            target: "nanocode_code_mode",
                            server = %server_name,
                            chunk.index = chunk_index,
                            chunk.bytes = size,
                            message.bytes = ?content,
                            "MCP server stderr"
                        );
                    }
                    chunk_index = chunk_index.saturating_add(1);
                }
                Err(error) => {
                    tracing::warn!(
                        target: "nanocode_code_mode",
                        server = %server_name,
                        %error,
                        "failed to read MCP server stderr"
                    );
                    break;
                }
            }
        }
    }));
}

async fn finish_startup(
    server: &McpServer,
    session: Session,
    oauth: Option<Arc<OAuthRuntime>>,
    parent: &Span,
) -> Result<ConnectedServer, McpServerError> {
    let client = Arc::new(ClientInner {
        session,
        oauth,
        payment: server.payment.clone(),
    });
    let span = info_span!(
        target: "nanocode_code_mode",
        parent: parent,
        "mcp.tools_list",
        otel.kind = "client",
        otel.status_code = tracing::field::Empty,
        status = tracing::field::Empty,
        tool.count = tracing::field::Empty,
    );
    client.refresh_oauth().await?;
    let tools = if !client.session.supports_tools() {
        Ok(Vec::new())
    } else {
        match nanocode_platform::timeout(
            server.startup_timeout,
            client.list_all_tools().instrument(span.clone()),
        )
        .await
        {
            Ok(Ok(tools)) => Ok(tools
                .into_iter()
                .filter(|tool| server.includes_tool(&tool.name))
                .collect::<Vec<_>>()),
            Ok(Err(error)) => Err(McpServerError {
                message: format!("MCP tools/list failed: {}", error.message),
                refused: error.refused,
            }),
            Err(_) => Err(startup_timeout(server, "tools/list").into()),
        }
    };
    span.record("status", if tools.is_ok() { "completed" } else { "failed" });
    span.record(
        "otel.status_code",
        if tools.is_ok() { "OK" } else { "ERROR" },
    );
    if let Ok(tools) = &tools {
        span.record("tool.count", tools.len());
    }
    let tools = tools?;
    Ok(ConnectedServer { client, tools })
}

/// Whether the server definitely turned the request down: a JSON-RPC error
/// or an HTTP 4xx other than 401, not a missing answer, a 5xx or a 401.
fn is_refusal(error: &SessionError) -> bool {
    let refusal = |status: u16| status != 401 && (400..500).contains(&status);
    match error {
        SessionError::Rpc(error) => error.status().is_none_or(refusal),
        SessionError::Transport(error) => error.status().is_some_and(refusal),
        _ => false,
    }
}

fn startup_timeout(server: &McpServer, operation: &str) -> String {
    format!(
        "MCP {operation} exceeded {:.1} seconds",
        server.startup_timeout.as_secs_f64()
    )
}
